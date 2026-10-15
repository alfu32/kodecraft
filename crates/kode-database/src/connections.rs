//! Explicit, lazy JDBC connection ownership. UI reads only these snapshots;
//! credentials, Java startup and JDBC calls run on tracked background tasks.
use crate::{
    bridge::{
        client::{BridgeError, Client, Failure},
        runtime::Runtime,
        wire::SqlError,
    },
    credentials::{CredentialStore, Secret},
    drivers::Installer,
    model::{DataSourceDefinition, DatabaseCapabilities},
};
use kode_platform::jobs::CancellationToken;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::Semaphore;
use tokio_util::task::TaskTracker;
mod operations;
static NEXT_ATTEMPT: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Disconnected,
    Connecting,
    Connected,
    Closing,
    Error,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub connection_id: String,
    pub data_source_id: String,
    #[serde(skip)]
    pub helper_epoch: String,
    pub capabilities: DatabaseCapabilities,
    pub product_name: String,
    pub product_version: String,
    pub driver_name: String,
    pub driver_version: String,
    pub auto_commit: bool,
    pub read_only: bool,
    pub catalog: Option<String>,
    pub schema: Option<String>,
    pub context_revision: u64,
    pub state_revision: u64,
    pub transaction_usable: bool,
}
impl std::fmt::Debug for SessionInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionInfo")
            .field("connection_id", &self.connection_id)
            .field("helper_epoch", &self.helper_epoch)
            .field("context_revision", &self.context_revision)
            .field("auto_commit", &self.auto_commit)
            .field("transaction_usable", &self.transaction_usable)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub success: bool,
    pub product_name: String,
    pub product_version: String,
    pub driver_name: String,
    pub driver_version: String,
    pub latency_ms: u64,
    pub capabilities: DatabaseCapabilities,
}
impl std::fmt::Debug for TestResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestResult")
            .field("success", &self.success)
            .field("latency_ms", &self.latency_ms)
            .finish_non_exhaustive()
    }
}
#[derive(Debug, Clone)]
pub struct Error {
    pub bridge: BridgeError,
    pub sql: Option<Box<SqlError>>,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.bridge.fmt(f)
    }
}
impl std::error::Error for Error {}
impl From<BridgeError> for Error {
    fn from(mut bridge: BridgeError) -> Self {
        let sql = bridge
            .remote_data()
            .and_then(|data| data.get("sqlError"))
            .and_then(|sql| serde_json::from_value(sql.clone()).ok());
        bridge.message = match bridge.remote_code {
            Some(-32021) => "Connection ID is stale; connect explicitly",
            Some(-32022) => "Datasource definition changed; disconnect before connecting again",
            Some(-32023) => {
                "Driver cannot apply this connection or transaction operation; inspect the current context"
            }
            Some(-32024) => "A connection could not be closed safely; restart the JDBC helper",
            _ => bridge.message,
        };
        Self { bridge, sql }
    }
}
pub type Result<T> = std::result::Result<T, Error>;
fn error(kind: Failure, message: &'static str) -> Error {
    BridgeError::new(kind, message).into()
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub phase: Phase,
    pub session: Option<SessionInfo>,
    pub error: Option<Error>,
    pub unknown_outcome: bool,
    pub revision: u64,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            phase: Phase::Disconnected,
            session: None,
            error: None,
            unknown_outcome: false,
            revision: 0,
        }
    }
}
struct Entry {
    snapshot: Snapshot,
    definition: Option<DataSourceDefinition>,
    attempt: String,
    client: Option<Client>,
}
struct Shared {
    runtime: Runtime,
    installer: Installer,
    credentials: CredentialStore,
    entries: Mutex<BTreeMap<String, Entry>>,
    epoch: Mutex<Option<String>>,
    stop: CancellationToken,
    credential_slots: Arc<Semaphore>,
}
struct Owner {
    shared: Arc<Shared>,
    stop: CancellationToken,
    tasks: TaskTracker,
    slots: Arc<Semaphore>,
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.stop.cancel();
        self.tasks.close();
    }
}
#[derive(Clone)]
pub struct Connections {
    owner: Arc<Owner>,
}

