//! Bounded workers, independent cancellation, coalesced notifications and draining shutdown.
use futures_util::FutureExt;
use kode_core::{DomainError, JobTag, Result, WorkspaceGeneration};
use std::{
    collections::HashMap,
    future::Future,
    panic::AssertUnwindSafe,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::{
    sync::{Semaphore, mpsc, watch},
    task::JoinSet,
};
pub use tokio_util::sync::CancellationToken;

pub const COMPLETION_CAPACITY: usize = 64;
pub const ASYNC_CAPACITY: usize = 32;
pub const BLOCKING_CAPACITY: usize = 4;
pub const PROGRESS_CAPACITY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OwnerId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Saves/failures retain their completion even under queue pressure or disposal.
    Critical,
    /// Superseded previews can be cancelled while waiting for publication capacity.
    Cancellable,
}

#[derive(Debug)]
pub struct Completion<T> {
    pub tag: JobTag,
    pub owner: OwnerId,
    pub result: Result<T>,
}

pub type JobFuture<T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'static>>;
pub type JobFactory<T> = Box<dyn FnOnce(CancellationToken) -> JobFuture<T> + Send + 'static>;
pub type BlockingJobFactory<T> = Box<dyn FnOnce(CancellationToken) -> Result<T> + Send + 'static>;

/// Inject this boundary into feature coordinators to replay jobs deterministically.
pub trait WorkerScheduler<T: Send + 'static> {
    fn schedule(
        &mut self,
        owner: OwnerId,
        tag: JobTag,
        delivery: Delivery,
        work: JobFactory<T>,
    ) -> Result<()>;
    fn schedule_blocking(
        &mut self,
        owner: OwnerId,
        tag: JobTag,
        delivery: Delivery,
        work: BlockingJobFactory<T>,
    ) -> Result<()>;
    fn dispose_owner(&mut self, owner: OwnerId);
    fn dispose_workspace(&mut self, workspace: WorkspaceGeneration);
}

pub trait Clock: Send + Sync {
    fn now_millis(&self) -> u64;
}
pub struct MonotonicClock(Instant);
impl Default for MonotonicClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}
impl Clock for MonotonicClock {
    fn now_millis(&self) -> u64 {
        self.0.elapsed().as_millis().min(u64::MAX as u128) as u64
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub tag: JobTag,
    pub message: String,
    pub completed: u64,
    pub total: Option<u64>,
    pub at_millis: u64,
}

/// One repaint bit and at most one latest progress value per active owner.
#[derive(Clone)]
pub struct Notifications {
    repaint: watch::Sender<u64>,
    progress: Arc<Mutex<HashMap<OwnerId, Progress>>>,
    clock: Arc<dyn Clock>,
}
impl Notifications {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        let (repaint, _) = watch::channel(0);
        Self {
            repaint,
            progress: Arc::default(),
            clock,
        }
    }
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.repaint.subscribe()
    }
    pub fn repaint(&self) {
        self.repaint
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
    pub fn progress(
        &self,
        owner: OwnerId,
        tag: JobTag,
        message: String,
        completed: u64,
        total: Option<u64>,
    ) {
        let mut progress = self.progress.lock().expect("progress mutex");
        if !progress.contains_key(&owner) && progress.len() >= PROGRESS_CAPACITY {
            return;
        }
        progress.insert(
            owner,
            Progress {
                tag,
                message: message.chars().take(512).collect(),
                completed,
                total,
                at_millis: self.clock.now_millis(),
            },
        );
        drop(progress);
        self.repaint();
    }
    pub fn take_progress(&self) -> HashMap<OwnerId, Progress> {
        std::mem::take(&mut *self.progress.lock().expect("progress mutex"))
    }
    pub fn dispose(&self, owner: OwnerId) {
        self.progress.lock().expect("progress mutex").remove(&owner);
    }
}

/// Owned by the application thread. All submitted inputs/results must be owned.
pub struct Scheduler<T: Send + 'static> {
    sender: mpsc::Sender<Completion<T>>,
    receiver: mpsc::Receiver<Completion<T>>,
    tasks: JoinSet<()>,
    active: HashMap<JobTag, (OwnerId, CancellationToken)>,
    async_slots: Arc<Semaphore>,
    blocking_slots: Arc<Semaphore>,
    notifications: Notifications,
}

