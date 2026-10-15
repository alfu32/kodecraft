use std::path::PathBuf;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryLocation {
    pub git_dir: PathBuf,
    pub work_tree: Option<PathBuf>,
    pub bare: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    Unborn {
        reference: String,
        name: String,
    },
    Branch {
        reference: String,
        name: String,
        commit: String,
    },
    Detached {
        commit: String,
    },
}
impl Head {
    pub fn label(&self) -> &str {
        match self {
            Self::Unborn { reference, .. } | Self::Branch { reference, .. } => reference,
            Self::Detached { commit } => commit,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StatusLayer {
    Index,
    Worktree,
    Conflict,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    TypeChanged,
    Untracked,
    Conflicted,
    Unreadable,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: PathBuf,
    pub original_path: Option<PathBuf>,
    pub layer: StatusLayer,
    pub kind: ChangeKind,
    /// Source-compatible one-column codes; conflicts use their porcelain pair.
    pub code: String,
}
impl StatusEntry {
    pub fn staged(&self) -> bool {
        self.layer == StatusLayer::Index
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub reference: String,
    pub commit: Option<String>,
    pub current: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    pub object: String,
    pub commit: Option<String>,
    pub annotated: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub author: String,
    pub timestamp: i64,
    pub offset_minutes: i32,
    pub message: String,
    pub parents: Vec<String>,
    pub tag: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing<T> {
    pub entries: Vec<T>,
    pub truncated: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    Missing,
    Text,
    Binary,
    TooLarge,
    Submodule,
    Symlink,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Content {
    pub kind: ContentKind,
    pub bytes: Vec<u8>,
    pub length: u64,
    pub object: Option<String>,
    pub mode: u32,
}
impl Content {
    pub fn missing() -> Self {
        Self {
            kind: ContentKind::Missing,
            bytes: vec![],
            length: 0,
            object: None,
            mode: 0,
        }
    }
    pub fn text(&self) -> Option<String> {
        matches!(self.kind, ContentKind::Text | ContentKind::Symlink)
            .then(|| {
                kode_core::document::encoding::decode(&self.bytes)
                    .ok()
                    .map(|text| text.text)
            })
            .flatten()
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffContents {
    pub path: PathBuf,
    pub staged: bool,
    pub revision: Option<String>,
    pub old: Content,
    pub new: Content,
}
#[derive(Debug, Clone)]
pub struct ReadLimits {
    pub blob_bytes: usize,
    pub entries: usize,
    pub commits: usize,
}
impl Default for ReadLimits {
    fn default() -> Self {
        Self {
            blob_bytes: 8 * 1024 * 1024,
            entries: 100_000,
            commits: 5_000,
        }
    }
}
impl ReadLimits {
    pub(crate) fn bounded(&self) -> Self {
        Self {
            blob_bytes: self.blob_bytes.min(32 * 1024 * 1024),
            entries: self.entries.min(1_000_000),
            commits: self.commits.min(50_000),
        }
    }
}
#[derive(Debug, Clone)]
pub struct RepositorySnapshot {
    pub location: RepositoryLocation,
    pub head: Option<Head>,
    pub status: Listing<StatusEntry>,
    pub branches: Listing<Branch>,
    pub commits: Listing<Commit>,
    pub tags: Listing<Tag>,
    pub errors: Vec<String>,
}
