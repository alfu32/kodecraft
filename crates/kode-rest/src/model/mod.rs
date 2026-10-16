//! Persisted collection data stays a JSON tree. Runtime responses, cookies and
//! execution handles deliberately have no fields in this model.
pub mod editing;
pub mod environment;
pub mod path;
pub mod workspace;
pub use environment::{Environment, EnvironmentValue};
pub use path::NodePath;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::sync::Arc;
pub use workspace::Workspace;
pub const COLLECTION_SCHEMA: &str =
    "https://schema.getpostman.com/json/collection/v2.1.0/collection.json";
pub type Extra = Map<String, Value>;
fn yes() -> bool {
    true
}
fn one() -> u64 {
    1
}
fn response_height() -> i32 {
    12
}
fn environment_height() -> i32 {
    8
}
fn collection() -> Arc<Value> {
    Arc::new(default_collection("REST API"))
}
pub fn default_collection(name: &str) -> Value {
    json!({"info":{"name":name,"schema":COLLECTION_SCHEMA},"item":[]})
}
pub fn new_folder(name: &str) -> Value {
    json!({"name":name,"item":[]})
}
pub fn new_request(name: &str) -> Value {
    json!({"id":uuid::Uuid::new_v4().to_string(),"name":name,"request":{"method":"GET","header":[],"url":{"raw":""}},"response":[]})
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UiState {
    pub selected_path: Option<String>,
    pub response_panel_height: i32,
    pub pretty_print_responses: bool,
    pub environment_panel_height: i32,
    #[serde(flatten)]
    pub extra: Extra,
}
impl Default for UiState {
    fn default() -> Self {
        Self {
            selected_path: None,
            response_panel_height: response_height(),
            pretty_print_responses: yes(),
            environment_panel_height: environment_height(),
            extra: Extra::new(),
        }
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProjectState {
    pub version: u64,
    pub collection: Arc<Value>,
    pub ui: UiState,
    pub environments: Arc<Vec<Environment>>,
    pub active_environment_id: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}
impl Default for ProjectState {
    fn default() -> Self {
        Self {
            version: one(),
            collection: collection(),
            ui: UiState::default(),
            environments: Arc::new(vec![]),
            active_environment_id: None,
            extra: Extra::new(),
        }
    }
}
impl std::fmt::Debug for ProjectState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestProjectState")
            .field("version", &self.version)
            .field("environments", &self.environments.len())
            .finish_non_exhaustive()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Collection,
    Folder,
    Request,
}
#[derive(Clone, PartialEq, Eq)]
pub struct TreeNode {
    pub path: NodePath,
    pub kind: NodeKind,
    pub name: String,
    pub method: Option<String>,
    pub depth: usize,
    pub has_children: bool,
}
