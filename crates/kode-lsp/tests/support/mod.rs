pub fn python() -> String {
    std::env::var("KODE_TEST_PYTHON")
        .unwrap_or_else(|_| if cfg!(windows) { "python" } else { "python3" }.into())
}
