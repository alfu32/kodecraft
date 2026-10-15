//! Native editor queries. Call from a bounded worker, never from terminal drawing.
use crate::{
    completion::{CompletionContext, CompletionEngine},
    frontend::{legacy::DefinitionRegistry, registry::SemanticAdapterSession},
    ids::{FileId, SemanticIds},
    index::IncrementalSemanticIndex,
    model::*,
    snapshot::SemanticSnapshot,
    store::StoreClient,
    tokens::{SemanticToken, SemanticTokenService},
};
use kode_core::{
    DocumentId, DocumentOrigin, DomainError, Result, Revision, WorkspaceGeneration,
    document::{Document, TextSnapshot},
    intelligence::{
        CompletionItem, DocumentSnapshot, HighlightSpan, IntelligenceProvider, IntelligenceRequest,
        NavigationTarget, SymbolKind as EditorKind,
    },
};
use kode_platform::{
    jobs::CancellationToken,
    paths::{ResourcePaths, compiled_resources},
};
use kode_syntax::{
    keywords::KeywordProvider,
    registry::{GrammarRegistry, canonical_language},
};
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

pub struct IntelligenceResources {
    pub grammars: GrammarRegistry,
    pub keywords: Arc<KeywordProvider>,
    pub definitions: Arc<DefinitionRegistry>,
}
impl IntelligenceResources {
    pub fn compiled() -> Result<Self> {
        Ok(Self {
            grammars: GrammarRegistry::compiled(),
            keywords: Arc::new(KeywordProvider::compiled()?),
            definitions: Arc::new(DefinitionRegistry::compiled()?),
        })
    }
    pub fn load(paths: &ResourcePaths, definitions: Option<&Path>) -> Result<Self> {
        let defaults = compiled_resources();
        let patterns = paths.resolve("keyword-patterns.txt", &defaults)?;
        let colors = paths.resolve("token-colors.txt", &defaults)?;
        let utf8 = |bytes: Vec<u8>| {
            String::from_utf8(bytes).map_err(|e| DomainError::Parse {
                format: "syntax resources",
                message: e.to_string(),
            })
        };
        Ok(Self {
            grammars: GrammarRegistry::compiled(),
            keywords: Arc::new(KeywordProvider::from_resources(
                &utf8(patterns.bytes)?,
                &utf8(colors.bytes)?,
            )?),
            definitions: Arc::new(DefinitionRegistry::load(paths, definitions)?),
        })
    }
}
#[derive(Clone)]
pub struct CapturedSource {
    pub path: String,
    pub snapshot: TextSnapshot,
}
impl CapturedSource {
    pub fn new(snapshot: TextSnapshot) -> Result<Self> {
        Ok(Self {
            path: source_path(&snapshot.origin)?,
            snapshot,
        })
    }
}
#[derive(Default)]
pub struct HotSources {
    sources: BTreeMap<FileId, CapturedSource>,
    order: VecDeque<FileId>,
    bytes: usize,
}
impl HotSources {
    pub fn insert(&mut self, source: CapturedSource) {
        let id = SemanticIds::file(&source.path);
        self.order.retain(|old| *old != id);
        if let Some(old) = self.sources.remove(&id) {
            self.bytes -= old.snapshot.len_bytes();
        }
        if source.snapshot.len_bytes() > 16 * 1024 * 1024 {
            return;
        }
        self.bytes += source.snapshot.len_bytes();
        self.sources.insert(id, source);
        self.order.push_back(id);
        while self.sources.len() > 32 || self.bytes > 64 * 1024 * 1024 {
            if let Some(old) = self.order.pop_front()
                && let Some(source) = self.sources.remove(&old)
            {
                self.bytes -= source.snapshot.len_bytes();
            }
        }
    }
    pub fn remove(&mut self, file: FileId) {
        if let Some(old) = self.sources.remove(&file) {
            self.bytes -= old.snapshot.len_bytes();
        }
        self.order.retain(|id| *id != file);
    }
    pub fn captures(&self) -> Vec<CapturedSource> {
        self.sources.values().cloned().collect()
    }
    pub fn get(&self, file: FileId) -> Option<&CapturedSource> {
        self.sources.get(&file)
    }
}
pub struct NativeTokenReport {
    pub index_revision: u64,
    pub resolution_generation: u64,
    pub document: (DocumentId, Revision),
    pub tokens: Vec<SemanticToken>,
}
pub struct NativeFacade {
    client: StoreClient,
    resources: Arc<IntelligenceResources>,
    generation: WorkspaceGeneration,
    sources: Arc<RwLock<HotSources>>,
    stop: CancellationToken,
}
impl NativeFacade {
    pub fn new(
        client: StoreClient,
        resources: Arc<IntelligenceResources>,
        generation: WorkspaceGeneration,
        sources: Arc<RwLock<HotSources>>,
        stop: CancellationToken,
    ) -> Self {
        Self {
            client,
            resources,
            generation,
            sources,
            stop,
        }
    }
    pub fn snapshot(&self) -> Result<SemanticSnapshot> {
        if self.stop.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        self.client.snapshot()
    }
    pub fn captured_source(&self, path: &str) -> Result<Option<TextSnapshot>> {
        if self.stop.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        Ok(self
            .sources
            .read()
            .map_err(|_| DomainError::Unsupported("hot source catalog poisoned".into()))?
            .get(SemanticIds::file(path))
            .map(|source| source.snapshot.clone()))
    }
    fn current(&self, request: &IntelligenceRequest) -> Result<()> {
        if self.stop.is_cancelled()
            || request.tag.workspace != self.generation
            || request
                .tag
                .document
                .is_some_and(|stamp| stamp != (request.snapshot.id, request.snapshot.revision))
        {
            return Err(DomainError::Cancelled);
        }
        SourceRange::caret(request.position).validate(&request.snapshot.text)?;
        Ok(())
    }
    fn snapshot_for(&self, source: &DocumentSnapshot) -> Result<(SemanticSnapshot, FileId)> {
        let previous = self.snapshot()?;
        let path = source_path(&source.origin)?;
        let file = SemanticIds::file(&path);
        {
            let hot = self
                .sources
                .read()
                .map_err(|_| DomainError::Unsupported("hot source catalog poisoned".into()))?;
            if hot.get(file).is_some_and(|current| {
                current.snapshot.id != source.id || current.snapshot.revision > source.revision
            }) {
                return Err(DomainError::Cancelled);
            }
        }
        let captures = self
            .sources
            .read()
            .map_err(|_| DomainError::Unsupported("hot source catalog poisoned".into()))?
            .captures();
        let mut sources: BTreeMap<_, _> = captures
            .into_iter()
            .map(|capture| {
                (
                    SemanticIds::file(&capture.path),
                    (capture.path, capture.snapshot.to_service_snapshot()),
                )
            })
            .collect();
        sources.insert(file, (path, source.clone()));
        sources.retain(|id, (_, source)| {
            !previous.file(*id).is_some_and(|f| {
                f.content_hash == SemanticIds::content_hash(&source.text)
                    && source
                        .language
                        .as_deref()
                        .is_none_or(|language| f.language_id == canonical_language(language))
            })
        });
        if sources.is_empty() {
            return Ok((previous, file));
        }
        // Unsaved source is resolved in a private worker snapshot. It never
        // overwrites disk-indexed facts or publishes a query's temporary state.
        let mut deltas = vec![];
        for (path, source) in sources.into_values() {
            let language = source
                .language
                .as_deref()
                .map(canonical_language)
                .or_else(|| {
                    previous
                        .file(SemanticIds::file(&path))
                        .map(|f| f.language_id.clone())
                })
                .unwrap_or_else(|| "text".into());
            let document = Document::new(
                source.id,
                source.origin.clone(),
                Some(language.clone()),
                &source.text,
            );
            let report = SemanticAdapterSession::new(&self.resources.grammars, &language).extract(
                &self.resources.definitions,
                &self.resources.keywords,
                &path,
                &document.snapshot(),
                self.generation,
                &self.stop,
            )?;
            let mut delta = report.delta;
            delta.file.parse_version = source.revision.0;
            delta.file.semantic_version = source.revision.0;
            deltas.push(delta);
        }
        let resolved =
            IncrementalSemanticIndex::prepare(previous.project(), deltas, [], &self.stop)?;
        Ok((
            SemanticSnapshot::new(previous.version(), resolved.report.project)?,
            file,
        ))
    }
    pub fn tokens(
        &self,
        source: &DocumentSnapshot,
        range: Option<SourceRange>,
    ) -> Result<NativeTokenReport> {
        if let Some(range) = range {
            range.validate(&source.text)?;
        }
        let (snapshot, file) = self.snapshot_for(source)?;
        Ok(NativeTokenReport {
            index_revision: snapshot.version(),
            resolution_generation: snapshot.resolution_generation(file),
            document: (source.id, source.revision),
            tokens: SemanticTokenService::tokens(file, &snapshot, range),
        })
    }
    fn target(
        &self,
        snapshot: &SemanticSnapshot,
        symbol: &SymbolRecord,
        range: SourceRange,
        declaration: bool,
        request: &DocumentSnapshot,
    ) -> Result<NavigationTarget> {
        let file = snapshot
            .file(symbol.file_id)
            .ok_or_else(|| DomainError::Unsupported("navigation file is absent".into()))?;
        self.target_at(
            &file.path,
            &file.language_id,
            symbol,
            range,
            declaration.then_some(symbol.declaration_range),
            request,
        )
    }
    fn target_at(
        &self,
        path: &str,
        language: &str,
        symbol: &SymbolRecord,
        range: SourceRange,
        declaration_range: Option<SourceRange>,
        request: &DocumentSnapshot,
    ) -> Result<NavigationTarget> {
        let source_id = SemanticIds::file(path);
        let current = source_path(&request.origin)? == path;
        let hot = self
            .sources
            .read()
            .map_err(|_| DomainError::Unsupported("hot source catalog poisoned".into()))?;
        let captured = hot.get(source_id);
        let document = if current {
            Some(request.id)
        } else {
            captured.map(|s| s.snapshot.id)
        };
        drop(hot);
        let missing_source =
            document.is_none() && (path.starts_with("virtual:") || !Path::new(path).is_file());
        Ok(NavigationTarget {
            path: PathBuf::from(path),
            document,
            identifier_range: range,
            location: None,
            declaration_range,
            language: Some(language.into()),
            kind: Some(editor_kind(symbol.kind)),
            name: Some(symbol.name.clone()),
            missing_source,
        })
    }
}
impl IntelligenceProvider for NativeFacade {
    fn definitions(&self, request: &IntelligenceRequest) -> Result<Vec<NavigationTarget>> {
        self.current(request)?;
        let (snapshot, file) = self.snapshot_for(&request.snapshot)?;
        let Some(symbol) = snapshot.symbol_at(file, request.position) else {
            return Ok(vec![]);
        };
        Ok(vec![self.target(
            &snapshot,
            symbol,
            symbol.name_range,
            true,
            &request.snapshot,
        )?])
    }
    fn references(&self, request: &IntelligenceRequest) -> Result<Vec<NavigationTarget>> {
        self.current(request)?;
        let (snapshot, file) = self.snapshot_for(&request.snapshot)?;
        let Some(symbol) = snapshot.symbol_at(file, request.position) else {
            return Ok(vec![]);
        };
        let mut targets = vec![];
        let mut templates = BTreeMap::<String, NavigationTarget>::new();
        for occurrence in snapshot.references(symbol.id) {
            if self.stop.is_cancelled() {
                return Err(DomainError::Cancelled);
            }
            if targets.len() >= 100_000 {
                return Err(DomainError::Unsupported(
                    "reference result limit exceeded".into(),
                ));
            }
            let Some(file) = snapshot.file(occurrence.file_id) else {
                continue;
            };
            let mut target = if let Some(template) = templates.get(&file.path) {
                template.clone()
            } else {
                let target = self.target_at(
                    &file.path,
                    &file.language_id,
                    symbol,
                    occurrence.range,
                    None,
                    &request.snapshot,
                )?;
                templates.insert(file.path.clone(), target.clone());
                target
            };
            target.identifier_range = occurrence.range;
            targets.push(target);
        }
        targets.sort_by(|a, b| {
            (&a.path, a.identifier_range.start, a.identifier_range.end).cmp(&(
                &b.path,
                b.identifier_range.start,
                b.identifier_range.end,
            ))
        });
        targets.dedup();
        Ok(targets)
    }
    fn completions(&self, request: &IntelligenceRequest) -> Result<Vec<CompletionItem>> {
        self.current(request)?;
        let (snapshot, file) = self.snapshot_for(&request.snapshot)?;
        let context = CompletionContext::at(
            file,
            snapshot
                .file(file)
                .map_or("text", |file| file.language_id.as_str()),
            &request.snapshot.text,
            request.position,
        )?;
        Ok(CompletionEngine::native(self.resources.keywords.clone())
            .complete(&context, &snapshot, 50, &self.stop)?
            .iter()
            .map(|candidate| candidate.item(&context))
            .collect())
    }
    fn highlights(&self, snapshot: &DocumentSnapshot) -> Result<Vec<HighlightSpan>> {
        Ok(self
            .tokens(snapshot, None)?
            .tokens
            .iter()
            .flat_map(SemanticToken::highlights)
            .collect())
    }
}
pub fn source_path(origin: &DocumentOrigin) -> Result<String> {
    match origin {
        DocumentOrigin::File(path) => path
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| DomainError::Unsupported("semantic source path is not UTF-8".into())),
        DocumentOrigin::Virtual { owner, key, .. } => Ok(format!(
            "virtual:{}",
            serde_json::to_string(&(owner, key)).expect("string tuple serializes")
        )),
    }
}
fn editor_kind(kind: SymbolKind) -> EditorKind {
    match kind {
        SymbolKind::Class | SymbolKind::Struct => EditorKind::Class,
        SymbolKind::Interface => EditorKind::Interface,
        SymbolKind::Function | SymbolKind::Constructor => EditorKind::Function,
        SymbolKind::Method => EditorKind::Method,
        SymbolKind::Field | SymbolKind::Property | SymbolKind::EnumMember => EditorKind::Field,
        SymbolKind::Enum => EditorKind::Enum,
        SymbolKind::Module | SymbolKind::Namespace => EditorKind::Module,
        SymbolKind::Package => EditorKind::Package,
        SymbolKind::Parameter => EditorKind::Parameter,
        SymbolKind::TypeAlias => EditorKind::TypeAlias,
        _ => EditorKind::Variable,
    }
}
