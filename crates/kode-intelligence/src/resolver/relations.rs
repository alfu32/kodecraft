use super::*;
impl State<'_> {
    pub fn derive_dependencies(&mut self) -> Result<()> {
        let files: Vec<_> = self.project.deltas.keys().copied().collect();
        for file in files {
            let generation = self
                .project
                .resolution_generations
                .get(&file)
                .copied()
                .unwrap_or(0)
                .max(1);
            self.project.resolution_generations.insert(file, generation);
        }
        let mut dependencies = BTreeMap::new();
        let generations = self.project.resolution_generations.clone();
        let mut add = |from: FileId, to: FileId, kind| {
            if from != to {
                dependencies.insert(
                    (from, to, kind),
                    FileDependencyRecord {
                        from_file_id: from,
                        to_file_id: to,
                        kind,
                        generation: generations[&from],
                    },
                );
            }
        };
        for i in 0..self.project.relations.len() {
            self.check()?;
            let relation = &self.project.relations[i];
            let (Some(from), Some(to)) = (self.symbol(relation.from), self.symbol(relation.to))
            else {
                continue;
            };
            let kind = match relation.kind {
                RelationKind::Extends | RelationKind::Implements => FileDependencyKind::Inheritance,
                RelationKind::Calls => FileDependencyKind::Call,
                RelationKind::Imports => FileDependencyKind::Import,
                RelationKind::TypeOf
                | RelationKind::Returns
                | RelationKind::ParameterType
                | RelationKind::AliasOf => FileDependencyKind::TypeReference,
                _ => FileDependencyKind::Reference,
            };
            add(from.file_id, to.file_id, kind);
        }
        for i in 0..self.project.occurrences.len() {
            self.check()?;
            let occurrence = &self.project.occurrences[i];
            let Some(target) = occurrence.resolved_symbol_id.and_then(|id| self.symbol(id)) else {
                continue;
            };
            let kind = match occurrence.kind {
                OccurrenceKind::Call => FileDependencyKind::Call,
                OccurrenceKind::TypeReference => FileDependencyKind::TypeReference,
                OccurrenceKind::MemberReference => FileDependencyKind::MemberReference,
                OccurrenceKind::Import => FileDependencyKind::Import,
                _ => FileDependencyKind::Reference,
            };
            add(occurrence.file_id, target.file_id, kind);
        }
        let imports: Vec<_> = self
            .project
            .deltas
            .values()
            .flat_map(|delta| {
                delta
                    .imports
                    .iter()
                    .map(|import| (delta.file.id, import.clone()))
            })
            .collect();
        for (file, import) in imports {
            self.check()?;
            if self.language(file) == "c" {
                for target in self.included_files(file) {
                    add(file, target, FileDependencyKind::Import);
                }
            } else if !import.wildcard {
                for target in self
                    .qualified
                    .get(&import.path)
                    .into_iter()
                    .flatten()
                    .filter_map(|id| self.symbol(*id))
                {
                    add(file, target.file_id, FileDependencyKind::Import);
                }
            } else {
                let prefix = format!("{}.", import.path);
                for (_, targets) in self
                    .qualified
                    .range(prefix.clone()..)
                    .take_while(|(name, _)| name.starts_with(&prefix))
                {
                    for target in targets.iter().filter_map(|id| self.symbol(*id)) {
                        add(file, target.file_id, FileDependencyKind::Import);
                    }
                }
            }
        }
        self.project.dependencies = dependencies.into_values().collect();
        self.check()
    }
}
