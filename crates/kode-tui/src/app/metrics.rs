//! Main-thread frame counters; OS sampling stays in the bounded platform workers.
use kode_platform::metrics::ProcessSample;
#[derive(Debug, Clone, Default)]
pub struct Metrics {
    pub loop_fps: f64,
    pub draw_fps: f64,
    pub process: ProcessSample,
}
impl Metrics {
    pub fn label(&self) -> String {
        let memory = self
            .process
            .resident_bytes
            .map_or_else(|| "--".into(), |bytes| format!("{}MiB", bytes / 1_048_576));
        let cpu = self
            .process
            .cpu_percent
            .map_or_else(|| "--".into(), |cpu| format!("{cpu:.1}%"));
        format!(
            "Loop:{:.0} Draw:{:.0} RSS:{memory} CPU:{cpu}",
            self.loop_fps, self.draw_fps
        )
    }
}
pub struct FrameCounter {
    since: std::time::Instant,
    loops: u64,
    draws: u64,
}
impl Default for FrameCounter {
    fn default() -> Self {
        Self {
            since: std::time::Instant::now(),
            loops: 0,
            draws: 0,
        }
    }
}
impl FrameCounter {
    pub fn loop_tick(&mut self) {
        self.loops += 1;
    }
    pub fn drawn(&mut self) {
        self.draws += 1;
    }
    pub fn update(&mut self, metrics: &mut Metrics) -> bool {
        let elapsed = self.since.elapsed();
        if elapsed < std::time::Duration::from_secs(1) {
            return false;
        }
        metrics.loop_fps = self.loops as f64 / elapsed.as_secs_f64();
        metrics.draw_fps = self.draws as f64 / elapsed.as_secs_f64();
        self.loops = 0;
        self.draws = 0;
        self.since = std::time::Instant::now();
        true
    }
}
