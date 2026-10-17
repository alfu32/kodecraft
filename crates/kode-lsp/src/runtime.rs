//! Bounded prerequisite probes on the existing Tokio runtime and managed children.
use crate::{
    catalog::invalid,
    package::{RuntimeKind, RuntimeRequirement, version},
};
use kode_core::{DomainError, Result};
use kode_platform::{
    jobs::CancellationToken,
    process::{ProcessFactory, ProcessOutput, ProcessSpec, run_capture},
};
use std::{path::Path, time::Duration};

/// Blocking-worker entry point, like HttpFetcher. It never creates a runtime.
pub(crate) fn verify_with_factory(
    program: &Path,
    requirement: &RuntimeRequirement,
    cancel: &CancellationToken,
    factory: &dyn ProcessFactory,
) -> Result<String> {
    let mut spec = ProcessSpec::new(program);
    spec.args = vec![
        match requirement.kind {
            RuntimeKind::Node => "--version",
            RuntimeKind::Java => "-version",
        }
        .into(),
    ];
    let output = capture(
        spec,
        &format!("{} version check", requirement.kind.label()),
        cancel,
        factory,
    )?;
    parse_version(output, requirement)
}
pub(crate) fn capture(
    spec: ProcessSpec,
    label: &str,
    cancel: &CancellationToken,
    factory: &dyn ProcessFactory,
) -> Result<ProcessOutput> {
    let runtime = tokio::runtime::Handle::try_current()
        .map_err(|_| invalid("Runtime verification requires the application's blocking worker"))?;
    runtime.block_on(probe(spec, label, cancel, factory))
}
async fn probe(
    spec: ProcessSpec,
    label: &str,
    cancel: &CancellationToken,
    factory: &dyn ProcessFactory,
) -> Result<ProcessOutput> {
    if cancel.is_cancelled() {
        return Err(DomainError::Cancelled);
    }
    let local = CancellationToken::new();
    let work = run_capture(spec, vec![], local.clone(), factory);
    tokio::pin!(work);
    let output = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            local.cancel();
            let _ = work.await;
            return Err(DomainError::Cancelled);
        },
        _ = tokio::time::sleep(Duration::from_secs(5)) => {
            local.cancel();
            let _ = work.await;
            return Err(invalid(format!("{label} timed out; check the configured runtime and retry")));
        },
        result = &mut work => result?,
    };
    Ok(output)
}
fn parse_version(output: ProcessOutput, requirement: &RuntimeRequirement) -> Result<String> {
    let text = match requirement.kind {
        RuntimeKind::Node => std::str::from_utf8(&output.stdout)
            .ok()
            .and_then(|text| text.trim().strip_prefix('v')),
        RuntimeKind::Java => output.stderr.lines().find_map(|line| {
            line.split_once("version \"")
                .and_then(|(_, tail)| tail.split('"').next())
        }),
    }
    .filter(|text| text.len() <= 64)
    .ok_or_else(|| {
        invalid(format!(
            "Cannot read {} version; {} or newer is required",
            requirement.kind.label(),
            requirement.minimum,
        ))
    })?;
    let numeric = text
        .split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .next()
        .unwrap_or("");
    let parts: Vec<_> = numeric.split('.').collect();
    let normalized = match parts.len() {
        1 => format!("{numeric}.0.0"),
        2 => format!("{numeric}.0"),
        _ => parts[..3].join("."),
    };
    let actual = version(&normalized).map_err(|_| {
        invalid(format!(
            "Cannot read {} version; {} or newer is required",
            requirement.kind.label(),
            requirement.minimum,
        ))
    })?;
    if !output.status.success() || output.truncated || actual < version(&requirement.minimum)? {
        return Err(invalid(format!(
            "{} {} or newer is required; found {}. Install a supported runtime or select a local server executable.",
            requirement.kind.label(),
            requirement.minimum,
            text,
        )));
    }
    Ok(text.into())
}
