pub fn running(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new("/proc").join(pid.to_string()).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let python = std::env::var("KODE_TEST_PYTHON")
            .unwrap_or_else(|_| if cfg!(windows) { "python" } else { "python3" }.into());
        let result = std::process::Command::new(python)
            .args(["-c", include_str!("process.py"), &pid.to_string()])
            .output()
            .expect("query native child process state");
        match result.status.code() {
            Some(0) => true,
            Some(1) => false,
            _ => panic!(
                "native process probe failed: {}",
                String::from_utf8_lossy(&result.stderr)
            ),
        }
    }
}
