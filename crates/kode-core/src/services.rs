use crate::intelligence::DocumentSnapshot;
use crate::{DocumentOrigin, JobTag, Result};
use std::path::Path;

pub trait Clipboard: Send + Sync {
    fn read(&self) -> Result<String>;
    fn write(&self, text: &str) -> Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskIdentity {
    pub canonical_path: std::path::PathBuf,
    pub length: u64,
    pub modified_nanos: Option<u128>,
    pub content_hash: String,
}

#[derive(Debug, Clone)]
pub struct LoadedDocument {
    pub tag: JobTag,
    pub origin: DocumentOrigin,
    pub bytes: Vec<u8>,
    pub disk_identity: Option<DiskIdentity>,
}

pub trait DocumentIo: Send + Sync {
    fn load(&self, origin: &DocumentOrigin, tag: JobTag) -> Result<LoadedDocument>;
    fn save(
        &self,
        path: &Path,
        snapshot: &DocumentSnapshot,
        expected: Option<&DiskIdentity>,
    ) -> Result<DiskIdentity>;
}
