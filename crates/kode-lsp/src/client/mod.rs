//! Bounded asynchronous stdio client. All UI publication stays in kode-tui.
mod rpc;
mod tasks;
use crate::{documents::*, transport::protocol};
use kode_core::{DocumentOrigin, DomainError, Result, document::TextSnapshot};
use kode_platform::{
    jobs::CancellationToken,
    process::{ManagedChild, ProcessFactory, ProcessSpec},
};
use lsp_types::{
    ClientInfo as ProtocolClientInfo, CompletionResponse, GotoDefinitionResponse, InitializeParams,
    InitializeResult, Location, Position, ServerCapabilities, Uri,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock, RwLock,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Semaphore, mpsc, oneshot};
use tokio_util::task::TaskTracker;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ClientState {
    Starting = 0,
    Running = 1,
    Stopping = 2,
    Stopped = 3,
    Failed = 4,
}
#[derive(Clone)]
pub struct ClientOptions {
    pub root: PathBuf,
    pub initialization_options: Option<Value>,
    pub configuration: Value,
    pub startup_timeout: Duration,
    pub request_timeout: Duration,
    pub write_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub exit_timeout: Duration,
    pub on_state: Option<Arc<dyn Fn(ClientState) + Send + Sync>>,
}
impl ClientOptions {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            initialization_options: None,
            configuration: json!({}),
            startup_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_millis(800),
            write_timeout: Duration::from_secs(2),
            shutdown_timeout: Duration::from_secs(1),
            exit_timeout: Duration::from_millis(500),
            on_state: None,
        }
    }
}
#[derive(Debug, Clone)]
pub struct ClientInfo {
    pub capabilities: ServerCapabilities,
    pub encoding: PositionEncoding,
    pub synchronization: SyncMode,
    pub server_name: Option<String>,
    pub server_version: Option<String>,
}
#[derive(Debug, Clone)]
pub struct ServerEvent {
    pub method: String,
    pub params: Value,
}
struct Shared {
    program: String,
    pid: u32,
    root: Uri,
    options: ClientOptions,
    state: AtomicU8,
    error: Mutex<Option<String>>,
    stop: CancellationToken,
    next_id: AtomicU64,
    pending: Mutex<BTreeMap<u64, oneshot::Sender<Result<Value>>>>,
    slots: Arc<Semaphore>,
    writer: mpsc::Sender<rpc::Frame>,
    budget: Arc<Semaphore>,
    info: OnceLock<Arc<ClientInfo>>,
    documents: RwLock<BTreeMap<String, (DocumentStamp, PositionMapper)>>,
    events: Mutex<std::collections::VecDeque<ServerEvent>>,
    stderr: kode_platform::process::TailBuffer,
    supervisor_error: Mutex<Option<String>>,
}
impl Shared {
    fn state(&self) -> ClientState {
        match self.state.load(Ordering::Acquire) {
            0 => ClientState::Starting,
            1 => ClientState::Running,
            2 => ClientState::Stopping,
            3 => ClientState::Stopped,
            _ => ClientState::Failed,
        }
    }
    fn set_state(&self, state: ClientState) {
        self.state.store(state as u8, Ordering::Release);
        if let Some(callback) = &self.options.on_state {
            callback(state);
        }
    }
    fn error(&self, message: impl Into<String>) -> DomainError {
        DomainError::Process {
            program: self.program.clone(),
            message: message.into(),
        }
    }
    fn fail(&self, message: impl Into<String>) {
        let message = message.into();
        if !matches!(self.state(), ClientState::Stopping | ClientState::Stopped) {
            *self.error.lock().expect("LSP error lock") = Some(message.clone());
            self.set_state(ClientState::Failed);
        }
        self.cancel_pending(&message);
        self.stop.cancel();
    }
    fn cancel_pending(&self, message: &str) {
        let pending = std::mem::take(&mut *self.pending.lock().expect("LSP pending lock"));
        for (_, reply) in pending {
            let _ = reply.send(Err(self.error(message)));
        }
    }
    fn event(&self, method: &str, params: Value) {
        let mut events = self.events.lock().expect("LSP events lock");
        if events.len() == 32 {
            events.pop_front();
        }
        events.push_back(ServerEvent {
            method: method.into(),
            params,
        });
    }
}
struct Owner {
    shared: Arc<Shared>,
    documents: mpsc::Sender<tasks::DocumentWork>,
    tracker: TaskTracker,
    shutdown: tokio::sync::Mutex<()>,
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.shared.stop.cancel();
        self.tracker.close();
    }
}
#[derive(Clone)]
pub struct LspClient {
    owner: Arc<Owner>,
}
impl LspClient {
    pub async fn start(
        spec: ProcessSpec,
        options: ClientOptions,
        factory: Arc<dyn ProcessFactory>,
        cancel: &CancellationToken,
    ) -> Result<Self> {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        let root = file_uri(&options.root)?;
        let mut child = ManagedChild::spawn(spec.clone(), factory.as_ref())?;
        let (Some(stdin), Some(stdout)) = (child.take_stdin(), child.take_stdout()) else {
            child.terminate_and_reap().await?;
            return Err(protocol("language server requires piped stdin/stdout"));
        };
        let pid = match child.id() {
            Some(pid) => pid,
            None => {
                child.terminate_and_reap().await?;
                return Err(protocol("language server exited before supervision"));
            }
        };
        let (writer, write_rx) = mpsc::channel(32);
        let (documents, doc_rx) = mpsc::channel(32);
        let shared = Arc::new(Shared {
            program: spec.program.display().to_string(),
            pid,
            root,
            options,
            state: AtomicU8::new(ClientState::Starting as u8),
            error: Mutex::new(None),
            stop: CancellationToken::new(),
            next_id: AtomicU64::new(1),
            pending: Mutex::new(BTreeMap::new()),
            slots: Arc::new(Semaphore::new(63)),
            writer,
            budget: Arc::new(Semaphore::new(32 * 1024 * 1024)),
            info: OnceLock::new(),
            documents: RwLock::new(BTreeMap::new()),
            events: Mutex::new(std::collections::VecDeque::new()),
            stderr: child.stderr_tail(),
            supervisor_error: Mutex::new(None),
        });
        let tracker = TaskTracker::new();
        tasks::spawn(
            &tracker,
            shared.clone(),
            tasks::write(shared.clone(), stdin, write_rx),
        );
        tasks::spawn(
            &tracker,
            shared.clone(),
            tasks::read(shared.clone(), stdout),
        );
        tasks::spawn(
            &tracker,
            shared.clone(),
            tasks::documents(shared.clone(), doc_rx),
        );
        tasks::spawn(
            &tracker,
            shared.clone(),
            tasks::supervise(shared.clone(), child),
        );
        let client = Self {
            owner: Arc::new(Owner {
                shared,
                documents,
                tracker,
                shutdown: tokio::sync::Mutex::new(()),
            }),
        };
        let initialized = client.initialize(cancel).await;
        if let Err(error) = initialized {
            client.force_shutdown().await;
            if matches!(error, DomainError::Cancelled) {
                return Err(error);
            }
            let stderr = client.stderr();
            if !stderr.is_empty() {
                return Err(client.owner.shared.error(format!(
                    "{error}; stderr: {}",
                    stderr.chars().take(2048).collect::<String>()
                )));
            }
            return Err(error);
        }
        Ok(client)
    }
    async fn initialize(&self, cancel: &CancellationToken) -> Result<()> {
        let shared = &self.owner.shared;
        #[allow(deprecated)]
        let params = InitializeParams {
            process_id: Some(std::process::id()),
            root_uri: Some(shared.root.clone()),
            workspace_folders: Some(vec![lsp_types::WorkspaceFolder {
                uri: shared.root.clone(),
                name: shared
                    .options
                    .root
                    .file_name()
                    .map_or("workspace".into(), |name| {
                        name.to_string_lossy().into_owned()
                    }),
            }]),
            client_info: Some(ProtocolClientInfo {
                name: "kode-rs".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            initialization_options: shared.options.initialization_options.clone(),
            capabilities: lsp_types::ClientCapabilities {
                general: Some(lsp_types::GeneralClientCapabilities {
                    position_encodings: Some(vec![
                        lsp_types::PositionEncodingKind::UTF16,
                        lsp_types::PositionEncodingKind::UTF8,
                        lsp_types::PositionEncodingKind::UTF32,
                    ]),
                    ..Default::default()
                }),
                workspace: Some(lsp_types::WorkspaceClientCapabilities {
                    configuration: Some(true),
                    workspace_folders: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let value = self
            .request_inner(
                "initialize",
                serde_json::to_value(params).map_err(|e| protocol(e.to_string()))?,
                shared.options.startup_timeout,
                cancel,
                true,
            )
            .await?;
        let initialized: InitializeResult = serde_json::from_value(value)
            .map_err(|e| protocol(format!("initialize result: {e}")))?;
        let encoding =
            PositionEncoding::negotiate(initialized.capabilities.position_encoding.as_ref())?;
        let synchronization =
            SyncMode::negotiated(initialized.capabilities.text_document_sync.as_ref());
        shared
            .info
            .set(Arc::new(ClientInfo {
                capabilities: initialized.capabilities,
                encoding,
                synchronization,
                server_name: initialized
                    .server_info
                    .as_ref()
                    .map(|info| info.name.clone()),
                server_version: initialized.server_info.and_then(|info| info.version),
            }))
            .map_err(|_| protocol("client initialized twice"))?;
        shared
            .send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}))
            .await?;
        if shared.stop.is_cancelled() {
            return Err(shared.error("server stopped during initialization"));
        }
        shared.set_state(ClientState::Running);
        Ok(())
    }
    pub fn state(&self) -> ClientState {
        self.owner.shared.state()
    }
    pub fn process_id(&self) -> u32 {
        self.owner.shared.pid
    }
    pub fn info(&self) -> Result<Arc<ClientInfo>> {
        self.owner
            .shared
            .info
            .get()
            .cloned()
            .ok_or_else(|| self.owner.shared.error("server is not initialized"))
    }
    pub fn last_error(&self) -> Option<String> {
        self.owner
            .shared
            .error
            .lock()
            .expect("LSP error lock")
            .clone()
    }
    pub fn stderr(&self) -> String {
        self.owner.shared.stderr.text()
    }
    pub fn take_events(&self) -> Vec<ServerEvent> {
        self.owner
            .shared
            .events
            .lock()
            .expect("LSP events lock")
            .drain(..)
            .collect()
    }
    pub fn pending_requests(&self) -> usize {
        self.owner
            .shared
            .pending
            .lock()
            .expect("LSP pending lock")
            .len()
    }
    pub fn document_stamp(&self, uri: &Uri) -> Option<DocumentStamp> {
        self.owner
            .shared
            .documents
            .read()
            .expect("LSP document lock")
            .get(uri.as_str())
            .map(|(stamp, _)| *stamp)
    }
    pub fn document_mapper(&self, uri: &Uri) -> Option<PositionMapper> {
        self.owner
            .shared
            .documents
            .read()
            .expect("LSP document lock")
            .get(uri.as_str())
            .map(|(_, mapper)| mapper.clone())
    }
    /// Low-level escape hatch; editor features use capability-checked helpers.
    pub async fn request(
        &self,
        method: &str,
        params: Value,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        self.request_inner(
            method,
            params,
            self.owner.shared.options.request_timeout,
            cancel,
            false,
        )
        .await
    }
    pub async fn sync_document(
        &self,
        snapshot: TextSnapshot,
        save: bool,
        cancel: &CancellationToken,
    ) -> Result<DocumentStamp> {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        if self.state() != ClientState::Running {
            return Err(self.owner.shared.error("server is not running"));
        }
        let (reply, receive) = oneshot::channel();
        self.owner
            .documents
            .try_send(tasks::DocumentWork::Synchronize {
                snapshot,
                save,
                reply,
            })
            .map_err(|_| DomainError::QueueFull)?;
        tokio::select! { biased;_=cancel.cancelled()=>Err(DomainError::Cancelled),result=receive=>result.map_err(|_|self.owner.shared.error("document synchronization stopped"))? }
    }
    pub async fn close_document(&self, origin: DocumentOrigin) -> Result<()> {
        let (reply, receive) = oneshot::channel();
        self.owner
            .documents
            .try_send(tasks::DocumentWork::Close { origin, reply })
            .map_err(|_| DomainError::QueueFull)?;
        receive
            .await
            .map_err(|_| self.owner.shared.error("document synchronization stopped"))?
    }
    pub async fn definitions(
        &self,
        uri: &Uri,
        position: Position,
        cancel: &CancellationToken,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let info = self.info()?;
        if !info
            .capabilities
            .definition_provider
            .as_ref()
            .is_some_and(|cap| !matches!(cap, lsp_types::OneOf::Left(false)))
        {
            return Err(DomainError::Unsupported(
                "server does not support definitions".into(),
            ));
        }
        let value = self
            .request(
                "textDocument/definition",
                json!({"textDocument":{"uri":uri},"position":position}),
                cancel,
            )
            .await?;
        serde_json::from_value(value).map_err(|e| protocol(e.to_string()))
    }
    pub async fn references(
        &self,
        uri: &Uri,
        position: Position,
        cancel: &CancellationToken,
    ) -> Result<Vec<Location>> {
        let info = self.info()?;
        if !info
            .capabilities
            .references_provider
            .as_ref()
            .is_some_and(|cap| !matches!(cap, lsp_types::OneOf::Left(false)))
        {
            return Err(DomainError::Unsupported(
                "server does not support references".into(),
            ));
        }
        let value=self.request("textDocument/references",json!({"textDocument":{"uri":uri},"position":position,"context":{"includeDeclaration":true}}),cancel).await?;
        if value.is_null() {
            Ok(vec![])
        } else {
            serde_json::from_value(value).map_err(|e| protocol(e.to_string()))
        }
    }
    pub async fn completions(
        &self,
        uri: &Uri,
        position: Position,
        cancel: &CancellationToken,
    ) -> Result<Option<CompletionResponse>> {
        if self.info()?.capabilities.completion_provider.is_none() {
            return Err(DomainError::Unsupported(
                "server does not support completion".into(),
            ));
        }
        let value = self
            .request(
                "textDocument/completion",
                json!({"textDocument":{"uri":uri},"position":position}),
                cancel,
            )
            .await?;
        serde_json::from_value(value).map_err(|e| protocol(e.to_string()))
    }
    async fn force_shutdown(&self) {
        self.owner.shared.stop.cancel();
        self.owner.tracker.close();
        self.owner.tracker.wait().await;
    }
    pub async fn shutdown(&self) -> Result<()> {
        let _guard = self.owner.shutdown.lock().await;
        let shared = &self.owner.shared;
        if self.state() == ClientState::Running {
            shared.set_state(ClientState::Stopping);
            shared.cancel_pending("server is stopping");
            let (reply, receive) = oneshot::channel();
            let closed = if self
                .owner
                .documents
                .try_send(tasks::DocumentWork::CloseAll { reply })
                .is_ok()
            {
                matches!(
                    tokio::time::timeout(shared.options.shutdown_timeout, receive).await,
                    Ok(Ok(Ok(())))
                )
            } else {
                false
            };
            if !closed {
                self.force_shutdown().await;
                shared.set_state(ClientState::Stopped);
                return self.shutdown_result();
            }
            let token = CancellationToken::new();
            let _ = self
                .request_inner(
                    "shutdown",
                    Value::Null,
                    shared.options.shutdown_timeout,
                    &token,
                    true,
                )
                .await;
            let _ = shared.send(json!({"jsonrpc":"2.0","method":"exit"})).await;
            let _ =
                tokio::time::timeout(shared.options.exit_timeout, shared.stop.cancelled()).await;
        }
        self.force_shutdown().await;
        if self.state() != ClientState::Failed {
            shared.set_state(ClientState::Stopped);
        }
        self.shutdown_result()
    }
    fn shutdown_result(&self) -> Result<()> {
        match self
            .owner
            .shared
            .supervisor_error
            .lock()
            .expect("LSP supervisor lock")
            .as_ref()
        {
            Some(error) => Err(self.owner.shared.error(error.clone())),
            None => Ok(()),
        }
    }
}
