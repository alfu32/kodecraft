//! Worker preparation of shared request editor documents and captured overrides.
use super::{form::BufferKind, headers};
use crate::editor::host::{
    EditorDocuments, EditorView, ExecutionAction, HostDescriptor, HostRole, QueuedEditorHost,
};
use kode_core::{
    DocumentId, DocumentOrigin, DomainError, Result, WorkspaceGeneration,
    document::{Document, snapshot::TextSnapshot},
};
use kode_platform::{
    clipboard::ClipboardService,
    jobs::{CancellationToken, Clock},
};
use kode_rest::{
    format::language_for_content_type,
    model::{
        Environment, NodePath, ProjectState, Workspace,
        editing::{self, Field},
    },
    resolve::{Protocol, RequestResolver},
};
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};

impl BufferKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Headers => "headers",
            Self::RawBody => "body",
            Self::GraphqlQuery => "graphql",
            Self::GraphqlVariables => "graphql-variables",
            Self::RawRequest => "raw-request",
        }
    }
    pub fn field(self) -> Option<Field> {
        match self {
            Self::RawBody => Some(Field::RawBody),
            Self::GraphqlQuery => Some(Field::GraphqlQuery),
            Self::GraphqlVariables => Some(Field::GraphqlVariables),
            _ => None,
        }
    }
    pub fn editable(self) -> bool {
        self != Self::RawRequest
    }
}
#[derive(Clone)]
pub struct Override {
    pub path: NodePath,
    pub kind: BufferKind,
    pub snapshot: TextSnapshot,
}
/// A captured rope is authoritative even when semantic publication is pending.
/// Invalid headers fail atomically. Persistence callers may retain that draft
/// separately; execution/preview callers must propagate the failure before I/O.
pub fn apply_override(
    model: &mut Workspace,
    capture: &Override,
    cancel: &CancellationToken,
) -> Result<()> {
    if cancel.is_cancelled() {
        return Err(DomainError::Cancelled);
    }
    if !capture.kind.editable() || model.node(&capture.path).is_none() {
        return Ok(());
    }
    let text = capture.snapshot.text();
    if capture.kind == BufferKind::Headers {
        editing::headers(model, &capture.path, &text)?;
    } else if let Some(field) = capture.kind.field() {
        editing::field(model, &capture.path, field, &text)?;
    }
    Ok(())
}
pub struct Input {
    pub collection: Arc<Value>,
    pub path: NodePath,
    pub kind: BufferKind,
    pub key: String,
    pub root: PathBuf,
    pub environment: Option<Environment>,
    pub environment_text: Option<TextSnapshot>,
    pub overrides: Vec<Override>,
    pub draft: Option<Arc<str>>,
}
pub struct Prepared {
    pub document: Document,
    pub descriptor: HostDescriptor,
    pub kind: BufferKind,
    pub key: String,
    pub warnings: Vec<kode_core::LineIndex>,
}
impl Prepared {
    /// Application thread only: bind a previously prepared rope to the common
    /// editor. Construction performs no materialization or file/network I/O.
    pub fn install(
        self,
        documents: &mut EditorDocuments,
        generation: WorkspaceGeneration,
        clipboard: Arc<ClipboardService>,
        clock: Arc<dyn Clock>,
    ) -> Result<EditorView<QueuedEditorHost>> {
        let id = documents.insert(self.document, &self.descriptor, generation, None)?;
        let mut host = QueuedEditorHost::new(self.descriptor);
        host.warnings = self.warnings;
        if self.kind == BufferKind::Headers {
            host.external = Some(Arc::new(headers::HeaderNames));
        }
        EditorView::new(id, documents, host, generation, clipboard, clock)
    }
}
fn invalid(message: &str) -> DomainError {
    DomainError::Parse {
        format: "REST editor",
        message: message.into(),
    }
}
pub fn prepare(input: Input, cancel: &CancellationToken) -> Result<Prepared> {
    if cancel.is_cancelled() {
        return Err(DomainError::Cancelled);
    }
    let environment_id = input
        .environment
        .as_ref()
        .map(|environment| environment.id.clone());
    let mut model = Workspace::from_state(ProjectState {
        collection: input.collection,
        environments: Arc::new(input.environment.into_iter().collect()),
        active_environment_id: environment_id.clone(),
        ..Default::default()
    })?;
    if let (Some(id), Some(snapshot)) = (environment_id, input.environment_text) {
        model.update_environment_text(&id, &snapshot.text());
    }
    if model.node(&input.path).is_none() {
        return Err(invalid("REST item no longer exists"));
    }
    if input.kind != BufferKind::Headers
        && !editing::is_request(model.node(&input.path).expect("checked path"), &input.path)
    {
        return Err(invalid("Selected node is not a request"));
    }
    let body = model
        .node(&input.path)
        .and_then(|node| node.pointer("/request/body"));
    let mut content_type = None;
    let (text, language) = if input.kind == BufferKind::RawRequest {
        for capture in input
            .overrides
            .iter()
            .filter(|capture| capture.path == input.path)
        {
            apply_override(&mut model, capture, cancel)?;
        }
        let request = RequestResolver::new(
            model.collection(),
            input.path.clone(),
            &input.root,
            model.active_environment(),
        )?
        .plan()?
        .materialize(64 * 1024 * 1024, cancel)?;
        let mut text = request.preview(10 * 1024 * 1024)?.text;
        let notice = if request.request.protocol == Protocol::WebSocket {
            "WebSocket opens receive-only. The configured body is not sent as an initial message."
        } else {
            "Logical request preview. Transport may normalize framing/authority headers; captured Transaction details are authoritative after execution."
        };
        text = format!("{notice}\n\n{text}");
        (text, Some("http".to_string()))
    } else {
        let text = if let Some(capture) = input
            .overrides
            .iter()
            .find(|capture| capture.path == input.path && capture.kind == input.kind)
        {
            capture.snapshot.text().to_string()
        } else if let Some(draft) = input.draft {
            draft.to_string()
        } else {
            match input.kind {
                BufferKind::Headers => editing::headers_text(
                    model.node(&input.path).expect("checked path"),
                    &input.path,
                ),
                BufferKind::RawBody => editing::text(body.and_then(|body| body.get("raw"))),
                BufferKind::GraphqlQuery => {
                    editing::text(body.and_then(|body| body.pointer("/graphql/query")))
                }
                BufferKind::GraphqlVariables => {
                    match body.and_then(|body| body.pointer("/graphql/variables")) {
                        Some(Value::String(text)) => text.clone(),
                        Some(value) => serde_json::to_string_pretty(value)
                            .map_err(|_| invalid("Cannot display GraphQL variables"))?,
                        None => "{}".into(),
                    }
                }
                BufferKind::RawRequest => unreachable!(),
            }
        };
        let language = match input.kind {
            BufferKind::Headers => Some("text".into()),
            BufferKind::GraphqlQuery => Some("graphql".into()),
            BufferKind::GraphqlVariables => Some("json".into()),
            BufferKind::RawBody => {
                let resolver = RequestResolver::new(
                    model.collection(),
                    input.path.clone(),
                    &input.root,
                    model.active_environment(),
                )?;
                let headers = resolver.effective_headers()?;
                content_type = headers
                    .iter()
                    .rev()
                    .find(|header| {
                        !header.disabled && header.name.eq_ignore_ascii_case("content-type")
                    })
                    .map(|header| header.value.clone());
                if let Some(kind) = &content_type {
                    let resolved = resolver.resolve(kind)?;
                    if resolved.unresolved.is_empty() && resolved.cycles.is_empty() {
                        content_type = Some(resolved.value);
                    }
                }
                if content_type.is_none() {
                    content_type = body
                        .and_then(|body| body.pointer("/options/raw/language"))
                        .and_then(Value::as_str)
                        .and_then(|language| match language {
                            "json" => Some("application/json"),
                            "xml" => Some("application/xml"),
                            "html" => Some("text/html"),
                            "javascript" | "js" => Some("application/javascript"),
                            "yaml" | "yml" => Some("application/yaml"),
                            _ => None,
                        })
                        .map(str::to_string);
                }
                language_for_content_type(content_type.as_deref())
                    .map(str::to_string)
                    .or_else(|| {
                        body.and_then(|body| body.pointer("/options/raw/language"))
                            .and_then(Value::as_str)
                            .map(|language| {
                                match language {
                                    "js" => "javascript",
                                    "yml" => "yaml",
                                    other => other,
                                }
                                .to_string()
                            })
                    })
            }
            BufferKind::RawRequest => None,
        };
        (text, language)
    };
    if cancel.is_cancelled() {
        return Err(DomainError::Cancelled);
    }
    let role = match input.kind {
        BufferKind::Headers => HostRole::RestHeaders,
        BufferKind::RawRequest => HostRole::RawRequest,
        _ => HostRole::RestBody,
    };
    let mut descriptor = HostDescriptor::new(
        role,
        "rest-request".into(),
        format!("{}:{}", input.key, input.kind.name()),
        format!("REST {}", input.kind.name()),
        input.root,
    );
    descriptor.language = language;
    descriptor.grammar = descriptor.language.clone().unwrap_or_else(|| "text".into());
    descriptor.mime = content_type;
    descriptor.execute = Some(ExecutionAction::SendRequest);
    let document = Document::new(
        DocumentId::fresh()?,
        DocumentOrigin::Virtual {
            owner: descriptor.owner.clone(),
            key: descriptor.key.clone(),
            label: descriptor.label.clone(),
        },
        descriptor.language.clone(),
        &text,
    );
    let warnings = if input.kind == BufferKind::Headers {
        kode_rest::headers::duplicate_lines(&text)
            .into_iter()
            .map(kode_core::LineIndex)
            .collect()
    } else {
        Vec::new()
    };
    Ok(Prepared {
        document,
        descriptor,
        kind: input.kind,
        key: input.key,
        warnings,
    })
}
