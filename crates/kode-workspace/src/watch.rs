//! Constant-memory watcher invalidation and five-second fallback polling.
use kode_core::DomainError;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

pub struct TreeWatcher {
    _watcher: Option<RecommendedWatcher>,
    dirty: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    source_revision: Arc<AtomicU64>,
}
impl TreeWatcher {
    /// Worker-owned setup/drop. Native failures still permit timer refreshes.
    pub fn new(root: &Path) -> Self {
        let dirty = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let callback_dirty = dirty.clone();
        let callback_error = error.clone();
        let source_revision = Arc::new(AtomicU64::new(0));
        let callback_revision = source_revision.clone();
        let source_root = root.to_path_buf();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            match event {
                Ok(event) if event.kind.is_access() => return,
                Ok(event) => {
                    if event.paths.is_empty()
                        || event
                            .paths
                            .iter()
                            .any(|path| !crate::exclusions::index_metadata(&source_root, path))
                    {
                        callback_revision.fetch_add(1, Ordering::AcqRel);
                    }
                }
                Err(failure) => {
                    *callback_error.lock().expect("watch error") = Some(failure.to_string());
                }
            }
            callback_dirty.store(true, Ordering::Release);
        })
        .and_then(|mut watcher| {
            watcher.watch(root, RecursiveMode::Recursive)?;
            Ok(watcher)
        });
        let watcher = match watcher {
            Ok(watcher) => Some(watcher),
            Err(failure) => {
                *error.lock().expect("watch error") = Some(failure.to_string());
                None
            }
        };
        Self {
            _watcher: watcher,
            dirty,
            error,
            source_revision,
        }
    }
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::AcqRel)
    }
    /// Non-consuming source invalidation excludes writes to owned index metadata.
    pub fn source_revision(&self) -> u64 {
        self.source_revision.load(Ordering::Acquire)
    }
    pub fn take_error(&self) -> Option<DomainError> {
        self.error.lock().expect("watch error").take().map(|error| {
            DomainError::Unsupported(format!(
                "file watcher: {error}; timer refresh remains active"
            ))
        })
    }
}
#[derive(Debug, Default)]
pub struct RefreshTimer {
    dirty_since: Option<u64>,
    last_refresh: u64,
}
impl RefreshTimer {
    pub const COALESCE_MS: u64 = 150;
    pub const FALLBACK_MS: u64 = 5_000;
    pub fn changed(&mut self, now: u64) {
        self.dirty_since.get_or_insert(now);
    }
    pub fn due(&mut self, now: u64, busy: bool) -> bool {
        if busy {
            return false;
        }
        if self
            .dirty_since
            .is_some_and(|since| now.saturating_sub(since) >= Self::COALESCE_MS)
            || now.saturating_sub(self.last_refresh) >= Self::FALLBACK_MS
        {
            self.dirty_since = None;
            self.last_refresh = now;
            true
        } else {
            false
        }
    }
    pub fn refreshed(&mut self, now: u64) {
        self.last_refresh = now;
        self.dirty_since = None;
    }
}
