use super::{ConfigLock, JsonFile, failure};
use crate::{
    catalog::{UrlCatalog, UrlTemplate},
    model::Extra,
    security,
};
use kode_core::{Result, services::DiskIdentity};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
#[derive(Deserialize, Serialize)]
struct RecentData {
    version: u32,
    #[serde(default)]
    families: BTreeMap<String, Vec<String>>,
    #[serde(flatten, serialize_with = "security::serialize_extra")]
    extra: Extra,
}
impl Default for RecentData {
    fn default() -> Self {
        Self {
            version: 1,
            families: BTreeMap::new(),
            extra: Extra::new(),
        }
    }
}
fn normalize(urls: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    urls.into_iter()
        .map(|url| url.trim().to_owned())
        .filter(|url| {
            !url.is_empty()
                && url.len() <= 64 * 1024
                && !security::contains_secret(url)
                && seen.insert(url.clone())
        })
        .take(12)
        .collect()
}
fn decode(bytes: &[u8]) -> Result<RecentData> {
    let mut data: RecentData = serde_json::from_slice(bytes)
        .map_err(|_| failure("malformed JDBC recents; original file retained"))?;
    if data.version != 1 || data.families.len() > 4096 {
        return Err(failure("unsupported JDBC recent version or family count"));
    }
    for values in data.families.values_mut() {
        *values = normalize(std::mem::take(values));
    }
    Ok(data)
}
#[derive(Debug, Clone)]
pub struct UrlRecentRepository {
    root: PathBuf,
    file: JsonFile,
}
impl UrlRecentRepository {
    pub fn new(root: PathBuf) -> Self {
        Self {
            file: JsonFile::new(&root, "jdbc-url-recents-v1.json"),
            root,
        }
    }
    fn read(&self) -> Result<(RecentData, Option<DiskIdentity>)> {
        match self.file.read(2 * 1024 * 1024, false)? {
            None => Ok((RecentData::default(), None)),
            Some((bytes, identity)) => Ok((decode(&bytes)?, Some(identity))),
        }
    }
    fn publish(&self, data: &RecentData, expected: Option<&DiskIdentity>) -> Result<()> {
        if data.families.len() > 4096 {
            return Err(failure("JDBC recent family limit exceeded"));
        }
        let bytes =
            serde_json::to_vec_pretty(data).map_err(|_| failure("invalid JDBC recent data"))?;
        if bytes.len() > 2 * 1024 * 1024 {
            return Err(failure("JDBC recents exceed 2 MiB"));
        }
        self.file.write(bytes, expected)
    }
    pub fn load(&self, family: &str) -> Result<Vec<String>> {
        Ok(self.read()?.0.families.remove(family).unwrap_or_default())
    }
    pub fn remember(&self, family: &str, raw_url: &str) -> Result<bool> {
        let url = raw_url.trim();
        if url.is_empty() || url.len() > 64 * 1024 || security::contains_secret(url) {
            return Ok(false);
        }
        let _lock = ConfigLock::acquire(&self.root)?;
        let (mut data, identity) = self.read()?;
        let old = data.families.entry(family.into()).or_default();
        old.retain(|old| old != url);
        old.insert(0, url.into());
        old.truncate(12);
        self.publish(&data, identity.as_ref())?;
        Ok(true)
    }
    pub fn seed(&self, family: &str, urls: &[String]) -> Result<()> {
        let _lock = ConfigLock::acquire(&self.root)?;
        let (mut data, identity) = self.read()?;
        let values = data.families.entry(family.into()).or_default();
        for url in urls.iter().rev().map(|url| url.trim()).filter(|url| {
            !url.is_empty() && url.len() <= 64 * 1024 && !security::contains_secret(url)
        }) {
            values.retain(|old| old != url);
            values.insert(0, url.into());
            values.truncate(12);
        }
        self.publish(&data, identity.as_ref())
    }
    pub fn import_legacy(&self, legacy_root: &Path) -> Result<bool> {
        if self.root == legacy_root
            || self
                .root
                .canonicalize()
                .ok()
                .zip(legacy_root.canonicalize().ok())
                .is_some_and(|(a, b)| a == b)
        {
            return Err(failure(
                "Rust recents must be separate from legacy configuration",
            ));
        }
        let _lock = ConfigLock::acquire(&self.root)?;
        if self.read()?.1.is_some() {
            return Ok(false);
        }
        let Some((bytes, _)) =
            JsonFile::new(legacy_root, "jdbc-url-recents.json").read(2 * 1024 * 1024, false)?
        else {
            return Ok(false);
        };
        self.publish(&decode(&bytes)?, None)?;
        Ok(true)
    }
}
#[derive(Debug, Clone)]
pub struct UrlSuggestions {
    pub recents: Vec<String>,
    pub standard: Vec<std::sync::Arc<UrlTemplate>>,
}
pub struct UrlSuggestionService {
    pub catalog: UrlCatalog,
    pub recents: UrlRecentRepository,
}
impl UrlSuggestionService {
    pub fn seed(&self, definitions: &[crate::model::DataSourceDefinition]) -> Result<()> {
        for definition in definitions {
            self.remember(&definition.driver.id, &definition.jdbc_url)?;
        }
        Ok(())
    }
    pub fn for_driver(&self, id: &str) -> Result<UrlSuggestions> {
        Ok(UrlSuggestions {
            recents: self.recents.load(self.catalog.recent_family(id))?,
            standard: self.catalog.templates(id).to_vec(),
        })
    }
    pub fn remember(&self, id: &str, url: &str) -> Result<bool> {
        self.recents.remember(self.catalog.recent_family(id), url)
    }
}
