use kode_platform::metrics::ProcessSampler;
#[test]
fn rust_process_metrics_have_resident_memory_and_delta_cpu_without_java() {
    let mut sampler = ProcessSampler::default();
    let first = sampler.sample();
    assert!(
        first.cpu_percent.is_none(),
        "first CPU sample has no elapsed baseline"
    );
    if sysinfo::IS_SUPPORTED_SYSTEM {
        assert!(first.resident_bytes.is_some_and(|bytes| bytes > 0));
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_millis(70) {
            std::hint::black_box((0_u64..1000).sum::<u64>());
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
        let cpu = sampler.sample().cpu_percent.unwrap();
        assert!(cpu.is_finite() && cpu > 0.0 && cpu <= 100.0, "CPU: {cpu}");
    }
}
