//! Streaming SQL state. Event collection is independent from rendering and
//! retains bounded previews even when later SQL or helper communication fails.
use crate::{
    bridge::{
        client::{BridgeError, Client, Failure},
        wire::{
            CancelStatus, Cell, Column, Outcome, QueryEvent, QueryFinal, QueryNotification,
            SqlError,
        },
    },
    connections::{Connections, SessionInfo},
};
use kode_platform::jobs::CancellationToken;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Notify, Semaphore, watch};
use tokio_util::task::TaskTracker;
mod preview;
use preview::Accumulator;
use serde_json::{Value, json};
use std::sync::atomic::AtomicBool;
static NEXT_EXECUTION: AtomicU64 = AtomicU64::new(1);
pub use preview::{Preview, ResultSet};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Queued,
    Running,
    Cancelling,
    Finished,
}
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub execution_id: Option<String>,
    pub phase: Phase,
    pub preview: Arc<Preview>,
    pub completion: Option<QueryFinal>,
    pub error: Option<BridgeError>,
    pub unknown_outcome: bool,
    pub cancel_status: Option<CancelStatus>,
    pub cancel_error: Option<BridgeError>,
}
impl Snapshot {
    /// Appends one SQL script statement's outcome while preserving all prior
    /// statement results and their bounded-preview retention permits.
    pub fn append_statement(&mut self, mut next: Self) {
        let mut preview = self.preview.as_ref().clone();
        preview.append(&next.preview);
        self.preview = Arc::new(preview);
        self.execution_id = next.execution_id.take();
        self.phase = next.phase;
        self.completion = next.completion.take();
        self.error = next.error.take();
        self.unknown_outcome |= next.unknown_outcome;
        self.cancel_status = next.cancel_status;
        self.cancel_error = next.cancel_error.take();
    }
}
#[derive(Debug, Clone)]
pub struct Options {
    pub max_rows: u32,
    pub fetch_size: u32,
    /// Absent leaves context unchanged; Some(None) restores the login default.
    /// Context changes and execution share the connection owner's serialized job.
    pub catalog: Option<Option<String>>,
    pub schema: Option<Option<String>>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            max_rows: 500,
            fetch_size: 200,
            catalog: None,
            schema: None,
        }
    }
}

