//! Application-owned model, virtual buffers and immutable worker captures.
use super::{
    editor::{
        drafts,
        form::{BufferKind, Form},
        jobs,
    },
    sidebar::Sidebar,
};
use crate::{app::workspace::CapturedDocument, editor::host::HostDescriptor};
use kode_core::{
    DocumentId, DocumentOrigin, DomainError, JobTag, Result,
    document::{Document, snapshot::TextSnapshot},
};
use kode_platform::jobs::CancellationToken;
use kode_rest::{
    import::{Import, Kind, export_file, read_file},
    model::{Environment, EnvironmentValue, NodePath, ProjectState, Workspace},
};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};
const DRAFTS: &str = "x-kode-rs-environment-buffers";
#[derive(Clone, PartialEq, Eq)]
pub enum DocumentKind {
    Environment(String),
    Node(NodePath),
    Notices,
    Buffer { key: String, kind: BufferKind },
}
pub struct Entry {
    pub kind: DocumentKind,
    pub descriptor: HostDescriptor,
}
pub struct RestState {
    pub response: super::editor::response::Panel,
    pub form: Option<Form>,
    pub forms: HashMap<String, Form>,
    pub markers: HashMap<DocumentId, String>,
    pub buffers: HashMap<(String, BufferKind), DocumentId>,
    pub dirty_buffers: HashSet<DocumentId>,
    pub pending_changes: HashMap<DocumentId, JobTag>,
    pub pending_derived: Option<JobTag>,
    pub derived_revision: u64,
    pub buffer_warnings: HashMap<DocumentId, Vec<kode_core::LineIndex>>,
    pub model: Workspace,
    pub sidebar: Sidebar,
    pub documents: HashMap<DocumentId, Entry>,
    pub pending_open: Option<JobTag>,
    pub ready_open: Option<DocumentId>,
    pub open_focus: Option<(Option<crate::focus::FocusId>, bool)>,
    pub observed_revision: u64,
    pub persistence_enabled: bool,
    pub warning: Option<String>,
    pub pending_environments: HashMap<DocumentId, JobTag>,
    pub drafts: HashMap<String, Arc<str>>,
    pub request_drafts: drafts::Drafts,
    pub staged_buffers: HashMap<String, TextSnapshot>,
    pub run: super::run::RunState,
    pub exports: HashMap<PathBuf, kode_core::services::DiskIdentity>,
}
impl RestState {
    pub fn new(value: &Value) -> Result<Self> {
        let mut parsed = if value.is_null() {
            Ok(ProjectState::default())
        } else {
            serde_json::from_value::<ProjectState>(value.clone()).map_err(|_| DomainError::Parse {
                format: "REST project state",
                message: "Invalid saved REST data; original state retained".into(),
            })
        };
        let mut warning = None;
        let mut persistence_enabled = true;
        let mut drafts = HashMap::new();
        let mut request_drafts = drafts::Drafts::new();
        if let Ok(project) = &mut parsed {
            match read_drafts(project) {
                Ok(saved) => drafts = saved,
                Err(error) => {
                    warning = Some(error.to_string());
                    persistence_enabled = false;
                }
            }
            // Large owned buffers live behind Arcs, outside the model's unknown
            // JSON metadata map, so capturing a keystroke never clones them.
            project.extra.remove(DRAFTS);
            match drafts::read(project) {
                Ok(saved) => request_drafts = saved,
                Err(error) => {
                    warning = Some(error.to_string());
                    persistence_enabled = false;
                }
            }
            project.extra.remove(drafts::KEY);
        }
        let model = match parsed.and_then(Workspace::from_state) {
            Ok(model) => model,
            Err(error) => {
                warning = Some(error.to_string());
                persistence_enabled = false;
                Workspace::default()
            }
        };
        let observed_revision = model.revision();
        let response = super::editor::response::Panel {
            pretty: model.ui().pretty_print_responses,
            ..Default::default()
        };
        Ok(Self {
            response,
            form: None,
            forms: HashMap::new(),
            markers: HashMap::new(),
            buffers: HashMap::new(),
            dirty_buffers: HashSet::new(),
            pending_changes: HashMap::new(),
            pending_derived: None,
            derived_revision: 0,
            buffer_warnings: HashMap::new(),
            sidebar: Sidebar::new(&model)?,
            model,
            documents: HashMap::new(),
            pending_open: None,
            ready_open: None,
            open_focus: None,
            observed_revision,
            persistence_enabled,
            warning,
            pending_environments: HashMap::new(),
            drafts,
            request_drafts,
            staged_buffers: HashMap::new(),
            run: super::run::RunState::default(),
            exports: HashMap::new(),
        })
    }
    pub fn environment_document(&self, environment: &str) -> Option<DocumentId> {
        self.documents.iter().find_map(|(id, entry)| {
            matches!(&entry.kind,DocumentKind::Environment(key)if key==environment).then_some(*id)
        })
    }
    pub fn path_for_key(&self, key: &str) -> Option<NodePath> {
        self.run
            .keys
            .iter()
            .find_map(|(path, candidate)| (candidate == key).then(|| path.clone()))
    }
    pub fn capture(&self) -> Option<Capture> {
        self.persistence_enabled.then(|| Capture {
            project: self.model.project_state(),
            staged_buffers: self.staged_buffers.clone(),
            drafts: self.drafts.clone(),
            request_drafts: self.request_drafts.clone(),
            requests: self
                .documents
                .iter()
                .filter_map(|(id, entry)| {
                    if let DocumentKind::Buffer { key, kind } = &entry.kind
                        && kind.editable()
                    {
                        return Some((
                            *id,
                            (
                                self.path_for_key(key)?,
                                *kind,
                                self.dirty_buffers.contains(id),
                            ),
                        ));
                    }
                    None
                })
                .collect(),
            environments: self
                .documents
                .iter()
                .filter_map(|(id, entry)| {
                    if let DocumentKind::Environment(key) = &entry.kind {
                        Some((*id, key.clone()))
                    } else {
                        None
                    }
                })
                .collect(),
        })
    }
}
fn read_drafts(state: &ProjectState) -> Result<HashMap<String, Arc<str>>> {
    let Some(value) = state.extra.get(DRAFTS) else {
        return Ok(HashMap::new());
    };
    let object = value
        .as_object()
        .ok_or_else(|| invalid("Invalid environment buffers; original REST state retained"))?;
    object
        .iter()
        .map(|(key, value)| {
            Ok((
                key.clone(),
                Arc::from(value.as_str().ok_or_else(|| {
                    invalid("Invalid environment buffer; original REST state retained")
                })?),
            ))
        })
        .collect()
}
pub struct Capture {
    project: ProjectState,
    staged_buffers: HashMap<String, TextSnapshot>,
    drafts: HashMap<String, Arc<str>>,
    environments: HashMap<DocumentId, String>,
    request_drafts: drafts::Drafts,
    requests: HashMap<DocumentId, (NodePath, BufferKind, bool)>,
}
impl Capture {
    /// Worker-only: materialize captured ropes, merge typed metadata and retain
    /// unfinished/comment lines without putting them in Postman export.
    pub fn into_value(self, documents: &[CapturedDocument]) -> Result<Value> {
        let mut model = Workspace::from_state(self.project)?;
        let mut drafts = self.drafts;
        for (environment, snapshot) in self.staged_buffers {
            if model.environment(&environment).is_some() {
                let text = snapshot.text();
                model.update_environment_text(&environment, &text);
                drafts.insert(environment, text);
            }
        }
        for captured in documents {
            let Some(environment) = self.environments.get(&captured.document.id()) else {
                continue;
            };
            if model.environment(environment).is_none() {
                continue;
            }
            let text = captured.document.snapshot().text().to_string();
            model.update_environment_text(environment, &text);
            drafts.insert(environment.clone(), Arc::from(text));
        }
        drafts.retain(|id, _| model.environment(id).is_some());
        let mut request_drafts = self.request_drafts;
        for captured in documents {
            if let Some((path, kind, dirty)) = self.requests.get(&captured.document.id())
                && (*dirty || captured.document.revision().0 > 0)
                && model.node(path).is_some()
            {
                request_drafts.insert(
                    (path.clone(), kind.name().into()),
                    captured.document.snapshot().text(),
                );
            }
        }
        request_drafts.retain(|(path, _), _| model.node(path).is_some());
        for ((path, name), text) in &request_drafts {
            if let Some(kind) = drafts::kind(name) {
                // Unfinished text is saved verbatim; typed JSON retains its last valid value.
                let _ = drafts::apply_text(&mut model, path, kind, text);
            }
        }
        let mut state = model.project_state();
        if !request_drafts.is_empty() {
            state
                .extra
                .insert(drafts::KEY.into(), drafts::write(request_drafts));
        }
        if !drafts.is_empty() || state.extra.contains_key(DRAFTS) {
            state.extra.insert(
                DRAFTS.into(),
                Value::Object(
                    drafts
                        .into_iter()
                        .map(|(key, text)| (key, Value::String(text.to_string())))
                        .collect(),
                ),
            );
        }
        serde_json::to_value(state).map_err(|_| invalid("Cannot serialize REST project state"))
    }
}
pub enum OpenData {
    Environment {
        environment: Box<Environment>,
        draft: Option<Arc<str>>,
        override_text: Option<TextSnapshot>,
    },
    Node {
        collection: Arc<Value>,
        path: NodePath,
    },
    Notices(Arc<Value>),
}
pub enum Work {
    Editor(Box<jobs::Job>),
    Run(Box<super::run::Prepare>),
    Open {
        tag: JobTag,
        descriptor: HostDescriptor,
        kind: DocumentKind,
        data: OpenData,
        expected: Option<Arc<Value>>,
    },
    Environment {
        tag: JobTag,
        environment: String,
        project: ProjectState,
        snapshot: TextSnapshot,
    },
    Import {
        tag: JobTag,
        nonce: kode_core::RequestId,
        kind: Kind,
        path: PathBuf,
        parent: NodePath,
        expected: Arc<Value>,
    },
    Export {
        tag: JobTag,
        nonce: kode_core::RequestId,
        path: PathBuf,
        collection: Arc<Value>,
        expected: Option<kode_core::services::DiskIdentity>,
    },
}
#[derive(Clone)]
pub enum Context {
    Editor(jobs::Context),
    Run {
        structure: u64,
    },
    Open {
        descriptor: HostDescriptor,
        kind: DocumentKind,
        expected: Option<Arc<Value>>,
    },
    Environment {
        document: DocumentId,
        environment: String,
    },
    Import {
        nonce: kode_core::RequestId,
        parent: NodePath,
        expected: Arc<Value>,
    },
    Export {
        nonce: kode_core::RequestId,
        path: PathBuf,
    },
}
pub enum Reply {
    Editor(Box<jobs::Reply>),
    Run(Box<super::run::Prepared>),
    Opened(Document),
    Environment(Vec<EnvironmentValue>),
    Imported(Import),
    Exported(kode_core::services::DiskIdentity),
}
impl Work {
    pub fn context(&self) -> Context {
        match self {
            Self::Editor(job) => Context::Editor(job.context()),
            Self::Run(work) => Context::Run {
                structure: work.structure,
            },
            Self::Open {
                descriptor,
                kind,
                expected,
                ..
            } => Context::Open {
                descriptor: descriptor.clone(),
                kind: kind.clone(),
                expected: expected.clone(),
            },
            Self::Environment {
                environment,
                snapshot,
                ..
            } => Context::Environment {
                document: snapshot.id,
                environment: environment.clone(),
            },
            Self::Import {
                nonce,
                parent,
                expected,
                ..
            } => Context::Import {
                nonce: *nonce,
                parent: parent.clone(),
                expected: expected.clone(),
            },
            Self::Export { nonce, path, .. } => Context::Export {
                nonce: *nonce,
                path: path.clone(),
            },
        }
    }
    pub fn tag(&self) -> JobTag {
        match self {
            Self::Editor(job) => job.tag(),
            Self::Run(work) => work.tag,
            Self::Open { tag, .. }
            | Self::Environment { tag, .. }
            | Self::Import { tag, .. }
            | Self::Export { tag, .. } => *tag,
        }
    }
    pub fn prepare(self, cancel: &CancellationToken) -> Result<(Context, Reply)> {
        let critical = matches!(&self, Self::Export { .. });
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        let result = match self {
            Self::Editor(job) => {
                let context = Context::Editor(job.context());
                (context, Reply::Editor(Box::new(job.prepare(cancel)?)))
            }
            Self::Run(work) => {
                let context = Context::Run {
                    structure: work.structure,
                };
                (context, Reply::Run(Box::new(work.prepare(cancel)?)))
            }
            Self::Open {
                descriptor,
                kind,
                data,
                expected,
                ..
            } => {
                let text = match data {
                    OpenData::Environment {
                        environment,
                        draft,
                        override_text,
                    } => {
                        if let Some(snapshot) = override_text {
                            snapshot.text().to_string()
                        } else if let Some(draft) = draft {
                            draft.to_string()
                        } else {
                            kode_rest::model::environment::format(&environment.values)
                        }
                    }
                    OpenData::Node { collection, path } => {
                        let model = Workspace::from_state(ProjectState {
                            collection,
                            ..Default::default()
                        })?;
                        let node = model
                            .node(&path)
                            .ok_or_else(|| invalid("Selected REST item no longer exists"))?;
                        serde_json::to_string_pretty(node)
                            .map_err(|_| invalid("Cannot display selected REST item"))?
                    }
                    OpenData::Notices(collection) => {
                        // Read retained metadata, including previous imports and
                        // restored sessions. The transient last-import notice
                        // vector is not the authoritative source.
                        let mut pending = vec![(NodePath::default(), collection.as_ref())];
                        let mut notices = Vec::new();
                        while let Some((path, node)) = pending.pop() {
                            if cancel.is_cancelled() {
                                return Err(DomainError::Cancelled);
                            }
                            if let Some(stored) = node
                                .get("x-kode-openapi")
                                .and_then(|source| source.get("notices"))
                            {
                                notices.push(serde_json::json!({"collectionPath":path.encode(),"notices":stored}));
                            }
                            if let Some(items) = node.get("item").and_then(Value::as_array) {
                                pending.extend(
                                    items
                                        .iter()
                                        .enumerate()
                                        .rev()
                                        .map(|(index, item)| (path.child(index), item)),
                                );
                            }
                        }
                        if notices.is_empty() {
                            "No retained OpenAPI import notices".into()
                        } else {
                            serde_json::to_string_pretty(&notices)
                                .map_err(|_| invalid("Cannot display import notices"))?
                        }
                    }
                };
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
                (
                    Context::Open {
                        descriptor,
                        kind,
                        expected,
                    },
                    Reply::Opened(document),
                )
            }
            Self::Environment {
                environment,
                project,
                snapshot,
                ..
            } => {
                let mut model = Workspace::from_state(project)?;
                if model.environment(&environment).is_none() {
                    return Err(DomainError::Cancelled);
                }
                model.update_environment_text(&environment, snapshot.text().as_ref());
                let values = model
                    .environment(&environment)
                    .expect("existing environment")
                    .values
                    .clone();
                (
                    Context::Environment {
                        document: snapshot.id,
                        environment,
                    },
                    Reply::Environment(values),
                )
            }
            Self::Import {
                nonce,
                kind,
                path,
                parent,
                expected,
                ..
            } => {
                let imported = read_file(&path, cancel)?;
                if imported.kind != kind {
                    return Err(invalid(
                        "Selected file does not match the chosen Postman/OpenAPI import type",
                    ));
                }
                (
                    Context::Import {
                        nonce,
                        parent,
                        expected,
                    },
                    Reply::Imported(imported),
                )
            }
            Self::Export {
                nonce,
                path,
                collection,
                expected,
                ..
            } => {
                let expected = match expected {
                    Some(identity) => Some(identity),
                    None => kode_rest::import::export_target_identity(&path, cancel)?,
                };
                let identity = export_file(&path, &collection, expected.as_ref(), cancel)?;
                (Context::Export { nonce, path }, Reply::Exported(identity))
            }
        };
        if !critical && cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        Ok(result)
    }
}
fn invalid(message: &str) -> DomainError {
    DomainError::Parse {
        format: "REST UI",
        message: message.into(),
    }
}