/// Omitted fields stay unchanged. Explicit null catalog/schema restores the
/// defaults captured when this connection logged in.
#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_commit: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
}
impl Connections {
    pub fn new(installer: Installer, credentials: CredentialStore) -> Self {
        let stop = CancellationToken::new();
        Self {
            owner: Arc::new(Owner {
                shared: Arc::new(Shared {
                    runtime: installer.runtime().clone(),
                    installer,
                    credentials,
                    entries: Mutex::new(BTreeMap::new()),
                    epoch: Mutex::new(None),
                    stop: stop.clone(),
                    credential_slots: Arc::new(Semaphore::new(4)),
                }),
                stop,
                tasks: TaskTracker::new(),
                slots: Arc::new(Semaphore::new(64)),
            }),
        }
    }
    pub fn snapshot(&self, source: &str) -> Snapshot {
        self.owner
            .shared
            .entries
            .lock()
            .expect("connection state")
            .get(source)
            .map(|entry| entry.snapshot.clone())
            .unwrap_or_default()
    }
    /// Reference for execution/metadata clients. This never starts Java and
    /// never returns a connection belonging to a failed or closing epoch.
    pub fn session(&self, source: &str) -> Result<SessionInfo> {
        let entries = self.owner.shared.entries.lock().expect("connection state");
        let entry = entries
            .get(source)
            .filter(|entry| entry.snapshot.phase == Phase::Connected)
            .ok_or_else(|| {
                error(
                    Failure::Unavailable,
                    "Datasource is disconnected; connect explicitly",
                )
            })?;
        if entry
            .client
            .as_ref()
            .is_none_or(|client| client.state() != crate::bridge::client::State::Running)
        {
            return Err(error(
                Failure::Transport,
                "JDBC helper stopped; reconnect explicitly",
            ));
        }
        entry
            .snapshot
            .session
            .clone()
            .ok_or_else(|| error(Failure::Protocol, "Connected datasource has no session"))
    }
    pub fn runtime(&self) -> &Runtime {
        &self.owner.shared.runtime
    }
    async fn run<T: Send + 'static>(
        &self,
        cancel: &CancellationToken,
        work: impl FnOnce(
            Arc<Shared>,
            CancellationToken,
            TaskTracker,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T>> + Send>>
        + Send
        + 'static,
    ) -> Result<T> {
        if self.owner.stop.is_cancelled() || cancel.is_cancelled() {
            return Err(error(Failure::Cancelled, "Connection operation cancelled"));
        }
        let slot = self
            .owner
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| error(Failure::Busy, "Connection operation capacity exhausted"))?;
        let token = self.owner.stop.child_token();
        struct Cancel(CancellationToken);
        impl Drop for Cancel {
            fn drop(&mut self) {
                self.0.cancel();
            }
        }
        let guard = Cancel(token.clone());
        let shared = self.owner.shared.clone();
        let tasks = self.owner.tasks.clone();
        let mut task = self.owner.tasks.spawn(async move {
            let _slot = slot;
            work(shared, token, tasks).await
        });
        let result = tokio::select! {biased;_=cancel.cancelled()=>{guard.0.cancel();task.await},result=&mut task=>result};
        result.map_err(|_| error(Failure::Transport, "Connection worker failed"))?
    }
    pub async fn open(
        &self,
        definition: DataSourceDefinition,
        password: Option<Secret>,
        cancel: &CancellationToken,
    ) -> Result<SessionInfo> {
        self.run(cancel, move |shared, token, tasks| {
            Box::pin(shared.open(definition, password, token, tasks))
        })
        .await
    }
    pub async fn test(
        &self,
        definition: DataSourceDefinition,
        password: Option<Secret>,
        cancel: &CancellationToken,
    ) -> Result<TestResult> {
        self.run(cancel, move |shared, token, tasks| {
            Box::pin(shared.test(definition, password, token, tasks))
        })
        .await
    }
    pub async fn close(&self, source: &str, cancel: &CancellationToken) -> Result<()> {
        let source = source.to_owned();
        self.run(cancel, move |shared, token, _| {
            Box::pin(async move { shared.close(&source, token).await })
        })
        .await
    }
    pub async fn set_context(
        &self,
        source: &str,
        context: Context,
        cancel: &CancellationToken,
    ) -> Result<SessionInfo> {
        let source = source.to_owned();
        self.run(cancel, move |shared, token, _| {
            Box::pin(async move {
                shared
                    .command(
                        &source,
                        "connection.setContext",
                        serde_json::to_value(context).expect("context serializes"),
                        token,
                    )
                    .await
            })
        })
        .await
    }
    pub async fn commit(&self, source: &str, cancel: &CancellationToken) -> Result<SessionInfo> {
        self.transaction(source, false, cancel).await
    }
    pub async fn rollback(&self, source: &str, cancel: &CancellationToken) -> Result<SessionInfo> {
        self.transaction(source, true, cancel).await
    }
    async fn transaction(
        &self,
        source: &str,
        rollback: bool,
        cancel: &CancellationToken,
    ) -> Result<SessionInfo> {
        let source = source.to_owned();
        self.run(cancel, move |shared, token, _| {
            Box::pin(async move {
                shared
                    .command(
                        &source,
                        if rollback {
                            "connection.rollback"
                        } else {
                            "connection.commit"
                        },
                        json!({}),
                        token,
                    )
                    .await
            })
        })
        .await
    }
    /// Saving even an equal definition can change its private credential value.
    /// D07 must await this before replacing the saved definition.
    pub async fn definition_changed(&self, source: &str, cancel: &CancellationToken) -> Result<()> {
        self.close(source, cancel).await
    }
    pub async fn shutdown(&self) -> Result<()> {
        self.owner.stop.cancel();
        self.owner.tasks.close();
        self.owner.tasks.wait().await;
        self.owner.shared.installer.stop_progress().await;
        self.owner
            .shared
            .runtime
            .shutdown()
            .await
            .map_err(Into::into)
    }
}

