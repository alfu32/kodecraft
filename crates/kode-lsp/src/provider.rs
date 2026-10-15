//! Editor composition. Synchronous query methods run only on F03 blocking workers;
//! the UI publishes O(1) rope captures and never waits for protocol traffic.
mod convert;
use crate::{
    client::{ClientState, LspClient},
    documents::{
        DocumentStamp, MAX_DOCUMENT_BYTES, MAX_OPEN_BYTES, MAX_OPEN_DOCUMENTS, PositionMapper,
        file_uri,
    },
    service::LspService,
};
use kode_core::{
    DocumentOrigin, DomainError, Result, WorkspaceGeneration,
    document::TextSnapshot,
    intelligence::{
        CompletionItem, DocumentSnapshot, HighlightSpan, IntelligenceProvider, IntelligenceRequest,
        NavigationTarget,
    },
};
use kode_platform::jobs::CancellationToken;
use lsp_types::Uri;
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicU8, Ordering},
};

#[derive(Clone)]
pub struct ProviderProof {
    editor: Arc<EditorLsp>,
    source_epoch: u64,
    service_revision: u64,
}
impl ProviderProof {
    /// Cheap application-thread publication check, including changes that happen
    /// after the worker has queued its reply.
    pub fn is_current(&self) -> bool {
        !self.editor.stop.is_cancelled()
            && self.editor.sources.read().expect("LSP captures lock").epoch == self.source_epoch
            && self.editor.service.revision() == self.service_revision
    }
}
pub struct ProviderUsage(AtomicU8, ProviderProof);
impl ProviderUsage {
    /// Mixed definition/reference operations still depend on the native index
    /// if any query fell back. Preview styling does not affect target provenance.
    pub fn primary_only(&self) -> bool {
        self.0.load(Ordering::Acquire) == 1
    }
    pub fn proof(&self) -> Option<ProviderProof> {
        self.primary_only().then(|| self.1.clone())
    }
}

