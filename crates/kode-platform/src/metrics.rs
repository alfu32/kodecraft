//! Sample only this Rust process, on a blocking worker, without Java or children.
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
#[derive(Debug, Clone, Default)]
pub struct ProcessSample {
    pub resident_bytes: Option<u64>,
    /// Normalized across the process's available logical CPUs, as in Kode's JVM tracker.
    pub cpu_percent: Option<f32>,
}
pub struct ProcessSampler {
    system: System,
    pid: Pid,
    previous: Option<(std::time::Instant, u64)>,
}
impl Default for ProcessSampler {
    fn default() -> Self {
        Self {
            system: System::new(),
            pid: Pid::from_u32(std::process::id()),
            previous: None,
        }
    }
}
impl ProcessSampler {
    pub fn sample(&mut self) -> ProcessSample {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self.pid]),
            true,
            ProcessRefreshKind::nothing()
                .without_tasks()
                .with_cpu()
                .with_memory(),
        );
        let now = std::time::Instant::now();
        let sample = self
            .system
            .process(self.pid)
            .map(|process| ProcessSample {
                resident_bytes: Some(process.memory()),
                cpu_percent: self.previous.map(|(before, cpu_ms)| {
                    let cores = std::thread::available_parallelism()
                        .map_or(1, usize::from)
                        .max(1);
                    let elapsed = now.duration_since(before).as_secs_f64().max(f64::EPSILON);
                    (process.accumulated_cpu_time().saturating_sub(cpu_ms) as f64
                        / elapsed
                        / 10.0
                        / cores as f64)
                        .clamp(0.0, 100.0) as f32
                }),
            })
            .unwrap_or_default();
        self.previous = self
            .system
            .process(self.pid)
            .map(|process| (now, process.accumulated_cpu_time()));
        sample
    }
}
