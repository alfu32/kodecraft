//! Typed adapters let every feature share the same bounded application scheduler.
use kode_core::{JobTag, Result, WorkspaceGeneration};
use kode_platform::jobs::{
    BlockingJobFactory, Delivery, JobFactory, OwnerId, Scheduler, WorkerScheduler,
};

pub struct MappedScheduler<'a, T, U: Send + 'static> {
    scheduler: &'a mut Scheduler<U>,
    map: fn(T) -> U,
}
impl<'a, T, U: Send + 'static> MappedScheduler<'a, T, U> {
    pub fn new(scheduler: &'a mut Scheduler<U>, map: fn(T) -> U) -> Self {
        Self { scheduler, map }
    }
}
impl<T: Send + 'static, U: Send + 'static> WorkerScheduler<T> for MappedScheduler<'_, T, U> {
    fn schedule(
        &mut self,
        owner: OwnerId,
        tag: JobTag,
        delivery: Delivery,
        work: JobFactory<T>,
    ) -> Result<()> {
        let map = self.map;
        self.scheduler
            .submit(owner, tag, delivery, move |cancel| async move {
                work(cancel).await.map(map)
            })
    }
    fn schedule_blocking(
        &mut self,
        owner: OwnerId,
        tag: JobTag,
        delivery: Delivery,
        work: BlockingJobFactory<T>,
    ) -> Result<()> {
        let map = self.map;
        self.scheduler
            .submit_blocking(owner, tag, delivery, move |cancel| work(cancel).map(map))
    }
    fn dispose_owner(&mut self, owner: OwnerId) {
        self.scheduler.cancel_owner(owner);
    }
    fn dispose_workspace(&mut self, workspace: WorkspaceGeneration) {
        self.scheduler.cancel_workspace(workspace);
    }
}
