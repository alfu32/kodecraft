use super::project::{Project, lexical_path};
use ignore::{
    Match,
    gitignore::{Gitignore, GitignoreBuilder},
};
use kode_core::{DomainError, Result};
use std::{
    collections::HashMap,
    fs,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IgnoreDecision {
    Unspecified,
    Ignore,
    Include,
}
struct Layer {
    parent: Option<Arc<Layer>>,
    matcher: Gitignore,
}
impl Layer {
    fn decision(&self, path: &Path, is_dir: bool) -> IgnoreDecision {
        match self.matcher.matched(path, is_dir) {
            Match::Ignore(_) => IgnoreDecision::Ignore,
            Match::Whitelist(_) => IgnoreDecision::Include,
            Match::None => self
                .parent
                .as_ref()
                .map_or(IgnoreDecision::Unspecified, |parent| {
                    parent.decision(path, is_dir)
                }),
        }
    }
}
/// One scan/classification snapshot: compile each directory's ignore file once.
/// The `ignore` crate parses Git syntax; only project-local rules are loaded.
pub struct IgnoreRules {
    root: PathBuf,
    extra: Vec<String>,
    layers: Mutex<HashMap<PathBuf, Arc<Layer>>>,
    warnings: Mutex<Vec<String>>,
}
impl IgnoreRules {
    pub fn new(project: &Project) -> Self {
        Self {
            root: project.root.clone(),
            extra: project.config.ignore_patterns.clone(),
            layers: Mutex::default(),
            warnings: Mutex::default(),
        }
    }
    fn layer(&self, directory: &Path) -> Result<Arc<Layer>> {
        if !directory.starts_with(&self.root) {
            return Err(DomainError::Unsupported(
                "ignore lookup outside project".into(),
            ));
        }
        if let Some(layer) = self
            .layers
            .lock()
            .expect("ignore cache")
            .get(directory)
            .cloned()
        {
            return Ok(layer);
        }
        let parent = if directory == self.root {
            None
        } else {
            Some(self.layer(directory.parent().expect("inside root"))?)
        };
        let mut builder = GitignoreBuilder::new(directory);
        let file = directory.join(".gitignore");
        match fs::symlink_metadata(&file) {
            Ok(metadata) if metadata.is_file() => {
                if let Some(error) = builder.add(&file) {
                    self.warnings
                        .lock()
                        .expect("ignore warnings")
                        .push(format!("{}: {error}", file.display()));
                }
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => self
                .warnings
                .lock()
                .expect("ignore warnings")
                .push(format!("{}: {error}", file.display())),
            _ => {}
        }
        if directory == self.root {
            for pattern in &self.extra {
                builder
                    .add_line(None, pattern)
                    .map_err(|error| DomainError::Parse {
                        format: "gitignore",
                        message: error.to_string(),
                    })?;
            }
        }
        let matcher = builder.build().map_err(|error| DomainError::Parse {
            format: "gitignore",
            message: error.to_string(),
        })?;
        let layer = Arc::new(Layer { parent, matcher });
        self.layers
            .lock()
            .expect("ignore cache")
            .insert(directory.into(), layer.clone());
        Ok(layer)
    }
    /// Traversal already prunes ignored ancestors. Local rules apply to children,
    /// preserving nested anchoring and nearest-directory precedence.
    pub fn entry(&self, path: &Path, is_dir: bool) -> Result<IgnoreDecision> {
        if path == self.root {
            return Ok(IgnoreDecision::Unspecified);
        }
        if !path.starts_with(&self.root) {
            return Err(DomainError::Unsupported(
                "ignore lookup outside project".into(),
            ));
        }
        Ok(self
            .layer(path.parent().expect("inside root"))?
            .decision(path, is_dir))
    }
    /// Bounded folder/tree query, consistent with traversal's ancestor pruning.
    pub fn is_ignored(&self, path: &Path, is_dir: bool) -> Result<bool> {
        let path = lexical_path(&self.root, path)?;
        let relative = path.strip_prefix(&self.root).expect("inside root");
        let mut current = self.root.clone();
        let count = relative.components().count();
        for (index, component) in relative.components().enumerate() {
            current.push(component.as_os_str());
            if self.entry(&current, index + 1 < count || is_dir)? == IgnoreDecision::Ignore {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub fn take_warnings(&self) -> Vec<String> {
        std::mem::take(&mut *self.warnings.lock().expect("ignore warnings"))
    }
    pub fn validate(&self) -> Result<()> {
        self.layer(&self.root).map(|_| ())
    }
}

#[derive(Debug, Clone)]
pub struct Exclusions {
    paths: Vec<PathBuf>,
}
impl Exclusions {
    pub fn new(project: &Project) -> Result<Self> {
        let mut paths = vec![];
        for path in &project.config.exclusions {
            if path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
            {
                return Err(DomainError::Unsupported(
                    "scan exclusions cannot contain parent components".into(),
                ));
            }
            let path = lexical_path(&project.root, path)?;
            if path == project.root {
                return Err(DomainError::Unsupported(
                    "the project root cannot be excluded".into(),
                ));
            }
            let path = fs::canonicalize(&path).unwrap_or(path);
            if !path.starts_with(&project.root) {
                return Err(DomainError::Unsupported(
                    "exclusion resolves outside project".into(),
                ));
            }
            paths.push(path);
        }
        Ok(Self { paths })
    }
    pub fn contains(&self, path: &Path) -> bool {
        self.paths.iter().any(|excluded| path.starts_with(excluded))
    }
}
pub fn index_metadata(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return true;
    };
    for (index, component) in relative.components().enumerate() {
        let name = component.as_os_str().to_string_lossy();
        if matches!(
            name.as_ref(),
            ".git" | ".kode" | ".kode.json" | ".kode-rs" | ".kode-rs.json"
        ) || name.starts_with(".kode-rs.json.")
            || name.starts_with(".kode.json.")
            || index == 0 && matches!(name.as_ref(), "build" | "target")
        {
            return true;
        }
    }
    false
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderStatus {
    GitIgnored,
    InSource,
    OutsideSource,
    ExcludedSource,
}
pub fn folder_status(
    project: &Project,
    path: &Path,
    rules: &IgnoreRules,
) -> Result<Option<FolderStatus>> {
    let lexical = match lexical_path(&project.root, path) {
        Ok(path) => path,
        Err(_) => return Ok(None),
    };
    if !lexical.is_dir() {
        return Ok(None);
    }
    let target = match project.resolve_existing(&lexical) {
        Ok(path) => path,
        Err(_) => return Ok(None),
    };
    let (roots, _) = project.resolved_roots();
    let in_source = roots.iter().any(|root| target.starts_with(root));
    let excluded =
        Exclusions::new(project)?.contains(&target) || index_metadata(&project.root, &target);
    Ok(Some(if in_source && excluded {
        FolderStatus::ExcludedSource
    } else if rules.is_ignored(&lexical, true)? {
        FolderStatus::GitIgnored
    } else if in_source {
        FolderStatus::InSource
    } else {
        FolderStatus::OutsideSource
    }))
}
