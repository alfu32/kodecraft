//! Owned external process. No requests are retried across helper epochs.
mod events;
mod request;
mod tasks;
use super::protocol;
pub use events::EventSubscription;
use kode_platform::{
    jobs::CancellationToken,
    process::{ManagedChild, ProcessFactory, ProcessSpec},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Semaphore, mpsc, oneshot};
use tokio_util::task::TaskTracker;

pub type Result<T> = std::result::Result<T, BridgeError>;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    Unavailable,
    Protocol,
    Transport,
    Timeout,
    Cancelled,
    Busy,
    Remote,
}
/// Public diagnostics deliberately omit SQL, credentials and remote error data.
#[derive(Clone, PartialEq, Eq)]
pub struct BridgeError {
    pub kind: Failure,
    pub unknown_outcome: bool,
    pub remote_code: Option<i64>,
    pub message: &'static str,
    remote_data: Option<Value>,
}
impl fmt::Debug for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BridgeError")
            .field("kind", &self.kind)
            .field("unknown_outcome", &self.unknown_outcome)
            .field("remote_code", &self.remote_code)
            .field("message", &self.message)
            .finish_non_exhaustive()
    }
}
impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}{}",
            self.message,
            if self.unknown_outcome {
                "; operation outcome is unknown"
            } else {
                ""
            }
        )
    }
}
impl std::error::Error for BridgeError {}
impl BridgeError {
    pub fn new(kind: Failure, message: &'static str) -> Self {
        Self {
            kind,
            unknown_outcome: false,
            remote_code: None,
            message,
            remote_data: None,
        }
    }
    /// Explicit access for typed feature decoding/redaction. Remote data is
    /// untrusted and potentially sensitive; Debug/Display never include it.
    pub fn remote_data(&self) -> Option<&Value> {
        self.remote_data.as_ref()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Starting,
    Running,
    Stopping,
    Stopped,
    Failed,
}
#[derive(Clone)]
pub struct Options {
    /// Helper-only path/configuration environment; version probing stays separate.
    pub helper_environment: Vec<(String, String)>,
    pub startup_timeout: Duration,
    pub request_timeout: Duration,
    pub write_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub on_state: Option<Arc<dyn Fn(State) + Send + Sync>>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            helper_environment: vec![],
            startup_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(2),
            shutdown_timeout: Duration::from_secs(2),
            on_state: None,
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub protocol_version: u32,
    pub helper_version: String,
    pub java_version: String,
    pub helper_epoch: String,
    pub max_frame_bytes: usize,
    pub capabilities: Vec<String>,
}
/// Notification bodies may contain result data. Debug intentionally omits them.
#[derive(Clone)]
pub struct Event {
    pub method: String,
    pub params: Value,
}
impl fmt::Debug for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Event")
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}
struct Waiting {
    reply: oneshot::Sender<Result<Value>>,
    mutating: bool,
    submitted: Arc<AtomicU8>,
}
struct Shared {
    pid: u32,
    options: Options,
    state: AtomicU8,
    stop: CancellationToken,
    reader_done: CancellationToken,
    next_id: AtomicU64,
    pending: Mutex<BTreeMap<String, Waiting>>,
    slots: Arc<Semaphore>,
    control_slots: Arc<Semaphore>,
    writer: mpsc::Sender<request::Frame>,
    control: mpsc::Sender<request::Frame>,
    budget: Arc<Semaphore>,
    control_budget: Arc<Semaphore>,
    info: OnceLock<Arc<Hello>>,
    stderr: kode_platform::process::TailBuffer,
    events: Mutex<events::Events>,
    event_budget: Arc<Semaphore>,
    failure: Mutex<Option<BridgeError>>,
}
impl Shared {
    fn state(&self) -> State {
        match self.state.load(Ordering::Acquire) {
            0 => State::Starting,
            1 => State::Running,
            2 => State::Stopping,
            3 => State::Stopped,
            _ => State::Failed,
        }
    }
    fn set_state(&self, state: State) {
        self.state.store(state as u8, Ordering::Release);
        if let Some(callback) = &self.options.on_state {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(state)));
        }
    }
    fn fail(&self, error: BridgeError) {
        if !matches!(self.state(), State::Stopping | State::Stopped) {
            let mut failure = self.failure.lock().expect("bridge failure");
            if failure.is_none() {
                *failure = Some(error.clone());
            }
            drop(failure);
            self.set_state(State::Failed);
        }
        let pending = std::mem::take(&mut *self.pending.lock().expect("bridge pending"));
        for (_, waiting) in pending {
            let mut error = error.clone();
            error.unknown_outcome = waiting.mutating
                && waiting
                    .submitted
                    .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
                    == Err(1);
            let _ = waiting.reply.send(Err(error));
        }
        self.stop.cancel();
    }
}
struct Owner {
    shared: Arc<Shared>,
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
pub struct Client {
    owner: Arc<Owner>,
}
impl Client {
    /// Low-level entry point also supports deterministic fake helpers. Production
    /// callers use Runtime, which validates Java and the installed helper first.
    pub async fn start(
        spec: ProcessSpec,
        options: Options,
        factory: Arc<dyn ProcessFactory>,
        cancel: &CancellationToken,
    ) -> Result<Self> {
        if cancel.is_cancelled() {
            return Err(BridgeError::new(
                Failure::Cancelled,
                "JDBC startup cancelled",
            ));
        }
        let mut child = ManagedChild::spawn(spec, factory.as_ref()).map_err(|_| {
            BridgeError::new(
                Failure::Unavailable,
                "Cannot start JDBC helper; check Java and helper installation",
            )
        })?;
        let pid = child.id().unwrap_or(0);
        let pipes = (child.take_stdin(), child.take_stdout());
        let (Some(stdin), Some(stdout)) = pipes else {
            let _ = child.terminate_and_reap().await;
            return Err(BridgeError::new(
                Failure::Transport,
                "JDBC helper pipes unavailable",
            ));
        };
        let (writer, write_rx) = mpsc::channel(32);
        let (control, control_rx) = mpsc::channel(8);
        let shared = Arc::new(Shared {
            pid,
            options,
            state: AtomicU8::new(0),
            stop: CancellationToken::new(),
            reader_done: CancellationToken::new(),
            next_id: AtomicU64::new(1),
            pending: Mutex::new(BTreeMap::new()),
            slots: Arc::new(Semaphore::new(64)),
            control_slots: Arc::new(Semaphore::new(8)),
            writer,
            control,
            budget: Arc::new(Semaphore::new(32 * 1024 * 1024)),
            control_budget: Arc::new(Semaphore::new(1024 * 1024)),
            info: OnceLock::new(),
            stderr: child.stderr_tail(),
            events: Mutex::new(events::Events {
                queue: VecDeque::new(),
                routes: BTreeMap::new(),
            }),
            event_budget: Arc::new(Semaphore::new(32 * 1024 * 1024)),
            failure: Mutex::new(None),
        });
        let tracker = TaskTracker::new();
        tasks::spawn(
            &tracker,
            shared.clone(),
            tasks::write(shared.clone(), stdin, write_rx, control_rx),
        );
        tasks::spawn(
            &tracker,
            shared.clone(),
            tasks::read(shared.clone(), stdout),
        );
        tasks::spawn(
            &tracker,
            shared.clone(),
            tasks::supervise(shared.clone(), child),
        );
        // No await between spawn and this owner: dropping startup cancels/reaps.
        let client = Self {
            owner: Arc::new(Owner {
                shared,
                tracker,
                shutdown: tokio::sync::Mutex::new(()),
            }),
        };
        let result = client.initialize(cancel).await;
        if let Err(error) = result {
            client.force_shutdown().await;
            return Err(error);
        }
        Ok(client)
    }
    async fn initialize(&self, cancel: &CancellationToken) -> Result<()> {
        let value=self.request_inner("hello",json!({"protocolVersion":protocol::VERSION,"clientVersion":env!("CARGO_PKG_VERSION")}),false,true,self.owner.shared.options.startup_timeout,cancel).await?;
        let info: Hello = serde_json::from_value(value)
            .map_err(|_| BridgeError::new(Failure::Protocol, "Invalid JDBC hello response"))?;
        let java = info
            .java_version
            .split(['.', '-', '+'])
            .next()
            .and_then(|n| n.parse::<u32>().ok());
        if info.protocol_version != protocol::VERSION
            || info.max_frame_bytes != protocol::MAX_FRAME_BYTES
            || java.is_none_or(|v| v < 21)
            || info.helper_epoch.is_empty()
            || info.helper_epoch.len() > 128
            || info.helper_version.len() > 128
            || info.capabilities.len() > 256
            || info.capabilities.iter().any(|v| v.len() > 128)
        {
            return Err(BridgeError::new(
                Failure::Protocol,
                "Incompatible JDBC helper; install the matching helper and Java 21 or newer",
            ));
        }
        if self.owner.shared.stop.is_cancelled() {
            return Err(BridgeError::new(
                Failure::Transport,
                "JDBC helper stopped during startup",
            ));
        }
        let _ = self.owner.shared.info.set(Arc::new(info));
        if self
            .owner
            .shared
            .state
            .compare_exchange(
                State::Starting as u8,
                State::Running as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return Err(BridgeError::new(
                Failure::Transport,
                "JDBC helper stopped during startup",
            ));
        }
        if let Some(callback) = &self.owner.shared.options.on_state {
            let _ =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(State::Running)));
        }
        Ok(())
    }
    pub fn state(&self) -> State {
        self.owner.shared.state()
    }
    /// Observe loss of this epoch without polling or starting another helper.
    pub async fn wait_stopped(&self) {
        self.owner.shared.stop.cancelled().await;
    }
    pub fn process_id(&self) -> u32 {
        self.owner.shared.pid
    }
    pub fn info(&self) -> Option<Arc<Hello>> {
        self.owner.shared.info.get().cloned()
    }
    pub fn last_error(&self) -> Option<BridgeError> {
        self.owner
            .shared
            .failure
            .lock()
            .expect("bridge failure")
            .clone()
    }
    pub fn pending_requests(&self) -> usize {
        self.owner
            .shared
            .pending
            .lock()
            .expect("bridge pending")
            .len()
    }
    /// Drivers and Java launchers may print secrets without our cooperation.
    /// Keep a bounded tail for stream drainage, but expose only this redacted
    /// indication. Raw stderr is never included in errors or notifications.
    pub fn diagnostic_tail(&self) -> Option<&'static str> {
        (!self.owner.shared.stderr.bytes().is_empty()).then_some(
            "JDBC helper emitted stderr; raw diagnostics suppressed to protect credentials",
        )
    }
    pub fn take_events(&self) -> Vec<Event> {
        let mut events = self.owner.shared.events.lock().expect("bridge events");
        events.queue.drain(..).map(|frame| frame.event).collect()
    }
    /// A dropped/cancelled request does not prove that JDBC stopped. Mutating
    /// requests report unknown_outcome once writing could have begun. Cancellation
    /// of SQL itself uses an independent query.cancel request with execution ID.
    pub async fn request(
        &self,
        method: &str,
        params: Value,
        mutating: bool,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        self.request_with_timeout(
            method,
            params,
            mutating,
            self.owner.shared.options.request_timeout,
            cancel,
        )
        .await
    }
    pub async fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        mutating: bool,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        self.request_inner(method, params, mutating, false, timeout, cancel)
            .await
    }
    pub fn cancel(&self) {
        if !matches!(self.state(), State::Failed | State::Stopped) {
            self.owner.shared.set_state(State::Stopping);
        }
        self.owner.shared.stop.cancel();
        self.owner.tracker.close();
    }
    pub(super) fn cleanup_tracker(&self) -> TaskTracker {
        self.owner.tracker.clone()
    }
    pub async fn force_shutdown(&self) {
        self.owner.shared.stop.cancel();
        self.owner.tracker.close();
        self.owner.tracker.wait().await;
    }
    pub async fn shutdown(&self) -> Result<()> {
        let _guard = self.owner.shutdown.lock().await;
        let shared = &self.owner.shared;
        if self.state() == State::Running {
            shared.set_state(State::Stopping);
            let _ = self
                .request_inner(
                    "shutdown",
                    json!({}),
                    false,
                    true,
                    shared.options.shutdown_timeout,
                    &CancellationToken::new(),
                )
                .await;
            let _ = tokio::time::timeout(
                shared.options.shutdown_timeout,
                shared.reader_done.cancelled(),
            )
            .await;
        }
        self.force_shutdown().await;
        if self.state() != State::Failed {
            shared.set_state(State::Stopped);
        }
        self.last_error().map_or(Ok(()), Err)
    }
}
