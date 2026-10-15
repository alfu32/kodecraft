use crate::model::{DriverSpec, Extra, JdbcDriverArtifact};
use kode_core::{DomainError, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
fn failure(message: impl Into<String>) -> DomainError {
    DomainError::Parse {
        format: "JDBC catalog",
        message: message.into(),
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DriverDefinition {
    pub id: String,
    pub display_name: String,
    pub database_family: String,
    pub driver_class: String,
    pub default_maven_coordinates: Option<String>,
    pub default_jdbc_url_template: String,
    pub url_prefixes: Vec<String>,
    pub artifact: Option<JdbcDriverArtifact>,
    pub alias_of: Option<String>,
    pub jdbc_url_templates: Vec<UrlTemplate>,
    #[serde(flatten)]
    pub extra: Extra,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistryData {
    version: u32,
    drivers: Vec<DriverDefinition>,
}
#[derive(Debug, Clone)]
pub struct DriverRegistry {
    definitions: Vec<DriverDefinition>,
}
impl DriverRegistry {
    pub fn compiled() -> Result<Self> {
        Self::from_json(include_bytes!("../../../resources/database/drivers.json"))
    }
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > 2 * 1024 * 1024 {
            return Err(failure("driver registry exceeds 2 MiB"));
        }
        let data: RegistryData =
            serde_json::from_slice(bytes).map_err(|_| failure("malformed driver registry"))?;
        if data.version != 1 || data.drivers.len() > 256 {
            return Err(failure("unsupported registry version or entry count"));
        }
        let mut ids = BTreeSet::new();
        for entry in &data.drivers {
            if crate::security::contains_secret(&entry.default_jdbc_url_template) {
                return Err(failure("credential-bearing default driver URL"));
            }
            if entry.id.is_empty() || !ids.insert(entry.id.to_ascii_lowercase()) {
                return Err(failure("duplicate or empty driver ID"));
            }
            if entry
                .url_prefixes
                .iter()
                .any(|prefix| !prefix.to_ascii_lowercase().starts_with("jdbc:"))
            {
                return Err(failure("driver prefix must start with jdbc:"));
            }
        }
        let registry = Self {
            definitions: data.drivers,
        };
        for definition in registry.definitions() {
            registry.provider(&definition.id, &mut BTreeSet::new())?;
        }
        Ok(registry)
    }
    pub fn definitions(&self) -> &[DriverDefinition] {
        &self.definitions
    }
    pub fn by_id(&self, id: &str) -> Option<&DriverDefinition> {
        self.definitions
            .iter()
            .find(|definition| definition.id.eq_ignore_ascii_case(id))
    }
    pub fn by_url(&self, url: &str) -> Option<&DriverDefinition> {
        self.definitions.iter().find(|definition| {
            definition.url_prefixes.iter().any(|prefix| {
                url.get(..prefix.len())
                    .is_some_and(|value| value.eq_ignore_ascii_case(prefix))
            })
        })
    }
    fn provider<'a>(
        &'a self,
        id: &str,
        visiting: &mut BTreeSet<String>,
    ) -> Result<&'a DriverDefinition> {
        let definition = self
            .by_id(id)
            .ok_or_else(|| failure("driver alias points to an unknown provider"))?;
        if !visiting.insert(definition.id.clone()) {
            return Err(failure("driver alias cycle"));
        }
        if let Some(alias) = &definition.alias_of {
            self.provider(alias, visiting)
        } else {
            Ok(definition)
        }
    }
    pub fn spec_for(&self, id: &str) -> Option<DriverSpec> {
        let definition = self.by_id(id)?;
        let provider = self.provider(id, &mut BTreeSet::new()).ok()?;
        let artifact = definition
            .artifact
            .as_ref()
            .or(provider.artifact.as_ref())
            .cloned();
        Some(DriverSpec {
            id: definition.id.clone(),
            display_name: definition.display_name.clone(),
            driver_class: if definition.driver_class.trim().is_empty() {
                provider.driver_class.clone()
            } else {
                definition.driver_class.clone()
            },
            coordinates: definition
                .default_maven_coordinates
                .as_ref()
                .or(provider.default_maven_coordinates.as_ref())
                .cloned()
                .or_else(|| artifact.as_ref().map(JdbcDriverArtifact::coordinate_label)),
            artifact,
            provider_id: Some(provider.id.clone()),
            jar_path: None,
            extra: Extra::new(),
        })
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UrlTemplate {
    pub id: String,
    pub label: String,
    pub template: String,
    pub description: Option<String>,
    pub advanced: bool,
    pub legacy: bool,
    pub min_driver_version: Option<String>,
    pub note: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DriverTemplates {
    pub driver_id: String,
    pub recent_family: Option<String>,
    pub default_port: Option<u16>,
    pub inherit: Option<String>,
    pub prepend: bool,
    pub templates: Vec<UrlTemplate>,
    pub note: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}
#[derive(Debug, Deserialize)]
struct UrlCatalogData {
    version: u32,
    drivers: Vec<DriverTemplates>,
}
#[derive(Debug, Clone)]
pub struct UrlCatalog {
    entries: BTreeMap<String, DriverTemplates>,
    effective: BTreeMap<String, Vec<UrlTemplate>>,
    families: BTreeMap<String, String>,
}
impl UrlCatalog {
    pub fn compiled(registry: &DriverRegistry) -> Result<Self> {
        Self::from_json(
            include_bytes!("../../../resources/database/jdbc-url-templates.json"),
            registry,
        )
    }
    pub fn from_json(bytes: &[u8], registry: &DriverRegistry) -> Result<Self> {
        if bytes.len() > 2 * 1024 * 1024 {
            return Err(failure("URL catalog exceeds 2 MiB"));
        }
        let data: UrlCatalogData =
            serde_json::from_slice(bytes).map_err(|_| failure("malformed URL template catalog"))?;
        if data.version != 1 || data.drivers.len() > 256 {
            return Err(failure("unsupported URL catalog version or entry count"));
        }
        let mut entries = BTreeMap::new();
        for entry in data.drivers {
            if entries.insert(entry.driver_id.clone(), entry).is_some() {
                return Err(failure("duplicate URL driver entry"));
            }
        }
        let expected: BTreeSet<_> = registry
            .definitions()
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        if entries.keys().map(String::as_str).collect::<BTreeSet<_>>() != expected {
            return Err(failure(
                "URL catalog must cover every registry driver exactly",
            ));
        }
        let mut catalog = Self {
            entries,
            effective: BTreeMap::new(),
            families: BTreeMap::new(),
        };
        for id in catalog.entries.keys().cloned().collect::<Vec<_>>() {
            let templates = catalog.resolve(&id, registry, &mut BTreeSet::new())?;
            let mut ids = BTreeSet::new();
            for template in &templates {
                let lower = template.template.to_ascii_lowercase();
                if !ids.insert(template.id.clone())
                    || !lower.starts_with("jdbc:")
                    || template.template.trim().is_empty()
                {
                    return Err(failure("duplicate/invalid JDBC URL template"));
                }
                if [
                    "{password}",
                    "{token}",
                    "{secret}",
                    "{accesskey}",
                    "{apikey}",
                    "trustservercertificate=true",
                    "sslverification=none",
                    "nocertcheck",
                    "tlsallowinvalidhostnames=true",
                    "validateservercertificate=0",
                ]
                .iter()
                .any(|unsafe_value| lower.contains(unsafe_value))
                    || crate::security::contains_secret(&template.template)
                {
                    return Err(failure(
                        "sensitive placeholder or TLS bypass in JDBC URL template",
                    ));
                }
            }
            catalog.effective.insert(id.clone(), templates);
            let family = catalog.family(&id, registry, &mut BTreeSet::new())?;
            catalog.families.insert(id, family);
        }
        Ok(catalog)
    }
    fn resolve(
        &self,
        id: &str,
        registry: &DriverRegistry,
        visiting: &mut BTreeSet<String>,
    ) -> Result<Vec<UrlTemplate>> {
        let entry = self
            .entries
            .get(id)
            .ok_or_else(|| failure("unknown URL inheritance driver"))?;
        if !visiting.insert(id.into()) {
            return Err(failure("URL inheritance cycle"));
        }
        let mut inherited = if let Some(parent) = &entry.inherit {
            self.resolve(parent, registry, visiting)?
        } else {
            vec![]
        };
        let mut own = entry.templates.clone();
        if let Some(definition) = registry.by_id(id) {
            own.extend(definition.jdbc_url_templates.clone());
        }
        if inherited.len().saturating_add(own.len()) > 5_000 {
            return Err(failure("effective URL template count exceeds 5,000"));
        }
        if entry.prepend {
            own.extend(inherited);
            Ok(own)
        } else {
            inherited.extend(own);
            Ok(inherited)
        }
    }
    fn family(
        &self,
        id: &str,
        registry: &DriverRegistry,
        visiting: &mut BTreeSet<String>,
    ) -> Result<String> {
        if !visiting.insert(id.into()) {
            return Err(failure("recent family cycle"));
        }
        let entry = self.entries.get(id);
        if let Some(family) = entry.and_then(|entry| entry.recent_family.clone()) {
            return Ok(family);
        }
        let parent = entry
            .and_then(|entry| entry.inherit.as_deref())
            .or_else(|| {
                registry
                    .by_id(id)
                    .and_then(|definition| definition.alias_of.as_deref())
            });
        if let Some(parent) = parent {
            self.family(parent, registry, visiting)
        } else {
            Ok(id.into())
        }
    }
    pub fn entry(&self, id: &str) -> Option<&DriverTemplates> {
        self.entries.get(id)
    }
    pub fn templates(&self, id: &str) -> &[UrlTemplate] {
        self.effective.get(id).map_or(&[], Vec::as_slice)
    }
    pub fn recent_family<'a>(&'a self, id: &'a str) -> &'a str {
        self.families.get(id).map_or(id, String::as_str)
    }
}
