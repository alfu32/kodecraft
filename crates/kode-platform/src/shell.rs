//! Workspace shells inherit the real terminal after its owner suspends raw input.
use std::{
    ffi::OsString,
    io,
    path::Path,
    process::{Command, ExitStatus, Stdio},
};
pub fn selected_shell() -> OsString {
    std::env::var_os(if cfg!(windows) { "COMSPEC" } else { "SHELL" })
        .filter(|value| !value.to_string_lossy().trim().is_empty())
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "cmd.exe".into()
            } else {
                "/bin/bash".into()
            }
        })
}
/// The selected executable is one argument, including paths containing spaces.
/// No command string is evaluated, and no shell startup flags are invented.
pub fn launch(program: &std::ffi::OsStr, root: &Path) -> io::Result<ExitStatus> {
    Command::new(program)
        .current_dir(root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
}