impl<T: Send + 'static> Scheduler<T> {
    pub fn new(notifications: Notifications) -> Self {
        Self::with_capacity(
            notifications,
            COMPLETION_CAPACITY,
            ASYNC_CAPACITY,
            BLOCKING_CAPACITY,
        )
    }
    pub fn with_capacity(
        notifications: Notifications,
        queue: usize,
        asynchronous: usize,
        blocking: usize,
    ) -> Self {
        assert!(queue > 0 && asynchronous > 0 && blocking > 0);
        let (sender, receiver) = mpsc::channel(queue);
        Self {
            sender,
            receiver,
            tasks: JoinSet::new(),
            active: HashMap::new(),
            async_slots: Arc::new(Semaphore::new(asynchronous)),
            blocking_slots: Arc::new(Semaphore::new(blocking)),
            notifications,
        }
    }
    pub fn submit<F, Fut>(
        &mut self,
        owner: OwnerId,
        tag: JobTag,
        delivery: Delivery,
        work: F,
    ) -> Result<()>
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        while self.tasks.try_join_next().is_some() {}
        self.active.retain(|_, (_, token)| !token.is_cancelled());
        if self.active.contains_key(&tag) {
            return Err(DomainError::Unsupported("duplicate active job tag".into()));
        }
        let permit = self
            .async_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| DomainError::QueueFull)?;
        let token = CancellationToken::new();
        self.active.insert(tag, (owner, token.clone()));
        let sender = self.sender.clone();
        let notify = self.notifications.clone();
        self.tasks.spawn(async move {
            let future = AssertUnwindSafe(async { work(token.clone()).await }).catch_unwind();
            let result = if delivery == Delivery::Cancellable {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => Err(DomainError::Cancelled),
                    output = future => output.unwrap_or_else(|_| Err(DomainError::Unsupported("worker panicked".into()))),
                }
            } else {
                future.await.unwrap_or_else(|_| Err(DomainError::Unsupported("worker panicked".into())))
            };
            let completion = Completion { tag, owner, result };
            if delivery == Delivery::Critical {
                if sender.send(completion).await.is_ok() { notify.repaint(); }
            } else {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => {},
                    sent = sender.send(completion) => { if sent.is_ok() { notify.repaint(); } },
                }
            }
            drop(permit);
        });
        Ok(())
    }
    pub fn submit_blocking<F>(
        &mut self,
        owner: OwnerId,
        tag: JobTag,
        delivery: Delivery,
        work: F,
    ) -> Result<()>
    where
        F: FnOnce(CancellationToken) -> Result<T> + Send + 'static,
    {
        let slot = self
            .blocking_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| DomainError::QueueFull)?;
        // Once launched, a blocking worker is always joined. Dropping its future
        // would leave the closure running after its owner or workspace is gone.
        self.submit(owner, tag, Delivery::Critical, move |token| async move {
            let cancelled = token.clone();
            let output = tokio::task::spawn_blocking(move || {
                let _slot = slot;
                if token.is_cancelled() {
                    return Err(DomainError::Cancelled);
                }
                work(token)
            })
            .await
            .map_err(|error| {
                DomainError::Unsupported(format!("blocking worker failed: {error}"))
            })?;
            if delivery == Delivery::Cancellable && cancelled.is_cancelled() {
                Err(DomainError::Cancelled)
            } else {
                output
            }
        })
    }
    pub fn cancel_owner(&mut self, owner: OwnerId) {
        self.active.retain(|_, (job_owner, token)| {
            if *job_owner == owner {
                token.cancel();
                false
            } else {
                true
            }
        });
        self.notifications.dispose(owner);
    }
    pub fn cancel_workspace(&mut self, workspace: WorkspaceGeneration) {
        self.active.retain(|tag, (_, token)| {
            if tag.workspace == workspace {
                token.cancel();
                false
            } else {
                true
            }
        });
    }
    pub async fn next(&mut self) -> Option<Completion<T>> {
        let output = self.receiver.recv().await;
        if let Some(completion) = &output {
            self.active.remove(&completion.tag);
        }
        output
    }
    pub fn try_next(&mut self) -> Option<Completion<T>> {
        let output = self.receiver.try_recv().ok()?;
        self.active.remove(&output.tag);
        Some(output)
    }
    /// Cancels owners, drains all critical completions and joins every worker.
    /// The caller applies/persists these completions before restoring the terminal.
    pub async fn shutdown(mut self) -> Vec<Completion<T>> {
        for (_, token) in self.active.values() {
            token.cancel();
        }
        drop(self.sender);
        let mut output = Vec::new();
        while let Some(completion) = self.receiver.recv().await {
            output.push(completion);
        }
        while self.tasks.join_next().await.is_some() {}
        output
    }
}

impl<T: Send + 'static> WorkerScheduler<T> for Scheduler<T> {
    fn schedule(
        &mut self,
        owner: OwnerId,
        tag: JobTag,
        delivery: Delivery,
        work: JobFactory<T>,
    ) -> Result<()> {
        self.submit(owner, tag, delivery, work)
    }
    fn dispose_owner(&mut self, owner: OwnerId) {
        self.cancel_owner(owner);
    }
    fn schedule_blocking(
        &mut self,
        owner: OwnerId,
        tag: JobTag,
        delivery: Delivery,
        work: BlockingJobFactory<T>,
    ) -> Result<()> {
        self.submit_blocking(owner, tag, delivery, work)
    }
    fn dispose_workspace(&mut self, workspace: WorkspaceGeneration) {
        self.cancel_workspace(workspace);
    }
}
