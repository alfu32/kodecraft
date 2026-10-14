use kode_core::{DocumentId, DomainError, JobTag, RequestId, Revision, WorkspaceGeneration};
use kode_platform::jobs::{Clock, Delivery, Notifications, OwnerId, PROGRESS_CAPACITY, Scheduler};
use std::{sync::Arc, time::Duration};

struct FakeClock;
impl Clock for FakeClock {
    fn now_millis(&self) -> u64 {
        42
    }
}
fn tag(id: u64) -> JobTag {
    JobTag {
        workspace: WorkspaceGeneration(1),
        request: RequestId(id),
        document: Some((DocumentId(1), Revision(1))),
    }
}
fn notifications() -> Notifications {
    Notifications::new(Arc::new(FakeClock))
}

#[tokio::test(start_paused = true)]
async fn delayed_a_fast_b_and_idle_repaint() {
    let notify = notifications();
    let mut repaint = notify.subscribe();
    let mut scheduler = Scheduler::new(notify);
    scheduler
        .submit(OwnerId(1), tag(1), Delivery::Cancellable, |_| async {
            tokio::time::sleep(Duration::from_secs(20)).await;
            Ok("A")
        })
        .unwrap();
    scheduler
        .submit(OwnerId(1), tag(2), Delivery::Cancellable, |_| async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            Ok("B")
        })
        .unwrap();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(2)).await;
    repaint.changed().await.unwrap();
    let b = scheduler.next().await.unwrap();
    assert_eq!(b.result.unwrap(), "B");
    assert!(b.tag.is_current(tag(2)));
    tokio::time::advance(Duration::from_secs(20)).await;
    let a = scheduler.next().await.unwrap();
    assert_eq!(a.result.unwrap(), "A");
    assert!(!a.tag.is_current(tag(2)));
    assert!(scheduler.shutdown().await.is_empty());
}

#[tokio::test]
async fn saturated_queue_keeps_all_saves_and_shutdown_drains() {
    let mut scheduler = Scheduler::with_capacity(notifications(), 1, 4, 1);
    for id in 1..=3 {
        scheduler
            .submit(
                OwnerId(1),
                tag(id),
                Delivery::Critical,
                move |_| async move { Ok(id) },
            )
            .unwrap();
    }
    scheduler
        .submit(OwnerId(2), tag(4), Delivery::Cancellable, |_| async {
            Ok(4)
        })
        .unwrap();
    assert!(matches!(
        scheduler.submit(OwnerId(2), tag(5), Delivery::Critical, |_| async { Ok(5) }),
        Err(DomainError::QueueFull)
    ));
    tokio::task::yield_now().await;
    scheduler.cancel_owner(OwnerId(2));
    let mut results: Vec<_> = tokio::time::timeout(Duration::from_secs(2), scheduler.shutdown())
        .await
        .unwrap()
        .into_iter()
        .map(|completion| completion.result.unwrap())
        .collect();
    results.sort();
    assert_eq!(results, vec![1, 2, 3]);
}

#[tokio::test]
async fn workspace_disposal_cancels_cooperatively_and_reports_critical_failures() {
    let mut scheduler: Scheduler<()> = Scheduler::new(notifications());
    scheduler
        .submit(OwnerId(1), tag(1), Delivery::Critical, |token| async move {
            token.cancelled().await;
            Err(DomainError::Cancelled)
        })
        .unwrap();
    scheduler
        .submit(OwnerId(2), tag(2), Delivery::Cancellable, |_| async {
            std::future::pending().await
        })
        .unwrap();
    scheduler.cancel_workspace(WorkspaceGeneration(1));
    let results = scheduler.shutdown().await;
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0].result, Err(DomainError::Cancelled)));
}

#[tokio::test]
async fn blocking_pool_is_bounded_and_shutdown_joins_the_closure() {
    let mut scheduler = Scheduler::with_capacity(notifications(), 1, 4, 1);
    let (entered, ready) = tokio::sync::oneshot::channel();
    scheduler
        .submit_blocking(OwnerId(1), tag(1), Delivery::Cancellable, move |token| {
            entered.send(()).unwrap();
            while !token.is_cancelled() {
                std::thread::yield_now();
            }
            Ok(7)
        })
        .unwrap();
    ready.await.unwrap();
    assert!(matches!(
        scheduler.submit_blocking(OwnerId(1), tag(2), Delivery::Critical, |_| Ok(8)),
        Err(DomainError::QueueFull)
    ));
    let results = scheduler.shutdown().await;
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0].result, Err(DomainError::Cancelled)));
}

#[tokio::test]
async fn worker_panic_is_a_visible_error_and_releases_capacity() {
    let mut scheduler: Scheduler<()> = Scheduler::new(notifications());
    scheduler
        .submit(OwnerId(1), tag(1), Delivery::Critical, |_| async {
            panic!("deliberate worker panic")
        })
        .unwrap();
    assert!(matches!(
        scheduler.next().await.unwrap().result,
        Err(DomainError::Unsupported(_))
    ));
    assert!(scheduler.shutdown().await.is_empty());
}

#[test]
fn progress_coalesces_is_bounded_and_uses_injected_clock() {
    let notify = notifications();
    for id in 0..1000 {
        notify.progress(OwnerId(1), tag(1), id.to_string(), id, Some(1000));
    }
    let status = notify.take_progress();
    assert_eq!(status.len(), 1);
    assert_eq!(status[&OwnerId(1)].completed, 999);
    assert_eq!(status[&OwnerId(1)].at_millis, 42);
    for owner in 0..1000 {
        notify.progress(OwnerId(owner), tag(1), "update".into(), 1, None);
    }
    assert_eq!(notify.take_progress().len(), PROGRESS_CAPACITY);
}

#[tokio::test]
async fn coordinator_can_inject_a_deterministic_worker_scheduler() {
    use kode_platform::jobs::{CancellationToken, JobFactory, WorkerScheduler};
    struct FakeScheduler {
        pending: Vec<JobFactory<u64>>,
    }
    impl WorkerScheduler<u64> for FakeScheduler {
        fn schedule(
            &mut self,
            _: OwnerId,
            _: JobTag,
            _: Delivery,
            work: JobFactory<u64>,
        ) -> kode_core::Result<()> {
            self.pending.push(work);
            Ok(())
        }
        fn dispose_owner(&mut self, _: OwnerId) {
            self.pending.clear();
        }
        fn dispose_workspace(&mut self, _: WorkspaceGeneration) {
            self.pending.clear();
        }
    }
    let mut scheduler = FakeScheduler { pending: vec![] };
    let boundary: &mut dyn WorkerScheduler<u64> = &mut scheduler;
    boundary
        .schedule(
            OwnerId(1),
            tag(1),
            Delivery::Critical,
            Box::new(|_| Box::pin(async { Ok(42) })),
        )
        .unwrap();
    assert_eq!(
        scheduler.pending.pop().unwrap()(CancellationToken::new())
            .await
            .unwrap(),
        42
    );
}
