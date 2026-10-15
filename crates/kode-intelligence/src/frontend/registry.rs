//! Per-document adapter selection; no shared mutable parser across workers.
use super::{
    c::CSemanticAdapter,
    generic::GenericSemanticAdapter,
    kotlin::KotlinSemanticAdapter,
    legacy::{self, DefinitionRegistry},
};
use crate::model::FileSemanticDelta;
use kode_core::{DomainError, Result, WorkspaceGeneration, document::TextSnapshot};
use kode_platform::jobs::CancellationToken;
use kode_syntax::{
    keywords::KeywordProvider,
    registry::{GrammarRegistry, canonical_language},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterCapability {
    Kotlin,
    C,
    Generic,
    Heuristic,
}
pub struct ExtractionReport {
    pub delta: FileSemanticDelta,
    pub capability: AdapterCapability,
    pub notes: Vec<String>,
}
enum Adapter {
    Kotlin(KotlinSemanticAdapter),
    C(CSemanticAdapter),
    Generic(GenericSemanticAdapter),
    Heuristic,
}
pub struct SemanticAdapterSession {
    language: String,
    adapter: Adapter,
    initialization_note: Option<String>,
}
impl SemanticAdapterSession {
    pub fn new(registry: &GrammarRegistry, language: &str) -> Self {
        let language = canonical_language(language);
        let result = match language.as_str() {
            "kotlin" => KotlinSemanticAdapter::new(registry).map(Adapter::Kotlin),
            "c" => CSemanticAdapter::new(registry).map(Adapter::C),
            _ if registry.get(&language).is_some() => {
                GenericSemanticAdapter::new(registry, &language).map(Adapter::Generic)
            }
            _ => Err(DomainError::Unsupported(format!(
                "no native {language} grammar; using heuristic definitions"
            ))),
        };
        let (adapter, initialization_note) = match result {
            Ok(adapter) => (adapter, None),
            Err(error) => (Adapter::Heuristic, Some(error.to_string())),
        };
        Self {
            language,
            adapter,
            initialization_note,
        }
    }
    pub fn language(&self) -> &str {
        &self.language
    }
    pub fn capability(&self) -> AdapterCapability {
        match self.adapter {
            Adapter::Kotlin(_) => AdapterCapability::Kotlin,
            Adapter::C(_) => AdapterCapability::C,
            Adapter::Generic(_) => AdapterCapability::Generic,
            Adapter::Heuristic => AdapterCapability::Heuristic,
        }
    }
    pub fn extract(
        &mut self,
        definitions: &DefinitionRegistry,
        keywords: &KeywordProvider,
        path: &str,
        snapshot: &TextSnapshot,
        generation: WorkspaceGeneration,
        cancel: &CancellationToken,
    ) -> Result<ExtractionReport> {
        let mut notes: Vec<_> = self.initialization_note.iter().cloned().collect();
        let result = match &mut self.adapter {
            Adapter::Kotlin(adapter) => adapter.extract(path, snapshot, generation, cancel),
            Adapter::C(adapter) => adapter.extract(path, snapshot, generation, cancel),
            Adapter::Generic(adapter) => adapter.extract(path, snapshot, generation, cancel),
            Adapter::Heuristic => legacy::extract(
                definitions,
                keywords,
                path,
                &self.language,
                snapshot,
                cancel,
            ),
        };
        let (mut delta, capability) = match result {
            Ok(delta) => (delta, self.capability()),
            Err(DomainError::Cancelled) => return Err(DomainError::Cancelled),
            Err(error) => {
                notes.push(format!("{error}; using heuristic definitions"));
                (
                    legacy::extract(
                        definitions,
                        keywords,
                        path,
                        &self.language,
                        snapshot,
                        cancel,
                    )?,
                    AdapterCapability::Heuristic,
                )
            }
        };
        // The source's secondary parser registrations also run configurable
        // regex extraction (SQL declarations, Rust-like DSLs, etc.). Preserve
        // those additional definitions without replacing an AST declaration.
        if capability == AdapterCapability::Generic && !primary_adapter(&self.language) {
            let fallback = legacy::extract(
                definitions,
                keywords,
                path,
                &self.language,
                snapshot,
                cancel,
            )?;
            merge_fallback(&mut delta, fallback);
        }
        let parsed = match &self.adapter {
            Adapter::Kotlin(adapter) => adapter.last_document(),
            Adapter::C(adapter) => adapter.last_document(),
            Adapter::Generic(adapter) => adapter.last_document(),
            Adapter::Heuristic => None,
        };
        if let Some(parsed) = parsed
            && parsed.has_errors()
            && parsed.snapshot().id == snapshot.id
            && parsed.snapshot().revision == snapshot.revision
            && parsed.source() == snapshot.text().as_ref()
        {
            notes.push(format!(
                "{}: partial syntax ({} recovery diagnostics)",
                self.language,
                parsed.diagnostics().len()
            ));
        }
        Ok(ExtractionReport {
            delta,
            capability,
            notes,
        })
    }
}

fn primary_adapter(language: &str) -> bool {
    matches!(
        language,
        "cpp"
            | "go"
            | "javascript"
            | "typescript"
            | "tsx"
            | "html"
            | "css"
            | "json"
            | "python"
            | "php"
            | "ruby"
            | "bash"
            | "swift"
    )
}
fn merge_fallback(delta: &mut FileSemanticDelta, fallback: FileSemanticDelta) {
    use crate::{ids::public_surface_hash, model::*};
    use std::collections::{BTreeMap, HashSet};
    let Some(file_scope) = delta
        .scopes
        .iter()
        .find(|s| s.kind == ScopeKind::File)
        .map(|s| s.id)
    else {
        return;
    };
    let mut names: HashSet<_> = delta.symbols.iter().map(|s| s.name_range).collect();
    let mut added = BTreeMap::new();
    for mut symbol in fallback.symbols {
        if names.insert(symbol.name_range) {
            symbol.scope_id = file_scope;
            added.insert(symbol.id, symbol.name_range);
            delta.symbols.push(symbol);
        }
    }
    let mut occurrences: BTreeMap<_, _> = delta.occurrences.drain(..).map(|o| (o.id, o)).collect();
    for mut occurrence in fallback.occurrences {
        occurrence.scope_id = file_scope;
        if occurrence.kind == OccurrenceKind::Declaration
            && !occurrence
                .resolved_symbol_id
                .is_some_and(|id| added.contains_key(&id))
        {
            continue;
        }
        if occurrence.kind == OccurrenceKind::Declaration {
            occurrences.insert(occurrence.id, occurrence);
        } else {
            occurrences.entry(occurrence.id).or_insert(occurrence);
        }
    }
    delta.occurrences = occurrences.into_values().collect();
    delta
        .occurrences
        .sort_by_key(|o| (o.range.start, o.range.end));
    delta.symbols.sort_by_key(|s| (s.name_range.start, s.id));
    delta.exported_surface_hash = public_surface_hash(delta);
}
