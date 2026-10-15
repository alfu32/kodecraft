use kode_core::{DomainError, Result, WorkspaceGeneration};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    pub source_roots: Vec<PathBuf>,
    pub exclusions: Vec<PathBuf>,
    /// Additional project-local Gitignore syntax; nested rules may override it.
    pub ignore_patterns: Vec<String>,
}
#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
    pub generation: WorkspaceGeneration,
    pub config: ProjectConfig,
}
impl Project {
    pub fn new(root: &Path, generation: WorkspaceGeneration) -> Result<Self> {
        let root = project_root(root)?;
        let source_roots = ["src", "lib", "include"]
            .into_iter()
            .filter(|name| root.join(name).is_dir())
            .map(PathBuf::from)
            .collect();
        Ok(Self {
            root,
            generation,
            config: ProjectConfig {
                source_roots,
                ..Default::default()
            },
        })
    }
    /// A saved empty root list is an explicit opt-out, not a request for defaults.
    pub fn configured(
        root: &Path,
        generation: WorkspaceGeneration,
        config: ProjectConfig,
    ) -> Result<Self> {
        Ok(Self {
            root: project_root(root)?,
            generation,
            config,
        })
    }
    pub fn add_source_root(&mut self, path: &Path) -> Result<bool> {
        let path = self.resolve_existing(path)?;
        if !path.is_dir() {
            return Err(DomainError::Unsupported(
                "source root must be a directory".into(),
            ));
        }
        let relative = path.strip_prefix(&self.root).expect("validated path");
        let relative = if relative.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            relative.to_path_buf()
        };
        if self
            .config
            .source_roots
            .iter()
            .any(|root| self.resolve_existing(root).ok().as_ref() == Some(&path))
        {
            return Ok(false);
        }
        self.config.source_roots.push(relative);
        Ok(true)
    }
    pub fn remove_source_root(&mut self, path: &Path) -> Result<bool> {
        let absolute = lexical_path(&self.root, path)?;
        let before = self.config.source_roots.len();
        self.config
            .source_roots
            .retain(|root| lexical_path(&self.root, root).ok().as_ref() != Some(&absolute));
        Ok(before != self.config.source_roots.len())
    }
    pub fn exclude(&mut self, path: &Path) -> Result<bool> {
        let path = self.resolve_existing(path)?;
        if path == self.root {
            return Err(DomainError::Unsupported(
                "the project root cannot be excluded".into(),
            ));
        }
        let relative = path
            .strip_prefix(&self.root)
            .expect("validated path")
            .to_path_buf();
        if self.config.exclusions.contains(&relative) {
            return Ok(false);
        }
        self.config.exclusions.push(relative);
        Ok(true)
    }
    pub fn include(&mut self, path: &Path) -> Result<bool> {
        let absolute = lexical_path(&self.root, path)?;
        let before = self.config.exclusions.len();
        self.config
            .exclusions
            .retain(|path| lexical_path(&self.root, path).ok().as_ref() != Some(&absolute));
        Ok(before != self.config.exclusions.len())
    }
    pub fn resolve_existing(&self, path: &Path) -> Result<PathBuf> {
        let requested = lexical_path(&self.root, path)?;
        let canonical = fs::canonicalize(&requested).map_err(|source| DomainError::Io {
            operation: "resolve project path",
            path: requested,
            source,
        })?;
        if !canonical.starts_with(&self.root) {
            return Err(DomainError::Unsupported(
                "path resolves outside the project root".into(),
            ));
        }
        Ok(canonical)
    }
    /// Successful internal filesystem moves preserve configured relative roots
    /// and exclusions. Deleted roots remain visible for recovery and feedback.
    pub fn reconcile_move(&mut self, from: &Path, to: &Path) {
        for paths in [&mut self.config.source_roots, &mut self.config.exclusions] {
            for path in paths {
                if let Ok(absolute) = lexical_path(&self.root, path)
                    && let Some(moved) = crate::tree::moved_path(&absolute, from, to)
                    && let Ok(relative) = moved.strip_prefix(&self.root)
                {
                    *path = relative.to_path_buf();
                }
            }
        }
    }
    /// Deduplicated effective roots, with overlapping children covered once.
    pub fn resolved_roots(&self) -> (Vec<PathBuf>, Vec<String>) {
        let mut roots = HashSet::new();
        let mut warnings = vec![];
        for path in &self.config.source_roots {
            match self.resolve_existing(path) {
                Ok(root) if root.is_dir() => {
                    roots.insert(root);
                }
                Ok(_) => warnings.push(format!(
                    "source root is not a directory: {}",
                    path.display()
                )),
                Err(error) => warnings.push(error.to_string()),
            }
        }
        let mut roots: Vec<_> = roots.into_iter().collect();
        roots.sort_by_key(|root| (root.components().count(), root.clone()));
        let mut effective: Vec<PathBuf> = vec![];
        for root in roots {
            if !effective.iter().any(|parent| root.starts_with(parent)) {
                effective.push(root);
            }
        }
        (effective, warnings)
    }
}
pub fn lexical_path(root: &Path, path: &Path) -> Result<PathBuf> {
    let path = if path.as_os_str().is_empty() {
        root.to_path_buf()
    } else if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            _ => out.push(component.as_os_str()),
        }
    }
    if !out.starts_with(root) {
        return Err(DomainError::Unsupported(
            "path is outside the project root".into(),
        ));
    }
    Ok(out)
}
fn project_root(path: &Path) -> Result<PathBuf> {
    let root = fs::canonicalize(path).map_err(|source| DomainError::Io {
        operation: "open project",
        path: path.into(),
        source,
    })?;
    if !root.is_dir() {
        return Err(DomainError::Unsupported(
            "project root must be a directory".into(),
        ));
    }
    Ok(root)
}
