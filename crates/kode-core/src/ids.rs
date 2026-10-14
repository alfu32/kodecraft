use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

macro_rules! identifier {
    ($name:ident) => {
        #[derive(
            Debug,
            Default,
            Clone,
            Copy,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Serialize,
            Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub u64);
    };
}
identifier!(WorkspaceGeneration);
identifier!(DocumentId);
identifier!(Revision);
identifier!(RequestId);

static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static DOCUMENT_SEQUENCE: AtomicU64 = AtomicU64::new(0);
impl RequestId {
    pub fn fresh() -> crate::Result<Self> {
        REQUEST_SEQUENCE
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map(|previous| Self(previous + 1))
            .map_err(|_| crate::DomainError::Unsupported("request identity exhausted".into()))
    }
}
impl DocumentId {
    pub fn fresh() -> crate::Result<Self> {
        DOCUMENT_SEQUENCE
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map(|previous| Self(previous + 1))
            .map_err(|_| crate::DomainError::Unsupported("document identity exhausted".into()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DocumentOrigin {
    File(PathBuf),
    Virtual {
        owner: String,
        key: String,
        label: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct JobTag {
    pub workspace: WorkspaceGeneration,
    pub request: RequestId,
    pub document: Option<(DocumentId, Revision)>,
}

impl JobTag {
    /// A result is applicable only to the exact owner/request/document snapshot.
    pub fn is_current(self, current: Self) -> bool {
        self == current
    }
}