struct Sources {
    snapshots: Vec<TextSnapshot>,
    epoch: u64,
    changed: CancellationToken,
}
pub struct EditorLsp {
    service: Arc<LspService>,
    generation: WorkspaceGeneration,
    runtime: tokio::runtime::Handle,
    sources: RwLock<Sources>,
    stop: CancellationToken,
}
struct Query {
    snapshot: TextSnapshot,
    client: LspClient,
    uri: Uri,
    mapper: PositionMapper,
    stamp: DocumentStamp,
    epoch: u64,
}
impl EditorLsp {
    pub fn new(
        service: Arc<LspService>,
        generation: WorkspaceGeneration,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        Self {
            service,
            generation,
            runtime,
            sources: RwLock::new(Sources {
                snapshots: vec![],
                epoch: 0,
                changed: CancellationToken::new(),
            }),
            stop: CancellationToken::new(),
        }
    }
    /// Prioritize attached hosts in the caller's order. Oversized/overflow captures
    /// remain native-only. Never flatten, read a file or construct a line index here.
    pub fn replace_sources(&self, snapshots: impl IntoIterator<Item = TextSnapshot>) -> bool {
        let mut accepted = vec![];
        let mut bytes = 0;
        for snapshot in snapshots {
            if !matches!(snapshot.origin, DocumentOrigin::File(_))
                || snapshot.language.is_none()
                || snapshot.len_bytes() > MAX_DOCUMENT_BYTES
                || accepted
                    .iter()
                    .any(|old: &TextSnapshot| old.id == snapshot.id)
                || accepted.len() == MAX_OPEN_DOCUMENTS
                || bytes + snapshot.len_bytes() > MAX_OPEN_BYTES
            {
                continue;
            }
            bytes += snapshot.len_bytes();
            accepted.push(snapshot);
        }
        let mut sources = self.sources.write().expect("LSP captures lock");
        let same = sources.snapshots.len() == accepted.len()
            && accepted
                .iter()
                .all(|new| sources.snapshots.iter().any(|old| same_snapshot(old, new)));
        if same {
            return false;
        }
        sources.changed.cancel();
        sources.changed = CancellationToken::new();
        sources.snapshots = accepted;
        sources.epoch = sources.epoch.wrapping_add(1);
        true
    }
    pub fn stop(&self) {
        self.stop.cancel();
        self.sources
            .read()
            .expect("LSP captures lock")
            .changed
            .cancel();
    }
    pub fn captured_source(&self, origin: &DocumentOrigin) -> Option<TextSnapshot> {
        self.sources
            .read()
            .expect("LSP captures lock")
            .snapshots
            .iter()
            .find(|snapshot| &snapshot.origin == origin)
            .cloned()
    }
    pub fn provider(
        self: &Arc<Self>,
        fallback: Option<Arc<dyn IntelligenceProvider>>,
        cancel: CancellationToken,
    ) -> Arc<dyn IntelligenceProvider> {
        self.tracked_provider(fallback, cancel).0
    }
    pub fn tracked_provider(
        self: &Arc<Self>,
        fallback: Option<Arc<dyn IntelligenceProvider>>,
        cancel: CancellationToken,
    ) -> (Arc<dyn IntelligenceProvider>, Arc<ProviderUsage>) {
        let usage = Arc::new(ProviderUsage(
            AtomicU8::new(0),
            ProviderProof {
                editor: self.clone(),
                source_epoch: self.sources.read().expect("LSP captures lock").epoch,
                service_revision: self.service.revision(),
            },
        ));
        (
            Arc::new(Composite {
                primary: self.clone(),
                fallback,
                cancel,
                usage: usage.clone(),
            }),
            usage,
        )
    }
    /// A single coalesced app job owns background lifecycle synchronization.
    /// Requests also synchronize their exact capture before sending a query.
    pub async fn synchronize(&self, cancel: &CancellationToken) -> Result<()> {
        let snapshots = self
            .sources
            .read()
            .expect("LSP captures lock")
            .snapshots
            .clone();
        for client in self.service.clients() {
            let selected: Vec<_> = snapshots
                .iter()
                .filter(|snapshot| {
                    snapshot
                        .language
                        .as_deref()
                        .and_then(|language| self.service.for_language(language))
                        .is_some_and(|selected| selected.process_id() == client.process_id())
                })
                .collect();
            for origin in client.document_origins() {
                if !selected.iter().any(|snapshot| snapshot.origin == origin) {
                    tokio::select! { biased;_=self.stop.cancelled()=>return Err(DomainError::Cancelled),_=cancel.cancelled()=>return Err(DomainError::Cancelled),result=client.close_document(origin)=>result? };
                }
            }
            for snapshot in selected {
                if cancel.is_cancelled() || self.stop.is_cancelled() {
                    return Err(DomainError::Cancelled);
                }
                let DocumentOrigin::File(path) = &snapshot.origin else {
                    continue;
                };
                let uri = file_uri(path)?;
                if client.document_stamp(&uri).is_some_and(|stamp| {
                    stamp.document == snapshot.id && stamp.revision >= snapshot.revision
                }) {
                    continue;
                }
                tokio::select! {biased;_=self.stop.cancelled()=>return Err(DomainError::Cancelled),_=cancel.cancelled()=>return Err(DomainError::Cancelled),result=client.sync_document((*snapshot).clone(),false,cancel)=>{result?;}};
            }
        }
        Ok(())
    }
    pub fn saved(&self, snapshot: TextSnapshot) -> Result<()> {
        let client = snapshot
            .language
            .as_deref()
            .and_then(|language| self.service.for_language(language))
            .ok_or_else(|| {
                DomainError::Unsupported("no running server for saved document".into())
            })?;
        self.runtime.block_on(async {
            tokio::select! {biased;_=self.stop.cancelled()=>Err(DomainError::Cancelled),result=client.sync_document(snapshot,true,&self.stop)=>result.map(|_|())}
        })
    }
    fn request<T>(
        &self,
        request: &IntelligenceRequest,
        cancel: &CancellationToken,
        query: impl AsyncFnOnce(&LspClient, &Uri, lsp_types::Position, &CancellationToken) -> Result<T>,
    ) -> Result<(Query, T)> {
        if request.tag.workspace != self.generation
            || cancel.is_cancelled()
            || self.stop.is_cancelled()
        {
            return Err(DomainError::Cancelled);
        }
        let (snapshot, epoch, changed) = {
            let sources = self.sources.read().expect("LSP captures lock");
            let snapshot = sources
                .snapshots
                .iter()
                .find(|snapshot| {
                    snapshot.id == request.snapshot.id
                        && snapshot.revision == request.snapshot.revision
                        && snapshot.origin == request.snapshot.origin
                        && snapshot.language == request.snapshot.language
                })
                .cloned()
                .ok_or(DomainError::Cancelled)?;
            (snapshot, sources.epoch, sources.changed.clone())
        };
        let client = snapshot
            .language
            .as_deref()
            .and_then(|language| self.service.for_language(language))
            .ok_or_else(|| DomainError::Unsupported("no running server for document".into()))?;
        let DocumentOrigin::File(path) = &snapshot.origin else {
            return Err(DomainError::NoSaveTarget);
        };
        let uri = file_uri(path)?;
        let (stamp, mapper, result) = self.runtime.block_on(async {
            tokio::select! {biased;
                _=self.stop.cancelled()=>Err(DomainError::Cancelled),
                _=changed.cancelled()=>Err(DomainError::Cancelled),
                _=cancel.cancelled()=>Err(DomainError::Cancelled),
                result=async {
                    // A target may live in another unsaved buffer. Put every
                    // selected replica ahead of this query on the server wire.
                    self.synchronize(cancel).await?;
                    let stamp = client.sync_document(snapshot.clone(),false,cancel).await?;
                    let mapper = client.document_mapper(&uri).ok_or(DomainError::Cancelled)?;
                    let position = mapper.position(request.position)?;
                    let result = query(&client,&uri,position,cancel).await?;
                    Ok((stamp,mapper,result))
                }=>result,
            }
        })?;
        let query = Query {
            snapshot,
            client,
            uri,
            mapper,
            stamp,
            epoch,
        };
        self.validate(&query)?;
        Ok((query, result))
    }
    fn validate(&self, query: &Query) -> Result<()> {
        let sources = self.sources.read().expect("LSP captures lock");
        if !self.stop.is_cancelled()
            && sources.epoch == query.epoch
            && sources
                .snapshots
                .iter()
                .any(|snapshot| same_snapshot(snapshot, &query.snapshot))
            && query.client.state() == ClientState::Running
            && query.client.document_stamp(&query.uri) == Some(query.stamp)
            && query
                .snapshot
                .language
                .as_deref()
                .and_then(|language| self.service.for_language(language))
                .is_some_and(|client| client.process_id() == query.client.process_id())
        {
            Ok(())
        } else {
            Err(DomainError::Cancelled)
        }
    }
    fn definitions(
        &self,
        request: &IntelligenceRequest,
        cancel: &CancellationToken,
    ) -> Result<Vec<NavigationTarget>> {
        let (query, response) =
            self.request(request, cancel, async |client, uri, position, cancel| {
                client.definitions(uri, position, cancel).await
            })?;
        let locations = convert::definitions(response);
        let targets = convert::targets(self, &query, locations)?;
        self.validate(&query)?;
        Ok(targets)
    }
    fn references(
        &self,
        request: &IntelligenceRequest,
        cancel: &CancellationToken,
    ) -> Result<Vec<NavigationTarget>> {
        let (query, response) =
            self.request(request, cancel, async |client, uri, position, cancel| {
                client.references(uri, position, cancel).await
            })?;
        let locations = response
            .into_iter()
            .map(|location| (location.uri, location.range, None))
            .collect();
        let targets = convert::targets(self, &query, locations)?;
        self.validate(&query)?;
        Ok(targets)
    }
    fn completions(
        &self,
        request: &IntelligenceRequest,
        cancel: &CancellationToken,
    ) -> Result<Vec<CompletionItem>> {
        let (query, response) =
            self.request(request, cancel, async |client, uri, position, cancel| {
                client.completions(uri, position, cancel).await
            })?;
        let items = convert::completions(&query, request, response)?;
        self.validate(&query)?;
        Ok(items)
    }
}
fn same_snapshot(left: &TextSnapshot, right: &TextSnapshot) -> bool {
    left.id == right.id
        && left.revision == right.revision
        && left.origin == right.origin
        && left.language == right.language
}
struct Composite {
    primary: Arc<EditorLsp>,
    fallback: Option<Arc<dyn IntelligenceProvider>>,
    cancel: CancellationToken,
    usage: Arc<ProviderUsage>,
}
impl Composite {
    fn cancelled(&self, request: &IntelligenceRequest) -> bool {
        self.cancel.is_cancelled()
            || self.primary.stop.is_cancelled()
            || request.tag.workspace != self.primary.generation
    }
}
impl IntelligenceProvider for Composite {
    fn definitions(&self, request: &IntelligenceRequest) -> Result<Vec<NavigationTarget>> {
        if self.cancelled(request) {
            return Err(DomainError::Cancelled);
        }
        let hits = self.primary.definitions(request, &self.cancel);
        if self.cancelled(request) {
            return Err(DomainError::Cancelled);
        }
        if hits.as_ref().is_ok_and(|hits| !hits.is_empty()) {
            self.usage.0.fetch_or(1, Ordering::AcqRel);
            return hits;
        }
        self.usage.0.fetch_or(2, Ordering::AcqRel);
        self.fallback
            .as_ref()
            .map_or(hits, |fallback| fallback.definitions(request))
    }
    fn references(&self, request: &IntelligenceRequest) -> Result<Vec<NavigationTarget>> {
        if self.cancelled(request) {
            return Err(DomainError::Cancelled);
        }
        let hits = self.primary.references(request, &self.cancel);
        if self.cancelled(request) {
            return Err(DomainError::Cancelled);
        }
        if hits.as_ref().is_ok_and(|hits| !hits.is_empty()) {
            self.usage.0.fetch_or(1, Ordering::AcqRel);
            return hits;
        }
        self.usage.0.fetch_or(2, Ordering::AcqRel);
        self.fallback
            .as_ref()
            .map_or(hits, |fallback| fallback.references(request))
    }
    fn completions(&self, request: &IntelligenceRequest) -> Result<Vec<CompletionItem>> {
        if self.cancelled(request) {
            return Err(DomainError::Cancelled);
        }
        let hits = self.primary.completions(request, &self.cancel);
        if self.cancelled(request) {
            return Err(DomainError::Cancelled);
        }
        if hits.as_ref().is_ok_and(|hits| !hits.is_empty()) {
            self.usage.0.fetch_or(1, Ordering::AcqRel);
            return hits;
        }
        self.usage.0.fetch_or(2, Ordering::AcqRel);
        self.fallback
            .as_ref()
            .map_or(hits, |fallback| fallback.completions(request))
    }
    fn highlights(&self, snapshot: &DocumentSnapshot) -> Result<Vec<HighlightSpan>> {
        self.fallback
            .as_ref()
            .map_or_else(|| Ok(vec![]), |fallback| fallback.highlights(snapshot))
    }
}
impl Drop for EditorLsp {
    fn drop(&mut self) {
        self.stop();
    }
}
