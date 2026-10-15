//! Workspace-owned server registry. Ordered language precedence matches the
//! source service; failed starts never appear as running processes.
use crate::{
    catalog::{ServerEntry, validate_command, validate_id},
    client::{ClientOptions, ClientState, LspClient},
    discovery::ResolvedCommand,
};
use kode_core::{DomainError, Result};
use kode_platform::{
    jobs::{CancellationToken, Notifications},
    process::{ProcessFactory, SystemProcessFactory},
};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};
const MAX_SERVERS: usize = 16;
struct Server {
    entry: ServerEntry,
    client: LspClient,
}
pub struct LspService {
    options: ClientOptions,
    factory: Arc<dyn ProcessFactory>,
    servers: RwLock<Vec<Server>>,
    errors: Mutex<VecDeque<(String, String)>>,
    operations: tokio::sync::Mutex<()>,
    stop: CancellationToken,
    revision: Arc<AtomicU64>,
    notifications: Option<Notifications>,
}
impl LspService {
    pub fn new(root: PathBuf, notifications: Option<Notifications>) -> Self {
        Self::with_factory(
            ClientOptions::new(root),
            Arc::new(SystemProcessFactory),
            notifications,
        )
    }
    pub fn with_factory(
        options: ClientOptions,
        factory: Arc<dyn ProcessFactory>,
        notifications: Option<Notifications>,
    ) -> Self {
        Self {
            options,
            factory,
            servers: RwLock::new(vec![]),
            errors: Mutex::new(VecDeque::new()),
            operations: tokio::sync::Mutex::new(()),
            stop: CancellationToken::new(),
            revision: Arc::new(AtomicU64::new(0)),
            notifications,
        }
    }
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }
    fn changed(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
        if let Some(notifications) = &self.notifications {
            notifications.repaint();
        }
    }
    fn error(&self, id: &str, error: Option<String>) {
        let mut errors = self.errors.lock().expect("LSP service error lock");
        errors.retain(|(key, _)| key != id);
        if let Some(error) = error {
            errors.push_back((id.into(), error));
            if errors.len() > 64 {
                errors.pop_front();
            }
        }
        drop(errors);
        self.changed();
    }
    pub fn last_error(&self, id: &str) -> Option<String> {
        self.servers
            .read()
            .expect("LSP registry lock")
            .iter()
            .find(|server| server.entry.id == id)
            .and_then(|server| server.client.last_error())
            .or_else(|| {
                self.errors
                    .lock()
                    .expect("LSP service error lock")
                    .iter()
                    .find(|(key, _)| key == id)
                    .map(|(_, error)| error.clone())
            })
    }
    pub fn running(&self, id: &str) -> bool {
        self.client(id).is_some()
    }
    pub fn has_running_servers(&self) -> bool {
        self.servers
            .read()
            .expect("LSP registry lock")
            .iter()
            .any(|server| server.client.state() == ClientState::Running)
    }
    pub fn client(&self, id: &str) -> Option<LspClient> {
        self.servers
            .read()
            .expect("LSP registry lock")
            .iter()
            .find(|server| server.entry.id == id && server.client.state() == ClientState::Running)
            .map(|server| server.client.clone())
    }
    pub fn for_language(&self, language: &str) -> Option<LspClient> {
        self.servers
            .read()
            .expect("LSP registry lock")
            .iter()
            .find(|server| {
                server.client.state() == ClientState::Running
                    && server
                        .entry
                        .languages
                        .iter()
                        .any(|candidate| candidate.eq_ignore_ascii_case(language))
            })
            .map(|server| server.client.clone())
    }
    pub fn take_events(&self) -> Vec<(String, crate::client::ServerEvent)> {
        self.servers
            .read()
            .expect("LSP registry lock")
            .iter()
            .flat_map(|server| {
                server
                    .client
                    .take_events()
                    .into_iter()
                    .map(|event| (server.entry.id.clone(), event))
            })
            .collect()
    }
    pub async fn start(
        &self,
        entry: ServerEntry,
        command: ResolvedCommand,
        cancel: &CancellationToken,
    ) -> Result<LspClient> {
        validate_id(&entry.id)?;
        validate_command(&command.args)?;
        let _guard = tokio::select! {biased;_=self.stop.cancelled()=>return Err(DomainError::Cancelled),_=cancel.cancelled()=>return Err(DomainError::Cancelled),guard=self.operations.lock()=>guard};
        if let Some(client) = self.client(&entry.id) {
            return Ok(client);
        }
        let previous = {
            let mut servers = self.servers.write().expect("LSP registry lock");
            servers
                .iter()
                .position(|server| server.entry.id == entry.id)
                .map(|index| servers.remove(index))
        };
        if let Some(previous) = previous {
            previous.client.shutdown().await?;
        }
        if self.servers.read().expect("LSP registry lock").len() >= MAX_SERVERS {
            return Err(DomainError::QueueFull);
        }
        let mut options = self.options.clone();
        if let Some(configuration) = entry.extra.get("configuration") {
            options.configuration = configuration.clone();
        }
        if let Some(initialization) = entry.extra.get("initializationOptions") {
            options.initialization_options = Some(initialization.clone());
        }
        let revision = self.revision.clone();
        let notifications = self.notifications.clone();
        let callback = options.on_state.clone();
        options.on_state = Some(Arc::new(move |state| {
            revision.fetch_add(1, Ordering::AcqRel);
            if let Some(notifications) = &notifications {
                notifications.repaint();
            }
            if let Some(callback) = &callback {
                callback(state);
            }
        }));
        let startup_token = self.stop.child_token();
        let future = LspClient::start(
            command.process_spec(),
            options,
            self.factory.clone(),
            &startup_token,
        );
        tokio::pin!(future);
        let result = tokio::select! {
            biased;
            _=self.stop.cancelled()=>{startup_token.cancel();if let Ok(client)=future.await {client.shutdown().await?;}Err(DomainError::Cancelled)},
            _=cancel.cancelled()=>{startup_token.cancel();if let Ok(client)=future.await {client.shutdown().await?;}Err(DomainError::Cancelled)},
            result=&mut future=>result,
        };
        match result {
            Ok(client) => {
                self.servers
                    .write()
                    .expect("LSP registry lock")
                    .push(Server {
                        entry: entry.clone(),
                        client: client.clone(),
                    });
                self.error(&entry.id, None);
                Ok(client)
            }
            Err(error) => {
                self.error(&entry.id, Some(error.to_string()));
                Err(error)
            }
        }
    }
    pub async fn stop(&self, id: &str) -> Result<()> {
        let _guard = self.operations.lock().await;
        let server = {
            let mut servers = self.servers.write().expect("LSP registry lock");
            servers
                .iter()
                .position(|server| server.entry.id == id)
                .map(|index| servers.remove(index))
        };
        if let Some(server) = server {
            let result = server.client.shutdown().await;
            self.error(id, result.as_ref().err().map(ToString::to_string));
            result
        } else {
            Err(DomainError::Unsupported(format!("{id} is not running")))
        }
    }
    pub async fn restart(
        &self,
        entry: ServerEntry,
        command: ResolvedCommand,
        cancel: &CancellationToken,
    ) -> Result<LspClient> {
        if self
            .servers
            .read()
            .expect("LSP registry lock")
            .iter()
            .any(|server| server.entry.id == entry.id)
        {
            self.stop(&entry.id).await?;
        }
        self.start(entry, command, cancel).await
    }
    pub async fn shutdown(&self) -> Result<()> {
        self.stop.cancel();
        let _guard = self.operations.lock().await;
        let servers = std::mem::take(&mut *self.servers.write().expect("LSP registry lock"));
        let results =
            futures_util::future::join_all(servers.iter().map(|server| server.client.shutdown()))
                .await;
        self.changed();
        let mut failure = None;
        for (server, result) in servers.into_iter().zip(results) {
            if let Err(error) = result {
                self.error(&server.entry.id, Some(error.to_string()));
                if failure.is_none() {
                    failure = Some(error);
                }
            }
        }
        if let Some(error) = failure {
            Err(error)
        } else {
            Ok(())
        }
    }
}
impl Drop for LspService {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Ok(servers) = self.servers.read() {
            for server in servers.iter() {
                server.client.cancel();
            }
        }
    }
}
