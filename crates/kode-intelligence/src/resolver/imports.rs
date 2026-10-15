use super::*;
use std::path::{Component, Path, PathBuf};
impl State<'_> {
    pub fn import_candidates(&self, name: &str, file: FileId, wildcard: bool) -> Vec<SymbolId> {
        let Some(delta) = self.project.deltas.get(&file) else {
            return vec![];
        };
        let mut result = BTreeSet::new();
        for import in &delta.imports {
            if self.language(file) == "c" {
                continue;
            }
            if wildcard && import.wildcard {
                if let Some(ids) = self.qualified.get(&format!("{}.{name}", import.path)) {
                    result.extend(ids);
                }
            } else if !wildcard
                && !import.wildcard
                && import
                    .alias
                    .as_deref()
                    .unwrap_or_else(|| import.path.rsplit('.').next().unwrap_or_default())
                    == name
                && let Some(ids) = self.qualified.get(&import.path)
            {
                result.extend(ids);
            }
        }
        result.into_iter().collect()
    }
    pub fn import_occurrence(&self, occurrence: &OccurrenceRecord) -> Option<SymbolId> {
        let delta = self.project.deltas.get(&occurrence.file_id)?;
        let import = delta.imports.iter().find(|i| {
            i.range.start <= occurrence.range.start && occurrence.range.end <= i.range.end
        })?;
        if import.path.rsplit('.').next() == Some(occurrence.text.as_str())
            || import.alias.as_deref() == Some(occurrence.text.as_str())
        {
            return self.unique(
                self.qualified
                    .get(&import.path)
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|id| {
                        self.symbol(*id).is_some_and(|s| {
                            self.accessible(s, occurrence.file_id, occurrence.scope_id)
                        })
                    }),
            );
        }
        // Intermediate qualifiers are package references, never arbitrary local
        // declarations that happen to share a component's spelling.
        let parts: Vec<_> = import.path.split('.').collect();
        for end in 1..parts.len() {
            if parts[end - 1] != occurrence.text {
                continue;
            }
            if let Some(ids) = self.qualified.get(&parts[..end].join(".")) {
                return self.unique(ids.iter().copied().filter(|id| {
                    self.symbol(*id)
                        .is_some_and(|s| s.kind == SymbolKind::Package)
                }));
            }
        }
        None
    }
    pub fn included_files(&self, file: FileId) -> BTreeSet<FileId> {
        let Some(delta) = self.project.deltas.get(&file) else {
            return BTreeSet::new();
        };
        let directory = Path::new(&delta.file.path)
            .parent()
            .unwrap_or_else(|| Path::new(""));
        let mut result = BTreeSet::new();
        for import in &delta.imports {
            let relative = normalize(directory.join(&import.path));
            for path in [&relative, &import.path] {
                let id = SemanticIds::file(path);
                if self.project.deltas.contains_key(&id) {
                    result.insert(id);
                }
            }
        }
        result
    }
}
fn normalize(path: PathBuf) -> String {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    result.push("..");
                }
            }
            other => result.push(other.as_os_str()),
        }
    }
    result.to_string_lossy().replace('\\', "/")
}
