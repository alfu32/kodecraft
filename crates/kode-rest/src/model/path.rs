use kode_core::{DomainError, Result};
use serde::{Deserialize, Serialize};
#[derive(Debug, Default, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodePath(pub Vec<usize>);
impl NodePath {
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }
    pub fn parent(&self) -> Self {
        Self(self.0[..self.0.len().saturating_sub(1)].to_vec())
    }
    pub fn child(&self, index: usize) -> Self {
        let mut path = self.0.clone();
        path.push(index);
        Self(path)
    }
    pub fn encode(&self) -> String {
        self.0
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join("/")
    }
    /// Malformed input never silently selects a different node.
    pub fn decode(text: Option<&str>) -> Result<Self> {
        let Some(text) = text.filter(|text| !text.trim().is_empty()) else {
            return Ok(Self::default());
        };
        let path = text
            .split('/')
            .map(|value| {
                value.parse::<usize>().map_err(|_| DomainError::Parse {
                    format: "REST node path",
                    message: "Expected nonnegative item indices separated by /".into(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        if path.len() > 64 {
            return Err(DomainError::Unsupported(
                "REST tree nesting exceeds 64 levels".into(),
            ));
        }
        Ok(Self(path))
    }
    pub fn starts_with(&self, ancestor: &Self) -> bool {
        self.0.starts_with(&ancestor.0)
    }
    /// Rebase a view path after removing an item; descendants of it disappear.
    pub fn after_remove(&self, removed: &Self) -> Option<Self> {
        if self.starts_with(removed) {
            return None;
        }
        let mut path = self.clone();
        let parent = removed.parent();
        let depth = parent.0.len();
        if path.0.len() > depth && path.starts_with(&parent) && path.0[depth] > removed.0[depth] {
            path.0[depth] -= 1;
        }
        Some(path)
    }
    /// Rebase a view path after inserting an item. Root replacement is not an
    /// array insertion; callers reset their view state separately in that case.
    pub fn after_insert(&self, inserted: &Self) -> Self {
        if inserted.is_root() {
            return self.clone();
        }
        let mut path = self.clone();
        let parent = inserted.parent();
        let depth = parent.0.len();
        if path.0.len() > depth && path.starts_with(&parent) && path.0[depth] >= inserted.0[depth] {
            path.0[depth] = path.0[depth].saturating_add(1);
        }
        path
    }
}
