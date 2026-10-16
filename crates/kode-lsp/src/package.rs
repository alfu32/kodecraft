//! Declarative dependency archives: no package-manager or lifecycle scripts run.
use crate::catalog::{Extra, invalid, relative_path};
use kode_core::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const PLAN_KEY: &str = "kodeRsPackage";
/// Installer output, retained separately so later manifest command edits are overrides.
pub const COMMAND_KEY: &str = "kodeRsInstalledCommand";
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackagePlan {
    pub version: u32,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    pub runtime: Option<RuntimeRequirement>,
    pub launcher: Option<Launcher>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Launcher {
    Jdtls,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Dependency {
    pub url: String,
    pub sha256: String,
    pub directory: String,
    pub unpack_subdir: Option<String>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeKind {
    Node,
    Java,
}
impl RuntimeKind {
    pub fn program(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Java => "java",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Node => "Node.js",
            Self::Java => "Java",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeRequirement {
    pub kind: RuntimeKind,
    pub minimum: String,
}
pub fn version(value: &str) -> Result<[u32; 3]> {
    let numbers = value
        .split('.')
        .map(str::parse::<u32>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| invalid("Runtime minimum must be a numeric major.minor.patch version"))?;
    if numbers.len() != 3 {
        return Err(invalid(
            "Runtime minimum must be a numeric major.minor.patch version",
        ));
    }
    Ok([numbers[0], numbers[1], numbers[2]])
}
pub fn url(value: &str) -> Result<()> {
    let url = reqwest::Url::parse(value).map_err(|error| invalid(error.to_string()))?;
    if value.len() > 8192
        || !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(invalid("expected HTTP(S) package URL without credentials"));
    }
    Ok(())
}
pub fn checksum(value: &str) -> Result<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid("invalid SHA-256 checksum"));
    }
    Ok(())
}
pub fn plan(extra: &Extra) -> Result<Option<PackagePlan>> {
    let Some(value) = extra.get(PLAN_KEY) else {
        return Ok(None);
    };
    let plan: PackagePlan = serde_json::from_value(value.clone())
        .map_err(|error| invalid(format!("Invalid package plan: {error}")))?;
    if plan.version != 1 || plan.dependencies.len() > 256 {
        return Err(invalid("Unsupported or oversized package plan"));
    }
    if let Some(runtime) = &plan.runtime {
        version(&runtime.minimum)?;
    }
    if plan.launcher == Some(Launcher::Jdtls)
        && plan
            .runtime
            .as_ref()
            .is_none_or(|runtime| runtime.kind != RuntimeKind::Java)
    {
        return Err(invalid(
            "JDT LS launcher requires a Java runtime requirement",
        ));
    }
    let mut directories = BTreeSet::new();
    for dependency in &plan.dependencies {
        url(&dependency.url)?;
        checksum(&dependency.sha256)?;
        let path = relative_path(&dependency.directory)?;
        if path.as_os_str().is_empty()
            || path == std::path::Path::new("installed.json")
            || !directories.insert(path)
        {
            return Err(invalid(
                "Dependency archive needs a distinct nonempty package directory",
            ));
        }
        if let Some(path) = &dependency.unpack_subdir {
            relative_path(path)?;
        }
    }
    Ok(Some(plan))
}
