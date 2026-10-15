use crate::{ids::FileId, model::FileDependencyRecord};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Immutable reverse graph; closure includes its seeds and terminates on cycles.
#[derive(Debug, Default)]
pub struct DependencyGraph {
    outgoing: BTreeMap<FileId, BTreeSet<FileId>>,
    incoming: BTreeMap<FileId, BTreeSet<FileId>>,
}
impl DependencyGraph {
    pub fn new<'a>(dependencies: impl IntoIterator<Item = &'a FileDependencyRecord>) -> Self {
        let mut graph = Self::default();
        for edge in dependencies {
            graph
                .outgoing
                .entry(edge.from_file_id)
                .or_default()
                .insert(edge.to_file_id);
            graph
                .incoming
                .entry(edge.to_file_id)
                .or_default()
                .insert(edge.from_file_id);
        }
        graph
    }
    pub fn dependencies_of(&self, file: FileId) -> impl Iterator<Item = FileId> + '_ {
        self.outgoing.get(&file).into_iter().flatten().copied()
    }
    pub fn direct_dependents_of(&self, file: FileId) -> impl Iterator<Item = FileId> + '_ {
        self.incoming.get(&file).into_iter().flatten().copied()
    }
    pub fn dependent_closure(&self, seeds: impl IntoIterator<Item = FileId>) -> BTreeSet<FileId> {
        let mut visited = BTreeSet::new();
        let mut queue = VecDeque::new();
        for seed in seeds {
            if visited.insert(seed) {
                queue.push_back(seed);
            }
        }
        while let Some(file) = queue.pop_front() {
            for dependent in self.direct_dependents_of(file) {
                if visited.insert(dependent) {
                    queue.push_back(dependent);
                }
            }
        }
        visited
    }
}
