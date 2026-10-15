use super::{sqlite::SqliteStore, *};
use crate::{model::ResolvedSemanticProject, snapshot::SnapshotPublisher};
use kode_core::{DomainError, JobTag, Result};
use kode_platform::jobs::CancellationToken;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use tokio::sync::oneshot;
const CAPACITY: usize = 16;
type Job = Box<dyn FnOnce(&mut State) + Send>;
struct State {
    db: SqliteStore,
    publisher: Arc<SnapshotPublisher>,
}
pub struct StoreReply<T> {
    pub tag: JobTag,
    pub value: T,
}
pub struct StoreTicket<T> {
    pub tag: JobTag,
    receiver: oneshot::Receiver<Result<T>>,
}
impl<T> StoreTicket<T> {
    pub async fn resolve(self) -> Result<StoreReply<T>> {
        let value = self.receiver.await.map_err(|_| {
            DomainError::Unsupported("semantic SQLite worker stopped before replying".into())
        })??;
        Ok(StoreReply {
            tag: self.tag,
            value,
        })
    }
}
#[derive(Clone)]
pub struct StoreClient {
    sender: SyncSender<Job>,
    closing: Arc<AtomicBool>,
    submit_gate: Arc<Mutex<()>>,
    publisher: Arc<SnapshotPublisher>,
}
pub struct StoreWorker {
    client: StoreClient,
    thread: Option<JoinHandle<()>>,
    pub status: CacheStatus,
}
struct StartupGuard {
    handle: Option<JoinHandle<()>>,
    closing: Arc<AtomicBool>,
}
impl Drop for StartupGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.closing.store(true, Ordering::Release);
            let _ = handle.join();
        }
    }
}
impl StoreWorker {
    /// Opening and loading run exclusively on the new dedicated thread.
    pub async fn start(location: StoreLocation) -> Result<Self> {
        Self::start_mode(location, false).await
    }
    /// Explicitly reset an owned cache schema before reindexing sources.
    pub async fn rebuild(location: StoreLocation) -> Result<Self> {
        Self::start_mode(location, true).await
    }
    async fn start_mode(location: StoreLocation, rebuild: bool) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Job>(CAPACITY);
        let closing = Arc::new(AtomicBool::new(false));
        let stop = closing.clone();
        let (ready_tx, ready_rx) = oneshot::channel();
        let handle = thread::Builder::new()
            .name("KodeSQLite".into())
            .spawn(move || {
                let opened = SqliteStore::open(&location, rebuild).and_then(|(mut db, status)| {
                    let loaded = db.with_operation(&CancellationToken::new(), |db| db.load())?;
                    let publisher = Arc::new(SnapshotPublisher::from_snapshot(loaded.snapshot));
                    Ok((State { db, publisher }, status))
                });
                let (mut state, status) = match opened {
                    Ok(value) => value,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                if ready_tx
                    .send(Ok((state.publisher.clone(), status)))
                    .is_err()
                {
                    return;
                }
                loop {
                    let job = if stop.load(Ordering::Acquire) {
                        match receiver.try_recv() {
                            Ok(job) => job,
                            Err(_) => break,
                        }
                    } else {
                        match receiver.recv_timeout(Duration::from_millis(50)) {
                            Ok(job) => job,
                            Err(mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    };
                    job(&mut state);
                }
            })
            .map_err(|e| DomainError::Process {
                program: "native SQLite worker".into(),
                message: e.to_string(),
            })?;
        let mut startup = StartupGuard {
            handle: Some(handle),
            closing: closing.clone(),
        };
        let ready = ready_rx
            .await
            .map_err(|_| DomainError::Unsupported("semantic cache startup thread failed".into()))
            .and_then(|value| value);
        let (publisher, status) = match ready {
            Ok(value) => value,
            Err(error) => {
                closing.store(true, Ordering::Release);
                join(startup.handle.take().expect("startup owns thread")).await?;
                return Err(error);
            }
        };
        Ok(Self {
            client: StoreClient {
                sender,
                closing,
                submit_gate: Arc::new(Mutex::new(())),
                publisher,
            },
            thread: startup.handle.take(),
            status,
        })
    }
    pub fn client(&self) -> StoreClient {
        self.client.clone()
    }
    /// Stop submissions, drain already accepted critical jobs and join off the
    /// application thread. Cancellation is explicit on individual requests.
    pub async fn shutdown(mut self) -> Result<()> {
        self.client.close();
        if let Some(handle) = self.thread.take() {
            join(handle).await?;
        }
        Ok(())
    }
}
async fn join(handle: JoinHandle<()>) -> Result<()> {
    tokio::task::spawn_blocking(move || handle.join())
        .await
        .map_err(|e| DomainError::Unsupported(format!("SQLite worker join failed: {e}")))?
        .map_err(|_| DomainError::Unsupported("SQLite worker panicked".into()))
}
impl Drop for StoreWorker {
    fn drop(&mut self) {
        self.client.close();
        // Orderly owners call shutdown(). Fallback drop still never detaches a
        // writer; it drains and joins before releasing the workspace's cache.
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}
impl StoreClient {
    fn close(&self) {
        let _guard = self
            .submit_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.closing.store(true, Ordering::Release);
    }
    pub fn snapshot(&self) -> Result<SemanticSnapshot> {
        self.publisher.load()
    }
    fn submit<T: Send + 'static>(
        &self,
        tag: JobTag,
        cancel: CancellationToken,
        operation: impl FnOnce(&mut State, &CancellationToken) -> Result<T> + Send + 'static,
    ) -> Result<StoreTicket<T>> {
        let _guard = self
            .submit_gate
            .lock()
            .map_err(|_| DomainError::Unsupported("SQLite submission lock poisoned".into()))?;
        if self.closing.load(Ordering::Acquire) {
            return Err(DomainError::Cancelled);
        }
        let (sender, receiver) = oneshot::channel();
        let job: Job = Box::new(move |state| {
            let result = if cancel.is_cancelled() {
                Err(DomainError::Cancelled)
            } else {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| operation(state, &cancel)))
                    .unwrap_or_else(|_| {
                        Err(DomainError::Unsupported(
                            "semantic cache operation panicked".into(),
                        ))
                    })
            };
            let _ = sender.send(result);
        });
        self.sender.try_send(job).map_err(|error| match error {
            TrySendError::Full(_) => DomainError::QueueFull,
            TrySendError::Disconnected(_) => DomainError::Cancelled,
        })?;
        Ok(StoreTicket { tag, receiver })
    }
    pub fn load(&self, tag: JobTag, cancel: CancellationToken) -> Result<StoreTicket<StoredIndex>> {
        self.submit(tag, cancel, |state, cancel| {
            let mut loaded = state.db.with_operation(cancel, |db| db.load())?;
            let current = state.publisher.load()?;
            if current.version() == loaded.snapshot.version() {
                loaded.snapshot = current;
            } else if loaded.snapshot.version() > current.version() {
                drop(state.publisher.publish(loaded.snapshot.clone())?);
            } else {
                return Err(DomainError::Unsupported(
                    "cache revision moved behind the published snapshot; restart indexing".into(),
                ));
            }
            Ok(loaded)
        })
    }
    pub fn commit(
        &self,
        tag: JobTag,
        snapshot: SemanticSnapshot,
        compatibility: Option<Vec<CompatibilityFile>>,
        cancel: CancellationToken,
    ) -> Result<StoreTicket<CommitReceipt>> {
        self.submit(tag, cancel, move |state, cancel| {
            let current = state.publisher.load()?;
            if state.db.revision()? != current.version() {
                return Err(DomainError::Unsupported(
                    "cache changed in another writer; reload before committing".into(),
                ));
            }
            let (receipt, next) = state
                .db
                .with_operation(cancel, |db| db.persist(snapshot, compatibility, cancel))?;
            if !receipt.reused {
                let previous = state.publisher.publish(next)?;
                drop(previous);
            }
            Ok(receipt)
        })
    }
    pub fn clear(
        &self,
        tag: JobTag,
        next_revision: u64,
        cancel: CancellationToken,
    ) -> Result<StoreTicket<CommitReceipt>> {
        let snapshot = SemanticSnapshot::new(next_revision, ResolvedSemanticProject::default())?;
        self.commit(tag, snapshot, Some(vec![]), cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{frontend::kotlin::KotlinSemanticAdapter, model::ResolvedSemanticProject};
    use kode_core::{
        DocumentId, DocumentOrigin, RequestId, WorkspaceGeneration, document::Document,
    };
    use kode_syntax::registry::GrammarRegistry;
    fn tag() -> JobTag {
        JobTag {
            workspace: WorkspaceGeneration(1),
            request: RequestId::fresh().unwrap(),
            document: None,
        }
    }
    fn prepared(version: u64, name: &str) -> SemanticSnapshot {
        let document = Document::new(
            DocumentId::fresh().unwrap(),
            DocumentOrigin::Virtual {
                owner: "test".into(),
                key: "test".into(),
                label: "test".into(),
            },
            Some("kotlin".into()),
            &format!("class {name}\n"),
        );
        let delta = KotlinSemanticAdapter::new(&GrammarRegistry::compiled())
            .unwrap()
            .extract(
                "Class.kt",
                &document.snapshot(),
                WorkspaceGeneration(1),
                &CancellationToken::new(),
            )
            .unwrap();
        SemanticSnapshot::new(
            version,
            ResolvedSemanticProject::from_deltas([delta]).unwrap(),
        )
        .unwrap()
    }
    #[tokio::test]
    async fn bounded_worker_keeps_old_readers_available_until_the_transaction_publishes() {
        let worker = StoreWorker::start(StoreLocation::Memory).await.unwrap();
        let client = worker.client();
        let before = client.snapshot().unwrap();
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let blocker = client
            .submit(tag(), CancellationToken::new(), move |_, _| {
                entered_tx
                    .send(thread::current().name().unwrap_or_default().to_owned())
                    .unwrap();
                release_rx
                    .recv_timeout(Duration::from_secs(5))
                    .map_err(|e| DomainError::Unsupported(e.to_string()))?;
                Ok(())
            })
            .unwrap();
        assert_eq!(entered_rx.await.unwrap(), "KodeSQLite");
        let pending = client
            .commit(
                tag(),
                prepared(1, "Pending"),
                None,
                CancellationToken::new(),
            )
            .unwrap();
        let mut queued = vec![];
        for _ in 1..CAPACITY {
            queued.push(client.load(tag(), CancellationToken::new()).unwrap());
        }
        assert!(matches!(
            client.load(tag(), CancellationToken::new()),
            Err(DomainError::QueueFull)
        ));
        assert!(std::ptr::eq(
            before.project(),
            client.snapshot().unwrap().project()
        ));
        assert_eq!(before.workspace_symbols().count(), 0);
        release_tx.send(()).unwrap();
        blocker.resolve().await.unwrap();
        pending.resolve().await.unwrap();
        assert!(
            client
                .snapshot()
                .unwrap()
                .workspace_symbols()
                .any(|s| s.name == "Pending")
        );
        assert_eq!(before.workspace_symbols().count(), 0);
        for ticket in queued {
            ticket.resolve().await.unwrap();
        }
        worker.shutdown().await.unwrap();
        assert!(matches!(
            client.load(tag(), CancellationToken::new()),
            Err(DomainError::Cancelled)
        ));
    }
    #[tokio::test]
    async fn sqlite_failure_after_deleting_old_rows_rolls_back_and_never_publishes() {
        let worker = StoreWorker::start(StoreLocation::Memory).await.unwrap();
        let client = worker.client();
        client
            .commit(tag(), prepared(1, "Before"), None, CancellationToken::new())
            .unwrap()
            .resolve()
            .await
            .unwrap();
        let before = client.snapshot().unwrap();
        client.submit(tag(),CancellationToken::new(),|state,_| {
            state.db.db.execute_batch("CREATE TRIGGER reject_after BEFORE INSERT ON symbols WHEN NEW.name='After' BEGIN SELECT RAISE(ABORT,'injected write interruption'); END;").map_err(super::super::sqlite::sql_error)?;Ok(())
        }).unwrap().resolve().await.unwrap();
        assert!(matches!(
            client
                .commit(tag(), prepared(2, "After"), None, CancellationToken::new())
                .unwrap()
                .resolve()
                .await,
            Err(DomainError::Sql { .. })
        ));
        assert!(std::ptr::eq(
            before.project(),
            client.snapshot().unwrap().project()
        ));
        let stored = client
            .load(tag(), CancellationToken::new())
            .unwrap()
            .resolve()
            .await
            .unwrap()
            .value;
        assert_eq!(stored.snapshot.version(), 1);
        assert!(
            stored
                .snapshot
                .workspace_symbols()
                .any(|s| s.name == "Before")
        );
        assert!(
            !stored
                .snapshot
                .workspace_symbols()
                .any(|s| s.name == "After")
        );
        worker.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn sqlite_vm_progress_observes_cancellation_and_later_requests_recover() {
        let worker = StoreWorker::start(StoreLocation::Memory).await.unwrap();
        let client = worker.client();
        let token = CancellationToken::new();
        let cancellation = token.clone();
        let ticket=client.submit(tag(),token,move|state,cancel| {
            state.db.with_operation(cancel,|db| {
                // Cancel after the operation entered its SQLite guard, then run
                // enough VM instructions to exercise the real progress callback.
                cancellation.cancel();
                db.db.query_row("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000) SELECT SUM(x) FROM n",[],|row|row.get::<_,i64>(0)).map_err(super::super::sqlite::sql_error)
            })
        }).unwrap();
        assert!(matches!(
            ticket.resolve().await,
            Err(DomainError::Cancelled)
        ));
        client
            .commit(
                tag(),
                prepared(1, "Recovered"),
                None,
                CancellationToken::new(),
            )
            .unwrap()
            .resolve()
            .await
            .unwrap();
        assert!(
            client
                .snapshot()
                .unwrap()
                .workspace_symbols()
                .any(|s| s.name == "Recovered")
        );
        worker.shutdown().await.unwrap();
    }
}
