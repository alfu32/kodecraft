//! Application adapter for the sidebar's run-selection action. R08 owns the
//! complete runner panel and workflow qualification.
use super::sidebar::TreeChange;
use kode_core::{DomainError, JobTag, RequestId, Result};
use kode_platform::jobs::CancellationToken;
use kode_rest::{
    http::Response,
    model::{Environment, NodeKind, NodePath, ProjectState, Workspace},
    runner::{Definition, Item, Summary},
    stream::{self, EventKind, EventReceiver},
};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::mpsc;

pub type Keys = Arc<HashMap<NodePath, String>>;
#[derive(Default)]
pub struct RunState {
    pub keys: Keys,
    pub pending: Option<JobTag>,
    pub running: Option<Running>,
    pub summary: Option<Summary>,
    pub client: stream::Executor,
    pub responses: VecDeque<Arc<Response>>,
}
pub struct Running {
    pub tag: JobTag,
    pub cancel: CancellationToken,
    pub events: EventReceiver,
    pub responses: mpsc::Receiver<Arc<Response>>,
    pub completed: usize,
    pub total: usize,
    pub current: Option<String>,
    pub received_bytes: u64,
}
pub struct Prepare {
    pub tag: JobTag,
    pub collection: Arc<serde_json::Value>,
    pub root: PathBuf,
    pub selection: NodePath,
    pub environment: Option<Environment>,
    pub environment_text: Option<EnvironmentText>,
    pub keys: Keys,
}
pub enum EnvironmentText {
    Buffer(kode_core::document::snapshot::TextSnapshot),
    Draft(Arc<str>),
}
pub struct Prepared {
    pub definition: Definition,
    pub keys: Keys,
}
impl Prepare {
    pub fn prepare(self, cancel: &CancellationToken) -> Result<Prepared> {
        let mut model = Workspace::from_state(ProjectState {
            collection: self.collection.clone(),
            environments: Arc::new(self.environment.iter().cloned().collect()),
            ..Default::default()
        })?;
        if model.node(&self.selection).is_none() {
            return Err(DomainError::MissingResource("REST run selection".into()));
        }
        let mut environment = self.environment;
        if let (Some(environment), Some(text)) = (&mut environment, self.environment_text) {
            let text = match text {
                EnvironmentText::Buffer(snapshot) => snapshot.text(),
                EnvironmentText::Draft(text) => text,
            };
            // Reuse the metadata-preserving codec; this capture is authoritative
            // even if a semantic parse of the open buffer is still pending.
            model.update_environment_text(&environment.id, &text);
            *environment = model
                .environment(&environment.id)
                .expect("imported environment")
                .clone();
        }
        let mut keys = (*self.keys).clone();
        let mut items = Vec::new();
        for node in model.tree() {
            if cancel.is_cancelled() {
                return Err(DomainError::Cancelled);
            }
            if node.kind != NodeKind::Request || !node.path.starts_with(&self.selection) {
                continue;
            }
            let id = match keys.get(&node.path) {
                Some(id) => id.clone(),
                None => {
                    let id = format!("rest:{}", RequestId::fresh()?.0);
                    keys.insert(node.path.clone(), id.clone());
                    id
                }
            };
            items.push(Item {
                id,
                path: node.path,
                name: node.name,
            });
        }
        Ok(Prepared {
            keys: Arc::new(keys),
            definition: Definition {
                collection: self.collection,
                root: self.root,
                environment,
                items,
                workspace: self.tag.workspace,
            },
        })
    }
}
impl RunState {
    pub fn tree_changed(&mut self, change: TreeChange) {
        if matches!(change, TreeChange::Replaced) {
            self.keys = Arc::default();
            self.pending = None;
            self.cancel();
            return;
        }
        self.keys = Arc::new(
            self.keys
                .iter()
                .filter_map(|(path, id)| {
                    let path = match &change {
                        TreeChange::Inserted(inserted) => path.after_insert(inserted),
                        TreeChange::Removed(removed) => path.after_remove(removed)?,
                        TreeChange::Moved { from, to } if path.starts_with(from) => {
                            let mut moved = to.clone();
                            moved.0.extend_from_slice(&path.0[from.0.len()..]);
                            moved
                        }
                        TreeChange::Moved { from, to } => path.after_remove(from)?.after_insert(to),
                        TreeChange::Replaced => unreachable!(),
                    };
                    Some((path, id.clone()))
                })
                .collect(),
        );
    }
    pub fn cancel(&self) {
        if let Some(running) = &self.running {
            running.cancel.cancel();
        }
    }
    pub fn retain_response(&mut self, response: Arc<Response>) {
        // Count captures conservatively, including multipart metadata and both
        // logical requests. Bound history by bytes as well as entry count.
        let weight = |response: &Response| {
            fn request_weight(request: &kode_rest::resolve::PreparedRequest) -> usize {
                use kode_rest::resolve::{BodyPlan, PartContent};
                let plan = match &request.request.body {
                    BodyPlan::Empty => 0,
                    BodyPlan::Bytes(bytes) => bytes.len(),
                    BodyPlan::File(path) => path.as_os_str().len(),
                    BodyPlan::Multipart { boundary, parts } => {
                        boundary.len()
                            + parts
                                .iter()
                                .map(|part| {
                                    part.name.len()
                                        + part.content_type.as_ref().map_or(0, String::len)
                                        + match &part.content {
                                            PartContent::Text(text) => text.len(),
                                            PartContent::File(path) => path.as_os_str().len(),
                                        }
                                })
                                .sum::<usize>()
                    }
                };
                request.body.len()
                    + plan
                    + request.request.url.len()
                    + request
                        .request
                        .headers
                        .iter()
                        .map(|h| h.name.len() + h.value.len())
                        .sum::<usize>()
            }
            response.body.len()
                + request_weight(&response.request)
                + request_weight(&response.normalized_request)
                + response
                    .headers
                    .iter()
                    .map(|h| h.name.len() + h.value.len())
                    .sum::<usize>()
                + response.cookies.iter().map(String::len).sum::<usize>()
                + response.final_url.len()
                + response.status_text.len()
        };
        self.responses
            .retain(|old| old.key.item_id != response.key.item_id);
        let bytes = weight(&response);
        const LIMIT: usize = 192 * 1024 * 1024;
        if bytes > LIMIT {
            return;
        }
        let mut retained = self.responses.iter().map(|r| weight(r)).sum::<usize>();
        while self.responses.len() >= 32 || retained.saturating_add(bytes) > LIMIT {
            let Some(old) = self.responses.pop_front() else {
                break;
            };
            retained = retained.saturating_sub(weight(&old));
        }
        self.responses.push_back(response);
    }
    /// Bounded per-frame publication; an infinite stream cannot monopolize input.
    pub fn pump(&mut self) -> bool {
        let Some(running) = &mut self.running else {
            return false;
        };
        let mut changed = false;
        for _ in 0..64 {
            let Ok(event) = running.events.try_recv() else {
                break;
            };
            if event.key.tag.workspace != running.tag.workspace {
                continue;
            }
            running.current = Some(event.key.item_id);
            if let EventKind::Progress { received_bytes, .. } = event.kind {
                running.received_bytes = received_bytes;
            }
            changed = true;
        }
        let mut responses = Vec::new();
        for _ in 0..4 {
            let Ok(response) = running.responses.try_recv() else {
                break;
            };
            running.completed += 1;
            responses.push(response);
        }
        changed |= !responses.is_empty();
        for response in responses {
            self.retain_response(response);
        }
        changed
    }
}
