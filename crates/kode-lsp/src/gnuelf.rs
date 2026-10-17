//! Validate a private GNU ELF closure without running the server's main function.
use crate::{catalog::invalid, discovery::ResolvedCommand};
use kode_core::Result;
use kode_platform::{jobs::CancellationToken, process::ProcessFactory};
use std::path::Path;

pub(crate) fn prepare(mut command: ResolvedCommand) -> Result<ResolvedCommand> {
    let root = command
        .cwd
        .as_deref()
        .ok_or_else(|| invalid("GNU ELF launcher needs an installed directory"))?;
    for argument in command.args.iter_mut().skip(5) {
        if let Some(resource) = argument.strip_prefix("--resource-dir=") {
            let relative = crate::catalog::relative_path(resource)?;
            if relative.as_os_str().is_empty() {
                return Err(invalid("Native resource directory is empty"));
            }
            let path = root
                .join(relative)
                .canonicalize()
                .map_err(|e| invalid(format!("Native resource directory is missing: {e}")))?;
            if !path.is_dir()
                || !path.starts_with(root.canonicalize().map_err(|e| invalid(e.to_string()))?)
            {
                return Err(invalid(
                    "Native resources must be inside the installed server",
                ));
            }
            *argument = format!(
                "--resource-dir={}",
                path.to_str()
                    .ok_or_else(|| invalid("Native resource directory is not UTF-8"))?
            );
        }
    }
    Ok(command)
}

pub(crate) fn verify(
    command: &ResolvedCommand,
    cancel: &CancellationToken,
    factory: &dyn ProcessFactory,
) -> Result<()> {
    if command.args.len() < 5
        || command.args[1] != "--inhibit-cache"
        || command.args[2] != "--library-path"
        || command.args[3].is_empty()
    {
        return Err(invalid(
            "GNU ELF launcher requires explicit private library paths",
        ));
    }
    let root = command
        .cwd
        .as_deref()
        .ok_or_else(|| invalid("GNU ELF launcher needs an installed directory"))?;
    let executable = crate::catalog::relative_path(&command.args[4])?;
    if executable.as_os_str().is_empty() || !root.join(executable).is_file() {
        return Err(invalid("GNU ELF server executable is missing"));
    }
    let mut spec = command.process_spec();
    // In --list mode glibc interprets trailing operands as symbol names.
    spec.args.truncate(4);
    spec.args.insert(0, "--list".into());
    let output = crate::runtime::capture(spec, "Native library check", cancel, factory)?;
    if !output.status.success() || output.truncated {
        return Err(invalid(format!(
            "Native library check failed; reinstall the language server or configure a local executable. {}",
            output.stderr.trim()
        )));
    }
    let text = std::str::from_utf8(&output.stdout)
        .map_err(|_| invalid("Invalid native library check output"))?;
    let root = root
        .canonicalize()
        .map_err(|e| invalid(format!("Cannot inspect native install: {e}")))?;
    let mut found = false;
    for line in text.lines() {
        if let Some((_, target)) = line.split_once(" => ") {
            found = true;
            let target = target
                .rsplit_once(" (0x")
                .map_or(target, |(path, _)| path)
                .trim();
            let resolved = Path::new(target)
                .canonicalize()
                .map_err(|e| invalid(format!("Native dependency is missing: {target}: {e}")))?;
            if !resolved.starts_with(&root) {
                return Err(invalid(format!(
                    "Native dependency resolves outside the language server install: {}",
                    resolved.display()
                )));
            }
        }
    }
    if !found {
        return Err(invalid(
            "Native library check returned no dependency mappings",
        ));
    }
    Ok(())
}
