//! Runs in its own process because it changes libgit2's global search path.
#![cfg(unix)]
use git2::{Config, ConfigLevel};
use kode_git::repository::skip_unreadable_system_config;
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn only_an_unreadable_system_config_is_skipped() {
    let system = tempfile::tempdir().unwrap();
    let config = system.path().join("gitconfig");
    fs::write(&config, "[core]\n").unwrap();
    // SAFETY: this test is the only code in its process using libgit2.
    #[allow(unsafe_code)]
    unsafe { git2::opts::set_search_path(ConfigLevel::System, system.path()) }.unwrap();

    skip_unreadable_system_config();
    assert_eq!(Config::find_system().unwrap(), config);

    fs::set_permissions(&config, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(&config).is_ok() {
        return; // Root can read anything, so it can't exercise the denial.
    }
    skip_unreadable_system_config();
    assert!(Config::find_system().is_err());
}