struct Pending {
    session: SessionInfo,
    accumulator: Mutex<Accumulator>,
    progress: watch::Sender<Snapshot>,
    changed: Notify,
    cancel: AtomicBool,
    events_closed: AtomicBool,
}
impl Pending {
    fn publish(&self, accumulator: &Accumulator) {
        self.progress.send_replace(accumulator.snapshot.clone());
        self.changed.notify_one();
    }
    fn fail(&self, error: BridgeError) {
        let mut accumulator = self.accumulator.lock().expect("SQL preview");
        if accumulator.snapshot.phase != Phase::Finished {
            accumulator.fail(error);
            self.publish(&accumulator);
        }
    }
}
struct Epoch {
    id: String,
    stop: CancellationToken,
}
struct Shared {
    connections: Connections,
    pending: Mutex<BTreeMap<String, Arc<Pending>>>,
    epoch: tokio::sync::Mutex<Option<Epoch>>,
    retention: Arc<Semaphore>,
    stop: CancellationToken,
}
struct Submission {
    session: SessionInfo,
    id: String,
    sql: String,
    options: Options,
    progress: watch::Sender<Snapshot>,
    token: CancellationToken,
    tasks: TaskTracker,
}
struct Owner {
    shared: Arc<Shared>,
    tasks: TaskTracker,
    slots: Arc<Semaphore>,
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.shared.stop.cancel();
        self.tasks.close();
    }
}
#[derive(Clone)]
pub struct Executor {
    owner: Arc<Owner>,
}
impl Executor {
    pub fn new(connections: Connections) -> Self {
        Self {
            owner: Arc::new(Owner {
                shared: Arc::new(Shared {
                    connections,
                    pending: Mutex::new(BTreeMap::new()),
                    epoch: tokio::sync::Mutex::new(None),
                    retention: Arc::new(Semaphore::new(64 * 1024 * 1024)),
                    stop: CancellationToken::new(),
                }),
                tasks: TaskTracker::new(),
                slots: Arc::new(Semaphore::new(16)),
            }),
        }
    }
    pub async fn execute(
        &self,
        source: &str,
        sql: String,
        options: Options,
        progress: watch::Sender<Snapshot>,
        cancel: &CancellationToken,
    ) -> std::result::Result<Snapshot, BridgeError> {
        if sql.trim().is_empty()
            || sql.len() > 4 * 1024 * 1024
            || !(1..=100_000).contains(&options.max_rows)
            || options.fetch_size > 10_000
            || [&options.catalog, &options.schema]
                .into_iter()
                .any(|value| {
                    value
                        .as_ref()
                        .and_then(Option::as_ref)
                        .is_some_and(|value| value.len() > 4096 || value.contains('\0'))
                })
        {
            return Err(BridgeError::new(
                Failure::Protocol,
                "Invalid SQL text or preview limits",
            ));
        }
        if self.owner.shared.stop.is_cancelled() || cancel.is_cancelled() {
            return Err(BridgeError::new(
                Failure::Cancelled,
                "SQL execution cancelled before submission",
            ));
        }
        let session = self
            .owner
            .shared
            .connections
            .session(source)
            .map_err(|error| error.bridge)?;
        let slot = self
            .owner
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| BridgeError::new(Failure::Busy, "SQL execution capacity exhausted"))?;
        let id = NEXT_EXECUTION
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map(|id| format!("execution-{id}"))
            .map_err(|_| BridgeError::new(Failure::Busy, "SQL execution IDs exhausted"))?;
        let token = self.owner.shared.stop.child_token();
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
            shared
                .execute(Submission {
                    session,
                    id,
                    sql,
                    options,
                    progress,
                    token,
                    tasks,
                })
                .await
        });
        let result = tokio::select! {biased;_=cancel.cancelled()=>{guard.0.cancel();task.await},result=&mut task=>result};
        result.map_err(|_| BridgeError::new(Failure::Transport, "SQL execution worker failed"))?
    }
    pub async fn shutdown(&self) {
        self.owner.shared.stop.cancel();
        self.owner.tasks.close();
        self.owner.tasks.wait().await;
    }
    /// Explicit retry/command cancellation for the current execution identity.
    /// This never starts Java for an unknown/already finished execution.
    pub async fn cancel(
        &self,
        id: &str,
        cancel: &CancellationToken,
    ) -> std::result::Result<CancelStatus, BridgeError> {
        let pending = self
            .owner
            .shared
            .pending
            .lock()
            .expect("SQL pending")
            .get(id)
            .cloned();
        let Some(pending) = pending else {
            return Ok(CancelStatus::AlreadyFinished);
        };
        if pending
            .accumulator
            .lock()
            .expect("SQL preview")
            .snapshot
            .phase
            == Phase::Finished
        {
            return Ok(CancelStatus::AlreadyFinished);
        }
        let client = self.owner.shared.connections.runtime().get(cancel).await?;
        if client
            .info()
            .is_none_or(|hello| hello.helper_epoch != pending.session.helper_epoch)
        {
            return Err(BridgeError::new(
                Failure::Transport,
                "Execution belongs to a stale helper",
            ));
        }
        let result = client
            .request_with_timeout(
                "query.cancel",
                json!({"executionId":id}),
                false,
                Duration::from_secs(2),
                cancel,
            )
            .await
            .and_then(cancel_status);
        let mut accumulator = pending.accumulator.lock().expect("SQL preview");
        if accumulator.snapshot.phase != Phase::Finished {
            accumulator.snapshot.phase = Phase::Cancelling;
        }
        match &result {
            Ok(status) => {
                accumulator.snapshot.cancel_status = Some(*status);
                accumulator.snapshot.cancel_error = None;
            }
            Err(error) => accumulator.snapshot.cancel_error = Some(error.clone()),
        }
        pending.publish(&accumulator);
        result
    }
}
fn unknown(kind: Failure, message: &'static str) -> BridgeError {
    let mut error = BridgeError::new(kind, message);
    error.unknown_outcome = true;
    error
}
impl Shared {
    async fn client(
        self: &Arc<Self>,
        session: &SessionInfo,
        token: &CancellationToken,
        tasks: &TaskTracker,
    ) -> std::result::Result<Client, BridgeError> {
        let client = self.connections.runtime().get(token).await?;
        let id = client
            .info()
            .ok_or_else(|| BridgeError::new(Failure::Protocol, "SQL helper has no epoch"))?
            .helper_epoch
            .clone();
        if client.info().is_none_or(|hello| {
            !["query.execute", "query.cancel"]
                .iter()
                .all(|method| hello.capabilities.iter().any(|name| name == method))
        }) {
            return Err(BridgeError::new(
                Failure::Unavailable,
                "SQL execution is unavailable in this helper; rebuild or update the JDBC helper",
            ));
        }
        if session.helper_epoch != id {
            return Err(BridgeError::new(
                Failure::Transport,
                "SQL connection belongs to a stale helper; connect explicitly",
            ));
        }
        let mut epoch = tokio::select! {biased;_=token.cancelled()=>return Err(BridgeError::new(Failure::Cancelled,"SQL preparation cancelled")),epoch=self.epoch.lock()=>epoch};
        if epoch.as_ref().is_some_and(|epoch| epoch.id == id) {
            return Ok(client);
        }
        if let Some(old) = epoch.take() {
            old.stop.cancel();
        }
        let mut events = client.subscribe_events("query.event")?;
        let stop = self.stop.child_token();
        let lifetime = stop.clone();
        let weak = Arc::downgrade(self);
        let watched = client.clone();
        let watched_id = id.clone();
        tasks.spawn(async move {
            loop {
                let event=tokio::select!{biased;_=lifetime.cancelled()=>break,event=events.recv()=>match event {Some(event)=>event,None=>break}};
                let Some(shared)=weak.upgrade()else{break;};
                let size=match serde_json::to_vec(&event.params){Ok(bytes)=>bytes.len(),Err(_)=>{watched.cancel();break;}};
                let notification:QueryNotification=match serde_json::from_value(event.params){Ok(value)=>value,Err(_)=>{shared.fail_epoch(&watched_id,unknown(Failure::Protocol,"Invalid SQL event payload"));watched.cancel();break;}};
                let pending=shared.pending.lock().expect("SQL pending").get(&notification.execution_id).cloned();
                if let Some(pending)=pending {
                    let mut accumulator=pending.accumulator.lock().expect("SQL preview");
                    match accumulator.accept(notification,size){
                        Ok(limit)=>{if limit{pending.cancel.store(true,Ordering::Release);}pending.publish(&accumulator);},
                        Err(error)=>{drop(accumulator);shared.fail_epoch(&watched_id,unknown(error.kind,error.message));watched.cancel();break;},
                    }
                }
            }
            if let Some(shared)=weak.upgrade(){
                let pending:Vec<_>=shared.pending.lock().expect("SQL pending").values().filter(|pending|pending.session.helper_epoch==watched_id).cloned().collect();
                for pending in pending{pending.events_closed.store(true,Ordering::Release);pending.changed.notify_one();}
            }
        });
        *epoch = Some(Epoch { id, stop });
        Ok(client)
    }
    fn fail_epoch(&self, epoch: &str, error: BridgeError) {
        let pending: Vec<_> = self
            .pending
            .lock()
            .expect("SQL pending")
            .values()
            .filter(|pending| pending.session.helper_epoch == epoch)
            .cloned()
            .collect();
        for pending in pending {
            pending.fail(error.clone());
        }
    }
    async fn execute(
        self: Arc<Self>,
        submission: Submission,
    ) -> std::result::Result<Snapshot, BridgeError> {
        let Submission {
            session,
            id,
            sql,
            options,
            progress,
            token,
            tasks,
        } = submission;
        if self.retention.available_permits() < 4096 {
            return Err(BridgeError::new(
                Failure::Busy,
                "SQL preview capacity is full; close previous results before executing",
            ));
        }
        let client = self.client(&session, &token, &tasks).await?;
        // A disconnect can happen while the event consumer is being installed.
        let current = self
            .connections
            .session(&session.data_source_id)
            .map_err(|error| error.bridge)?;
        if current.connection_id != session.connection_id
            || current.helper_epoch != session.helper_epoch
        {
            return Err(BridgeError::new(
                Failure::Transport,
                "SQL connection changed before submission",
            ));
        }
        self.connections.execution_submitted(&session);
        let pending = Arc::new(Pending {
            session: session.clone(),
            accumulator: Mutex::new(Accumulator::new(id.clone(), self.retention.clone())),
            progress,
            changed: Notify::new(),
            cancel: AtomicBool::new(false),
            events_closed: AtomicBool::new(false),
        });
        self.pending
            .lock()
            .expect("SQL pending")
            .insert(id.clone(), pending.clone());
        struct Registration {
            shared: Arc<Shared>,
            id: String,
        }
        impl Drop for Registration {
            fn drop(&mut self) {
                self.shared
                    .pending
                    .lock()
                    .expect("SQL pending")
                    .remove(&self.id);
            }
        }
        let _registration = Registration {
            shared: self.clone(),
            id: id.clone(),
        };
        pending.publish(&pending.accumulator.lock().expect("SQL preview"));
        let mut params = json!({"connectionId":session.connection_id,"executionId":id,"sql":sql,"maxRows":options.max_rows,"fetchSize":options.fetch_size});
        if let Some(catalog) = options.catalog {
            params["catalog"] = json!(catalog);
        }
        if let Some(schema) = options.schema {
            params["schema"] = json!(schema);
        }
        let no_cancel = CancellationToken::new();
        let request = client.request_with_timeout(
            "query.execute",
            params,
            true,
            Duration::from_secs(600),
            &no_cancel,
        );
        tokio::pin!(request);
        let mut cancellation_sent = false;
        let response = loop {
            let failure = pending
                .accumulator
                .lock()
                .expect("SQL preview")
                .snapshot
                .error
                .clone();
            if let Some(failure) = failure {
                break Err(failure);
            }
            if (token.is_cancelled() || pending.cancel.load(Ordering::Acquire))
                && !cancellation_sent
            {
                cancellation_sent = true;
                {
                    let mut accumulator = pending.accumulator.lock().expect("SQL preview");
                    accumulator.snapshot.phase = Phase::Cancelling;
                    pending.publish(&accumulator);
                }
                let acknowledgement = cancel_query(&client, &id).await;
                let mut accumulator = pending.accumulator.lock().expect("SQL preview");
                match acknowledgement {
                    Ok(status) => accumulator.snapshot.cancel_status = Some(status),
                    Err(error) => accumulator.snapshot.cancel_error = Some(error),
                }
                pending.publish(&accumulator);
            }
            tokio::select! {biased;
                _=self.stop.cancelled()=>break Err(unknown(Failure::Cancelled,"Workspace closed while SQL was active; outcome is unknown")),
                result=&mut request=>break result,
                _=token.cancelled(),if !cancellation_sent=>{},
                _=pending.changed.notified()=>{},
            }
        };
        let result = match response {
            Ok(value) => self.complete(&pending, &id, value).await,
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            if !cancellation_sent && error.kind != Failure::Remote {
                let _ = cancel_query(&client, &id).await;
            }
            if error.kind == Failure::Protocol {
                client.cancel();
            }
            self.connections.execution_failed(&session, error.clone());
            pending.fail(error);
        }
        let snapshot = pending
            .accumulator
            .lock()
            .expect("SQL preview")
            .snapshot
            .clone();
        Ok(snapshot)
    }
    async fn complete(
        &self,
        pending: &Pending,
        id: &str,
        value: Value,
    ) -> std::result::Result<(), BridgeError> {
        let final_result: QueryFinal = serde_json::from_value(value.clone())
            .map_err(|_| unknown(Failure::Protocol, "Invalid SQL final response"))?;
        if final_result.execution_id != id || final_result.event_count.is_none() {
            return Err(unknown(
                Failure::Protocol,
                "SQL final response has no event barrier",
            ));
        }
        let count = final_result.event_count.expect("checked SQL count");
        let deadline = tokio::time::sleep(Duration::from_secs(5));
        tokio::pin!(deadline);
        loop {
            {
                let accumulator = pending.accumulator.lock().expect("SQL preview");
                if let Some(error) = accumulator.snapshot.error.as_ref() {
                    return Err(error.clone());
                }
                if accumulator.next >= count {
                    break;
                }
                if pending.events_closed.load(Ordering::Acquire) {
                    return Err(unknown(
                        Failure::Protocol,
                        "SQL final response is missing preceding events",
                    ));
                }
            }
            tokio::select! {biased;_=self.stop.cancelled()=>return Err(unknown(Failure::Cancelled,"Workspace closed before SQL events drained")),_= &mut deadline=>return Err(unknown(Failure::Protocol,"SQL final response is missing preceding events")),_=pending.changed.notified()=>{}}
        }
        pending
            .accumulator
            .lock()
            .expect("SQL preview")
            .finish(final_result.clone())
            .map_err(|error| unknown(error.kind, error.message))?;
        self.connections.execution_completed(
            &pending.session,
            &final_result,
            value.get("connectionState"),
        );
        let accumulator = pending.accumulator.lock().expect("SQL preview");
        pending.publish(&accumulator);
        Ok(())
    }
}
async fn cancel_query(client: &Client, id: &str) -> std::result::Result<CancelStatus, BridgeError> {
    let value = client
        .request_with_timeout(
            "query.cancel",
            json!({"executionId":id}),
            false,
            Duration::from_secs(2),
            &CancellationToken::new(),
        )
        .await?;
    cancel_status(value)
}
fn cancel_status(value: Value) -> std::result::Result<CancelStatus, BridgeError> {
    serde_json::from_value(value.get("status").cloned().unwrap_or(Value::Null)).map_err(|_| {
        BridgeError::new(
            Failure::Protocol,
            "Invalid SQL cancellation acknowledgement",
        )
    })
}
