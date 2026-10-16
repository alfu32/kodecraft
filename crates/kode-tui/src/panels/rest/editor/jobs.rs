//! Captured request-editor jobs; all parsing/materialization stays on workers.
use super::{
    buffers,
    form::{self, BufferKind},
};
use kode_core::{DocumentId, DomainError, JobTag, Result, document::snapshot::TextSnapshot};
use kode_platform::jobs::CancellationToken;
use kode_rest::{
    model::{NodePath, ProjectState, Workspace, editing},
    resolve::RequestResolver,
};
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};

pub struct ModelCapture {
    pub project: ProjectState,
    pub environment_text: Option<TextSnapshot>,
    pub root: PathBuf,
}
impl ModelCapture {
    fn model(self) -> Result<(Workspace, PathBuf)> {
        let mut model = Workspace::from_state(self.project)?;
        if let Some(snapshot) = self.environment_text
            && let Some(id) = model
                .active_environment()
                .map(|environment| environment.id.clone())
        {
            model.update_environment_text(&id, &snapshot.text());
        }
        Ok((model, self.root))
    }
}
pub enum Job {
    Form {
        tag: JobTag,
        key: String,
        path: NodePath,
        capture: Box<ModelCapture>,
    },
    Buffer {
        tag: JobTag,
        structure: u64,
        input: Box<buffers::Input>,
    },
    Change {
        tag: JobTag,
        key: String,
        kind: BufferKind,
        path: NodePath,
        collection: Arc<Value>,
        snapshot: TextSnapshot,
    },
    Derived {
        tag: JobTag,
        key: String,
        path: NodePath,
        revision: u64,
        capture: Box<ModelCapture>,
    },
}
#[derive(Clone)]
pub enum Context {
    Form {
        key: String,
        path: NodePath,
        collection: Arc<Value>,
    },
    Buffer {
        key: String,
        kind: BufferKind,
        structure: u64,
    },
    Change {
        key: String,
        kind: BufferKind,
        document: DocumentId,
    },
    Derived {
        key: String,
        revision: u64,
    },
}
pub enum Change {
    Headers(Value),
    Text(Arc<str>),
}
pub enum Reply {
    Form(Box<(form::Prepared, form::Derived)>),
    Buffer(Box<buffers::Prepared>),
    Change(Change),
    Derived(form::Derived),
}
fn derived(model: &Workspace, path: &NodePath, root: &std::path::Path) -> form::Derived {
    let result = (|| {
        let resolver = RequestResolver::new(
            model.collection(),
            path.clone(),
            root,
            model.active_environment(),
        )?;
        Ok::<_, DomainError>(form::Derived {
            auth: resolver.effective_auth()?,
            headers: resolver.effective_headers()?,
            variables: resolver.effective_variables().cloned().collect(),
            error: None,
        })
    })();
    result.unwrap_or_else(|error| form::Derived {
        error: Some(error.to_string()),
        ..Default::default()
    })
}
impl Job {
    pub fn tag(&self) -> JobTag {
        match self {
            Self::Form { tag, .. }
            | Self::Buffer { tag, .. }
            | Self::Change { tag, .. }
            | Self::Derived { tag, .. } => *tag,
        }
    }
    pub fn context(&self) -> Context {
        match self {
            Self::Form {
                key, path, capture, ..
            } => Context::Form {
                key: key.clone(),
                path: path.clone(),
                collection: capture.project.collection.clone(),
            },
            Self::Buffer {
                input, structure, ..
            } => Context::Buffer {
                key: input.key.clone(),
                kind: input.kind,
                structure: *structure,
            },
            Self::Change {
                key,
                kind,
                snapshot,
                ..
            } => Context::Change {
                key: key.clone(),
                kind: *kind,
                document: snapshot.id,
            },
            Self::Derived { key, revision, .. } => Context::Derived {
                key: key.clone(),
                revision: *revision,
            },
        }
    }
    pub fn prepare(self, cancel: &CancellationToken) -> Result<Reply> {
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        let reply = match self {
            Self::Form { path, capture, .. } => {
                let (model, root) = capture.model()?;
                Reply::Form(Box::new((
                    form::Prepared::from_model(&model, &path)?,
                    derived(&model, &path, &root),
                )))
            }
            Self::Buffer { input, .. } => {
                Reply::Buffer(Box::new(buffers::prepare(*input, cancel)?))
            }
            Self::Change {
                kind,
                path,
                collection,
                snapshot,
                ..
            } => {
                let text = snapshot.text();
                if kind == BufferKind::Headers {
                    let mut model = Workspace::from_state(ProjectState {
                        collection,
                        ..Default::default()
                    })?;
                    editing::headers(&mut model, &path, &text)?;
                    let node = model.node(&path).ok_or(DomainError::Cancelled)?;
                    let headers = if editing::is_request(node, &path) {
                        node.pointer("/request/header")
                    } else {
                        node.get("x-kode-headers")
                    }
                    .cloned()
                    .unwrap_or_else(|| Value::Array(Vec::new()));
                    Reply::Change(Change::Headers(headers))
                } else {
                    Reply::Change(Change::Text(text))
                }
            }
            Self::Derived { path, capture, .. } => {
                let (model, root) = capture.model()?;
                Reply::Derived(derived(&model, &path, &root))
            }
        };
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        Ok(reply)
    }
}
