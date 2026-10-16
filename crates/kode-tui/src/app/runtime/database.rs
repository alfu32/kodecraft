//! Database composition services. Configuration/credential I/O is explicit and
//! runs on blocking workers; JDBC methods use the lazy, tracked native clients.
use crate::panels::database::connection_dialog::Draft;
use kode_core::{DomainError, RequestId, Result};
use kode_database::{
    bridge::{
        client::Options,
        runtime::{Installation, Runtime},
    },
    catalog::{DriverRegistry, UrlCatalog},
    config::{DataSourceRepository, DatabasePaths, recents::UrlRecentRepository},
    connections::{Connections, TestResult},
    credentials::CredentialStore,
    drivers::Installer,
    metadata::Metadata,
    model::DataSourceDefinition,
};
use kode_platform::{
    jobs::CancellationToken,
    paths::ResourcePaths,
    process::{ProcessFactory, SystemProcessFactory},
};
use std::sync::Arc;
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct DatabaseConfiguration {
    pub paths: DatabasePaths,
    pub registry: Arc<DriverRegistry>,
    pub urls: Arc<UrlCatalog>,
    pub installation: Installation,
}
impl DatabaseConfiguration {
    /// Call alongside other resource loading on the startup blocking worker.
    /// User database files and credentials are not read here.
    pub fn from_environment(resources: &ResourcePaths) -> Result<Self> {
        let registry = Arc::new(DriverRegistry::from_resources(resources)?);
        Ok(Self {
            paths: DatabasePaths::from_environment(resources),
            urls: Arc::new(UrlCatalog::from_resources(resources, &registry)?),
            registry,
            installation: Installation::from_environment(None, resources),
        })
    }
}
pub struct Services {
    pub config: DatabaseConfiguration,
    pub connections: Connections,
    pub metadata: Metadata,
    pub installer: Installer,
    pub executor: kode_database::execution::Executor,
    mutations: Arc<Semaphore>,
    process_factory: Arc<dyn ProcessFactory>,
}
pub struct Saved {
    pub sources: Vec<DataSourceDefinition>,
    pub warning: Option<String>,
}
impl Services {
    pub fn new(config: DatabaseConfiguration) -> Result<Self> {
        Self::with_factory(config, Arc::new(SystemProcessFactory))
    }
    pub fn with_factory(
        config: DatabaseConfiguration,
        factory: Arc<dyn ProcessFactory>,
    ) -> Result<Self> {
        let options = Installer::configure(Options::default(), &config.paths)
            .map_err(|e| DomainError::Unsupported(e.to_string()))?;
        let installer = Installer::new(Runtime::new_with_managed_java(
            config.installation.clone(),
            options,
            factory.clone(),
            kode_database::java_runtime::storage_root(&config.paths.root),
        ));
        let connections = Connections::new(
            installer.clone(),
            CredentialStore::new(config.paths.root.clone()),
        );
        Ok(Self {
            config,
            metadata: Metadata::new(connections.clone()),
            executor: kode_database::execution::Executor::new(connections.clone()),
            connections,
            installer,
            mutations: Arc::new(Semaphore::new(1)),
            process_factory: factory,
        })
    }
    pub async fn install_java_runtime(
        &self,
        cancel: &CancellationToken,
        progress: tokio::sync::watch::Sender<kode_database::java_runtime::Progress>,
    ) -> Result<String> {
        // Recheck on explicit acceptance. A newly configured/local JVM wins and
        // no release metadata is fetched when Java is already usable.
        match self.connections.runtime().get(cancel).await {
            Ok(_) => {
                return Ok("Java is already available. Retry Test or Connect explicitly.".into());
            }
            Err(error) if error.requires_java_runtime() => {}
            Err(error) => return Err(DomainError::Unsupported(error.to_string())),
        }
        let root = kode_database::java_runtime::storage_root(&self.config.paths.root);
        let factory = self.process_factory.clone();
        let handle = tokio::runtime::Handle::current();
        let worker_cancel = cancel.clone();
        let _cancel_on_drop = cancel.clone().drop_guard();
        tokio::task::spawn_blocking(move || {
            let options = kode_database::java_runtime::Options::from_environment(root)?;
            kode_database::java_runtime::Installer::new(options, handle, factory)?
                .run(&worker_cancel, &progress)
        })
        .await
        .map_err(|_| {
            DomainError::Unsupported("Java runtime installation worker failed".into())
        })??;
        self.connections
            .runtime()
            .acknowledge_java_runtime_install();
        Ok("Java runtime installed. Retry Test or Connect explicitly.".into())
    }
    pub async fn sources(&self) -> Result<Vec<DataSourceDefinition>> {
        let repository = DataSourceRepository::new(self.config.paths.root.clone());
        blocking(move || repository.load()).await
    }
    pub async fn recents(&self, driver: &str) -> Result<Vec<String>> {
        let repository = UrlRecentRepository::new(self.config.paths.root.clone());
        let family = self.config.urls.recent_family(driver).to_owned();
        blocking(move || repository.load(&family)).await
    }
    async fn remember(&self, definition: &DataSourceDefinition) -> Option<String> {
        let repository = UrlRecentRepository::new(self.config.paths.root.clone());
        let family = self
            .config
            .urls
            .recent_family(&definition.driver.id)
            .to_owned();
        let url = definition.jdbc_url.clone();
        blocking(move || repository.remember(&family, &url).map(|_| ()))
            .await
            .err()
            .map(|_| "Connection succeeded; URL recents could not be saved".into())
    }
    pub async fn connect(
        &self,
        definition: DataSourceDefinition,
        cancel: &CancellationToken,
    ) -> Result<Option<String>> {
        self.connections
            .open(definition.clone(), None, cancel)
            .await
            .map_err(connection_error)?;
        Ok(self.remember(&definition).await)
    }
    pub async fn test(
        &self,
        draft: Draft,
        cancel: &CancellationToken,
    ) -> Result<(TestResult, Option<String>)> {
        let result = self
            .connections
            .test(draft.definition.clone(), draft.password, cancel)
            .await
            .map_err(connection_error)?;
        let warning = self.remember(&draft.definition).await;
        Ok((result, warning))
    }
    pub async fn save(&self, mut draft: Draft, cancel: &CancellationToken) -> Result<Saved> {
        let _permit = tokio::select! {biased;_=cancel.cancelled()=>return Err(DomainError::Cancelled),permit=self.mutations.clone().acquire_owned()=>permit.map_err(|_|DomainError::Cancelled)?};
        // A changed private value under the same reference also requires close.
        self.connections
            .definition_changed(&draft.definition.id, cancel)
            .await
            .map_err(connection_error)?;
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        let root = self.config.paths.root.clone();
        let nonce = RequestId::fresh()?.0;
        blocking(move || {
            let repository = DataSourceRepository::new(root.clone());
            let credentials = CredentialStore::new(root);
            let before = repository.load()?;
            let previous = before
                .iter()
                .find(|ds| ds.id == draft.definition.id)
                .cloned();
            let mut created = None;
            if let Some(password) = &draft.password {
                // Never change an existing reference before publishing the new
                // definition. Failed publication can clean up this owned entry.
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                let reference = format!("{}.password-{stamp}-{nonce}", draft.definition.id);
                credentials.put(&reference, password)?;
                draft.definition.credential_reference = Some(reference.clone());
                created = Some(reference);
            }
            let sources = match repository.upsert(draft.definition) {
                Ok(sources) => sources,
                Err(error) => {
                    if let Some(reference) = created {
                        let _ = credentials.remove(Some(&reference));
                    }
                    return Err(error);
                }
            };
            let warning = previous
                .and_then(|previous| remove_unused(&credentials, &sources, &previous).err())
                .map(|_| "Definition saved; an old private credential could not be removed".into());
            Ok(Saved { sources, warning })
        })
        .await
    }
    pub async fn remove(&self, source: String, cancel: &CancellationToken) -> Result<Saved> {
        let _permit = tokio::select! {biased;_=cancel.cancelled()=>return Err(DomainError::Cancelled),permit=self.mutations.clone().acquire_owned()=>permit.map_err(|_|DomainError::Cancelled)?};
        self.connections
            .close(&source, cancel)
            .await
            .map_err(connection_error)?;
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        let root = self.config.paths.root.clone();
        blocking(move || {
            let repository = DataSourceRepository::new(root.clone());
            let credentials = CredentialStore::new(root);
            let previous = repository.load()?.into_iter().find(|ds| ds.id == source);
            let sources = repository.remove(&source)?;
            let warning = previous
                .and_then(|previous| remove_unused(&credentials, &sources, &previous).err())
                .map(|_| "Datasource removed; a private credential could not be removed".into());
            Ok(Saved { sources, warning })
        })
        .await
    }
    pub async fn shutdown(&self) -> Result<()> {
        self.executor.shutdown().await;
        self.metadata.shutdown().await;
        self.connections
            .shutdown()
            .await
            .map_err(|e| DomainError::Unsupported(e.to_string()))
    }
}
fn remove_unused(
    credentials: &CredentialStore,
    sources: &[DataSourceDefinition],
    previous: &DataSourceDefinition,
) -> Result<()> {
    for reference in previous
        .credential_reference
        .iter()
        .chain(previous.credential_properties.values())
    {
        if !sources.iter().any(|ds| {
            ds.credential_reference.as_ref() == Some(reference)
                || ds.credential_properties.values().any(|r| r == reference)
        }) {
            credentials.remove(Some(reference))?;
        }
    }
    Ok(())
}