impl Shared {
    fn attempt(&self) -> Result<String> {
        NEXT_ATTEMPT
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map(|id| format!("connection-{id}"))
            .map_err(|_| error(Failure::Busy, "Connection attempt IDs exhausted"))
    }
    async fn client(
        self: &Arc<Self>,
        cancel: &CancellationToken,
        tasks: &TaskTracker,
    ) -> Result<Client> {
        let client = self.runtime.get(cancel).await?;
        let epoch = client
            .info()
            .ok_or_else(|| error(Failure::Protocol, "JDBC hello is missing"))?
            .helper_epoch
            .clone();
        let mut current = self.epoch.lock().expect("connection epoch");
        if current.as_ref() != Some(&epoch) {
            *current = Some(epoch.clone());
            let weak = Arc::downgrade(self);
            let watched = client.clone();
            let stop = self.stop.clone();
            tasks.spawn(async move {
                tokio::select! {biased; _=stop.cancelled()=>return, _=watched.wait_stopped()=>{}}
                if let Some(shared)=weak.upgrade() {
                    let mut entries=shared.entries.lock().expect("connection state");
                    for entry in entries.values_mut() {
                        if entry.client.as_ref().and_then(Client::info).is_some_and(|info|info.helper_epoch==epoch) {
                            entry.snapshot.phase=Phase::Error;
                            entry.snapshot.session=None;
                            entry.client=None;
                            entry.snapshot.unknown_outcome=true;
                            entry.snapshot.error=Some(error(Failure::Transport,"JDBC helper stopped; connection outcomes are unknown; reconnect explicitly"));
                            entry.snapshot.revision=entry.snapshot.revision.saturating_add(1);
                        }
                    }
                }
            });
        }
        Ok(client)
    }
    async fn params(
        self: &Arc<Self>,
        definition: &DataSourceDefinition,
        password: Option<Secret>,
        attempt: &str,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        serde_json::to_value(definition).map_err(|_| {
            error(
                Failure::Protocol,
                "Datasource contains invalid or unsafe connection settings",
            )
        })?;
        if definition.id.is_empty()
            || definition.id.len() > 128
            || definition.id.chars().any(char::is_control)
            || definition.properties.len() + definition.credential_properties.len() > 254
        {
            return Err(error(
                Failure::Protocol,
                "Datasource exceeds connection parameter bounds",
            ));
        }
        let inspected = self
            .installer
            .inspect_spec(&definition.driver, cancel)
            .await?;
        let mut references: BTreeSet<String> =
            definition.credential_properties.values().cloned().collect();
        if password.is_none()
            && let Some(reference) = definition
                .credential_reference
                .as_ref()
                .filter(|value| !value.trim().is_empty())
        {
            references.insert(reference.clone());
        }
        let values = if references.is_empty() {
            BTreeMap::new()
        } else {
            let slot = self
                .credential_slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| error(Failure::Busy, "Credential reader capacity exhausted"))?;
            let store = self.credentials.clone();
            tokio::task::spawn_blocking(move || {
                let _slot = slot;
                store.get_many(&references)
            })
            .await
            .map_err(|_| error(Failure::Unavailable, "Credential reader failed"))?
            .map_err(|_| {
                error(
                    Failure::Unavailable,
                    "Cannot read private credentials; check the credential store",
                )
            })?
        };
        if cancel.is_cancelled() {
            return Err(error(
                Failure::Cancelled,
                "Connection preparation cancelled",
            ));
        }
        let mut properties = definition.properties.clone();
        for (key, reference) in &definition.credential_properties {
            properties.insert(
                key.clone(),
                values
                    .get(reference)
                    .ok_or_else(|| {
                        error(
                            Failure::Unavailable,
                            "A required credential reference is missing",
                        )
                    })?
                    .expose()
                    .into(),
            );
        }
        if !definition.username.trim().is_empty() {
            properties.insert("user".into(), definition.username.clone());
        }
        let password = password.as_ref().or_else(|| {
            definition
                .credential_reference
                .as_ref()
                .and_then(|reference| values.get(reference))
        });
        if password.is_none()
            && definition
                .credential_reference
                .as_ref()
                .is_some_and(|reference| !reference.trim().is_empty())
        {
            return Err(error(
                Failure::Unavailable,
                "The datasource password reference is missing",
            ));
        }
        if let Some(password) = password {
            properties.insert("password".into(), password.expose().into());
        }
        // The inspect result belongs to the current helper; an explicit restart
        // during preparation must not send its stale loader into the new epoch.
        if self
            .runtime
            .get(cancel)
            .await?
            .info()
            .is_none_or(|info| info.helper_epoch != inspected.helper_epoch)
        {
            return Err(error(
                Failure::Transport,
                "JDBC helper changed during connection preparation; connect explicitly again",
            ));
        }
        Ok(
            json!({"dataSourceId":definition.id,"attemptId":attempt,"loaderId":inspected.loader_id,"driverClass":inspected.selected_driver_class,"jdbcUrl":definition.jdbc_url,"properties":properties,"catalog":definition.default_catalog,"schema":definition.default_schema,"autoCommit":definition.auto_commit,"readOnly":definition.read_only}),
        )
    }
}