pub(super) fn connection_error(error: kode_database::connections::Error) -> DomainError {
    if let Some(sql) = error.sql {
        DomainError::Sql {
            sqlstate: sql.sql_state.clone().filter(|state| {
                state.len() <= 16 && state.chars().all(|ch| ch.is_ascii_alphanumeric())
            }),
            vendor_code: sql.vendor_code,
            message: crate::panels::database::results::sql_error_details(&sql),
        }
    } else {
        DomainError::Unsupported(error.to_string())
    }
}
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| DomainError::Unsupported("Database configuration worker failed".into()))?
}

impl super::AppRuntime {
    pub fn database_configuration_failed(&mut self, error: DomainError) {
        let message = error.to_string();
        self.state.database.tree.notice = message.clone();
        self.database_error = Some(message);
    }
    pub fn database_services(&self) -> Option<Arc<Services>> {
        self.database.clone()
    }
    pub fn configure_database(&mut self, config: DatabaseConfiguration) -> Result<()> {
        if self.database.is_some() {
            return Err(DomainError::Unsupported(
                "Database services are already configured".into(),
            ));
        }
        self.state
            .database
            .configure(config.registry.clone(), config.urls.clone());
        self.database = Some(Arc::new(Services::new(config)?));
        self.database_error = None;
        Ok(())
    }
    pub(super) fn rebind_database(&mut self) -> Result<()> {
        if let Some(previous) = self.database.take() {
            let config = previous.config.clone();
            self.state.database.initialized = false;
            self.configure_database(config)?;
        }
        Ok(())
    }
    pub(super) fn pump_database(&mut self) -> Result<()> {
        if self.state.closing {
            return Ok(());
        }
        if let Some(dialog) = &mut self.state.database.java_runtime
            && dialog.poll()
        {
            self.notifications.repaint();
        }
        if let Some(database) = &self.database {
            let required = database
                .connections
                .runtime()
                .last_error()
                .is_some_and(|error| error.requires_java_runtime());
            if self.state.database.java_required != required {
                self.notifications.repaint();
            }
            self.state.database.java_required = required;
            if let Some(form) = &mut self.state.database.form {
                form.java_required = required;
            }
            let sources: Vec<_> = self
                .state
                .database
                .tree
                .sources
                .iter()
                .map(|ds| ds.id.clone())
                .collect();
            for source in sources {
                let snapshot = database.connections.snapshot(&source);
                if self.state.sql.connections.get(&source).is_none_or(|old| {
                    old.revision != snapshot.revision
                        || old.phase != snapshot.phase
                        || old.session.as_ref().map(|session| {
                            (
                                &session.connection_id,
                                &session.helper_epoch,
                                session.state_revision,
                                session.context_revision,
                                session.auto_commit,
                                session.transaction_usable,
                                session.context_unknown,
                            )
                        }) != snapshot.session.as_ref().map(|session| {
                            (
                                &session.connection_id,
                                &session.helper_epoch,
                                session.state_revision,
                                session.context_revision,
                                session.auto_commit,
                                session.transaction_usable,
                                session.context_unknown,
                            )
                        })
                }) {
                    self.notifications.repaint();
                }
                if let Some(session) = &snapshot.session
                    && self
                        .state
                        .sql
                        .manager
                        .set_auto_commit(&source, session.auto_commit)
                {
                    self.state.changed()?;
                }
                self.state
                    .sql
                    .connections
                    .insert(source.clone(), snapshot.clone());
                self.state.database.tree.set_status(&source, snapshot);
            }
        }
        self.state.database.drain_tree(&mut self.state.focus)
    }
    pub(super) fn database_work(
        &mut self,
        work: crate::panels::database::controller::Work,
    ) -> Result<()> {
        use crate::panels::database::controller::{Operation, Report};
        use kode_platform::jobs::{Delivery, OwnerId};
        let context = work.context();
        let tag = work.tag;
        if self.state.closing
            && !matches!(
                context,
                crate::panels::database::controller::Context::Save
                    | crate::panels::database::controller::Context::Delete
            )
        {
            work.cancel.cancel();
            self.state.database.complete(
                tag,
                context,
                Err("Database operation cancelled while closing the workspace".into()),
                false,
                &mut self.state.focus,
            );
            return Ok(());
        }
        let Some(database) = self.database.clone() else {
            self.state.database.complete(
                tag,
                context,
                Err(self
                    .database_error
                    .clone()
                    .unwrap_or_else(|| "Database services are not configured".into())),
                false,
                &mut self.state.focus,
            );
            return Ok(());
        };
        let submitted=self.jobs.submit(OwnerId(self.state.database.tree.id.0),tag,Delivery::Critical,move|scheduler_cancel|async move{
            let cancel=work.cancel;let on_cancel=cancel.clone();
            let future=async {
                Ok(super::Output::Database(match work.operation{
                    Operation::Initialize=>Report::Sources{sources:database.sources().await?,warning:None},
                    Operation::Restart=>{database.connections.runtime().restart(&cancel).await.map_err(|error|DomainError::Unsupported(error.to_string()))?;Report::Restarted},
                    Operation::Save(draft)=>{let saved=database.save(draft,&cancel).await?;Report::Sources{sources:saved.sources,warning:saved.warning}},
                    Operation::Delete(source)=>{let saved=database.remove(source,&cancel).await?;Report::Sources{sources:saved.sources,warning:saved.warning}},
                    Operation::Connect(definition)=>{
                        if database.connections.snapshot(&definition.id).phase==kode_database::connections::Phase::Connected{database.connections.close(&definition.id,&cancel).await.map_err(|error|DomainError::Unsupported(error.to_string()))?;Report::Closed}
                        else{Report::Connected(database.connect(definition,&cancel).await?)}
                    },
                    Operation::Close(source)=>{database.connections.close(&source,&cancel).await.map_err(|error|DomainError::Unsupported(error.to_string()))?;Report::Closed},
                    Operation::Test(draft)=>{let(result,warning)=database.test(draft,&cancel).await?;Report::Test{result,warning}},
                    Operation::Metadata(load)=>{if load.refresh{database.metadata.refresh_all(load.key.source());}Report::Metadata(database.metadata.introspect(load.key.source(),load.request,load.refresh,&cancel).await.map_err(|error|connection_error(error.into()))?)},
                    Operation::Resolve{spec,progress}=>Report::Installed(database.installer.resolve(&spec,progress,&cancel).await),
                    Operation::JavaRuntime{progress}=>Report::JavaRuntime(database.install_java_runtime(&cancel,progress).await?),
                    Operation::Recents(driver)=>Report::Recents(database.recents(&driver).await?),
                }))
            };
            tokio::pin!(future);
            tokio::select!{biased;_=scheduler_cancel.cancelled()=>{on_cancel.cancel();future.await},result=&mut future=>result}
        });
        match submitted {
            Ok(()) => {
                self.contexts.insert(tag, super::Context::Database(context));
                Ok(())
            }
            Err(error) => {
                self.state.database.complete(
                    tag,
                    context,
                    Err(error.to_string()),
                    false,
                    &mut self.state.focus,
                );
                Ok(())
            }
        }
    }
}
