use super::*;
use indexmap::IndexMap;
use kode_core::{DomainError, Result};
use std::collections::{BTreeMap, VecDeque};

pub fn validate_collection(collection: &Value) -> Result<()> {
    let bad = |message: &str| DomainError::Parse {
        format: "Postman collection",
        message: message.into(),
    };
    if !collection.is_object() {
        return Err(bad("Collection must be a JSON object"));
    }
    if !collection.get("info").is_some_and(Value::is_object) {
        return Err(bad("Collection is missing info{}"));
    }
    if !collection.get("item").is_some_and(Value::is_array) {
        return Err(bad("Collection is missing item[]"));
    }
    if text(collection["info"].get("schema"))
        .is_some_and(|schema| !schema.contains("collection/v2.1.0/collection.json"))
    {
        return Err(bad("Unsupported collection schema; expected Postman v2.1"));
    }
    let mut pending = vec![(collection, 0)];
    let mut count = 0;
    while let Some((node, depth)) = pending.pop() {
        count += 1;
        if count > 100_000 || depth > 64 {
            return Err(bad("Collection tree exceeds 100,000 nodes or 64 levels"));
        }
        if let Some(children) = node.get("item").and_then(Value::as_array) {
            pending.extend(children.iter().map(|child| (child, depth + 1)));
        }
    }
    Ok(())
}
fn items(node: &Value) -> &[Value] {
    node.get("item")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}
fn metrics(node: &Value) -> (usize, usize) {
    let mut pending = vec![(node, 0)];
    let mut count = 0;
    let mut max_depth = 0;
    while let Some((node, depth)) = pending.pop() {
        count += 1;
        max_depth = max_depth.max(depth);
        pending.extend(items(node).iter().map(|child| (child, depth + 1)));
    }
    (count, max_depth)
}
fn node_at<'a>(root: &'a Value, path: &NodePath) -> Option<&'a Value> {
    let mut node = root;
    for &index in &path.0 {
        node = items(node).get(index)?;
    }
    node.is_object().then_some(node)
}
fn node_mut<'a>(root: &'a mut Value, path: &NodePath) -> Option<&'a mut Value> {
    let mut node = root;
    for &index in &path.0 {
        node = node.get_mut("item")?.as_array_mut()?.get_mut(index)?;
    }
    node.is_object().then_some(node)
}
fn folder(node: &Value) -> bool {
    kind(node) != NodeKind::Request && node.get("item").is_some_and(Value::is_array)
}
fn kind(node: &Value) -> NodeKind {
    if node
        .get("request")
        .is_some_and(|request| request.is_object() || request.is_string())
    {
        NodeKind::Request
    } else {
        NodeKind::Folder
    }
}
fn text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}
fn new_ids(node: &mut Value) {
    if !node.is_object() {
        return;
    }
    if kind(node) == NodeKind::Request {
        node["id"] = Value::String(uuid::Uuid::new_v4().to_string());
    }
    if let Some(children) = node.get_mut("item").and_then(Value::as_array_mut) {
        for child in children {
            new_ids(child);
        }
    }
}

/// One JSON collection plus project-local environment/UI metadata. Captures
/// share immutable collection/environment ownership; edits copy on demand.
#[derive(Default)]
pub struct Workspace {
    state: ProjectState,
    revision: u64,
}
impl Workspace {
    pub fn from_state(state: ProjectState) -> Result<Self> {
        let mut model = Self::default();
        model.restore(state)?;
        Ok(model)
    }
    pub fn restore(&mut self, mut state: ProjectState) -> Result<()> {
        validate_collection(&state.collection)?;
        let mut ids = std::collections::BTreeSet::new();
        if state
            .environments
            .iter()
            .any(|environment| environment.id.is_empty() || !ids.insert(&environment.id))
        {
            return Err(DomainError::Parse {
                format: "REST session",
                message: "Environment identities must be nonempty and unique".into(),
            });
        }
        if !state
            .active_environment_id
            .as_ref()
            .is_some_and(|id| ids.contains(id))
        {
            state.active_environment_id = None;
        }
        self.state = state;
        self.repair_selection();
        self.touch();
        Ok(())
    }
    fn touch(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn project_state(&self) -> ProjectState {
        self.state.clone()
    }
    pub fn collection(&self) -> &Value {
        &self.state.collection
    }
    pub fn export_collection(&self) -> Arc<Value> {
        self.state.collection.clone()
    }
    pub fn replace_collection(&mut self, collection: Value) -> Result<()> {
        validate_collection(&collection)?;
        self.state.collection = Arc::new(collection);
        self.state.ui.selected_path = None;
        self.touch();
        Ok(())
    }
    /// Append an import atomically. Existing paths and request identities stay
    /// stable; an invalid destination or tree leaves the workspace untouched.
    pub fn append_items(
        &mut self,
        parent: &NodePath,
        imported: Vec<Value>,
    ) -> Result<Vec<NodePath>> {
        let mut candidate = self.collection().clone();
        let target = node_mut(&mut candidate, parent)
            .filter(|node| folder(node))
            .ok_or_else(|| DomainError::Parse {
                format: "REST import destination",
                message: format!("{} must identify a collection or folder", parent.encode()),
            })?;
        let children = target
            .get_mut("item")
            .and_then(Value::as_array_mut)
            .expect("folder checked");
        let start = children.len();
        let paths = (start..start + imported.len())
            .map(|index| parent.child(index))
            .collect();
        children.extend(imported);
        validate_collection(&candidate)?;
        self.state.collection = Arc::new(candidate);
        self.touch();
        Ok(paths)
    }
    pub fn ui(&self) -> &UiState {
        &self.state.ui
    }
    pub fn collection_name(&self) -> String {
        text(
            self.collection()
                .get("info")
                .and_then(|info| info.get("name")),
        )
        .unwrap_or_else(|| "REST API".into())
    }
    pub fn node(&self, path: &NodePath) -> Option<&Value> {
        node_at(self.collection(), path)
    }
    pub fn selected_path(&self) -> NodePath {
        NodePath::decode(self.state.ui.selected_path.as_deref())
            .ok()
            .filter(|path| self.node(path).is_some())
            .unwrap_or_default()
    }
    fn selection(&mut self, path: NodePath) {
        self.state.ui.selected_path = (!path.is_root()).then(|| path.encode());
    }
    fn repair_selection(&mut self) {
        let path = self.selected_path();
        self.selection(path);
    }
    pub fn select(&mut self, path: &NodePath) -> bool {
        if self.node(path).is_none() {
            return false;
        }
        self.selection(path.clone());
        self.touch();
        true
    }
    pub fn set_response_panel_height(&mut self, height: i32) {
        self.state.ui.response_panel_height = height.clamp(5, 60);
        self.touch();
    }
    pub fn set_environment_panel_height(&mut self, height: i32) {
        self.state.ui.environment_panel_height = height.clamp(5, 60);
        self.touch();
    }
    pub fn set_pretty_print_responses(&mut self, value: bool) {
        self.state.ui.pretty_print_responses = value;
        self.touch();
    }
    pub fn replace_node(&mut self, path: &NodePath, replacement: Value) -> Result<bool> {
        if self.node(path).is_none() {
            return Ok(false);
        }
        if !replacement.is_object() {
            return Err(DomainError::Unsupported(
                "REST tree nodes must be objects".into(),
            ));
        }
        // Validate a candidate before replacing the authority. Arbitrary unknown
        // fields are retained by callers' scoped edits, never reconstructed DTOs.
        let mut candidate = (*self.state.collection).clone();
        *node_mut(&mut candidate, path).expect("validated path") = replacement;
        validate_collection(&candidate)?;
        self.state.collection = Arc::new(candidate);
        self.repair_selection();
        self.touch();
        Ok(true)
    }
    pub fn set_node_field(
        &mut self,
        path: &NodePath,
        key: &str,
        value: Option<Value>,
    ) -> Result<bool> {
        let Some(node) = self.node(path) else {
            return Ok(false);
        };
        let mut node = node.clone();
        let object = node.as_object_mut().expect("node object");
        if let Some(value) = value {
            object.insert(key.into(), value);
        } else {
            object.remove(key);
        }
        self.replace_node(path, node)
    }
    pub fn rename(&mut self, path: &NodePath, name: &str) -> Result<bool> {
        if path.is_root() {
            let mut info = self.collection()["info"].clone();
            info["name"] = Value::String(name.into());
            self.set_node_field(path, "info", Some(info))
        } else {
            self.set_node_field(path, "name", Some(Value::String(name.into())))
        }
    }
    pub fn tree(&self) -> Vec<TreeNode> {
        let mut result = vec![TreeNode {
            path: NodePath::default(),
            kind: NodeKind::Collection,
            name: self.collection_name(),
            method: None,
            depth: 0,
            has_children: !items(self.collection()).is_empty(),
        }];
        let mut pending: Vec<_> = items(self.collection())
            .iter()
            .enumerate()
            .rev()
            .map(|(index, node)| (NodePath(vec![index]), node, 1))
            .collect();
        while let Some((path, node, depth)) = pending.pop() {
            if !node.is_object() {
                continue;
            }
            let kind = kind(node);
            result.push(TreeNode {
                path: path.clone(),
                kind,
                name: text(node.get("name")).unwrap_or_else(|| {
                    if kind == NodeKind::Request {
                        "Unnamed Request"
                    } else {
                        "Unnamed Folder"
                    }
                    .into()
                }),
                method: text(
                    node.get("request")
                        .and_then(|request| request.get("method")),
                ),
                depth,
                has_children: kind == NodeKind::Folder && !items(node).is_empty(),
            });
            if kind == NodeKind::Folder {
                pending.extend(
                    items(node)
                        .iter()
                        .enumerate()
                        .rev()
                        .map(|(index, node)| (path.child(index), node, depth + 1)),
                );
            }
        }
        result
    }
    fn insert(&mut self, parent: &NodePath, index: usize, item: Value) -> Option<NodePath> {
        let (count, depth) = metrics(&item);
        if parent.0.len() + 1 + depth > 64
            || metrics(self.collection()).0.saturating_add(count) > 100_000
        {
            return None;
        }
        let node = self.node(parent)?;
        if !parent.is_root() && !folder(node) {
            return None;
        }
        let index = index.min(items(node).len());
        let inserted = parent.child(index);
        let selected = self.selected_path().after_insert(&inserted);
        node_mut(Arc::make_mut(&mut self.state.collection), parent)?
            .get_mut("item")?
            .as_array_mut()?
            .insert(index, item);
        self.selection(selected);
        self.touch();
        Some(inserted)
    }
    pub fn add_folder(&mut self, parent: &NodePath, name: &str) -> Option<NodePath> {
        let count = items(self.node(parent)?).len();
        self.insert(parent, count, new_folder(name))
    }
    pub fn add_request(&mut self, parent: &NodePath, name: &str) -> Option<NodePath> {
        let count = items(self.node(parent)?).len();
        self.insert(parent, count, new_request(name))
    }
    pub fn delete(&mut self, path: &NodePath) -> bool {
        if path.is_root() || self.node(path).is_none() {
            return false;
        }
        let parent = path.parent();
        let selected = self
            .selected_path()
            .after_remove(path)
            .unwrap_or_else(|| parent.clone());
        node_mut(Arc::make_mut(&mut self.state.collection),&parent).expect("validated parent")["item"].as_array_mut().expect("item array").remove(*path.0.last().expect("nonroot"));
        self.selection(selected);
        self.touch();
        true
    }
    pub fn duplicate(&mut self, path: &NodePath) -> Option<NodePath> {
        if path.is_root() {
            return None;
        }
        let mut copy = self.node(path)?.clone();
        new_ids(&mut copy);
        self.insert(&path.parent(), path.0.last()? + 1, copy)
    }
    pub fn move_node(
        &mut self,
        path: &NodePath,
        target_parent: &NodePath,
        target_index: Option<usize>,
    ) -> Option<NodePath> {
        if path.is_root() || target_parent.starts_with(path) {
            return None;
        }
        self.node(path)?;
        if target_parent.0.len() + 1 + metrics(self.node(path)?).1 > 64 {
            return None;
        }
        let target = self.node(target_parent)?;
        if !target_parent.is_root() && !folder(target) {
            return None;
        }
        let mut index = target_index.unwrap_or(items(target).len());
        let source_parent = path.parent();
        let source_index = *path.0.last()?;
        let parent = target_parent.after_remove(path)?;
        if source_parent == *target_parent && source_index < index {
            index = index.saturating_sub(1);
        }
        let selected = self.selected_path();
        let moved_selection = selected
            .starts_with(path)
            .then(|| selected.0[path.0.len()..].to_vec());
        let node = node_mut(Arc::make_mut(&mut self.state.collection), &source_parent)
            .expect("source parent")["item"]
            .as_array_mut()
            .expect("item array")
            .remove(source_index);
        let destination = node_mut(Arc::make_mut(&mut self.state.collection), &parent)
            .expect("rebased destination")["item"]
            .as_array_mut()
            .expect("item array");
        index = index.min(destination.len());
        destination.insert(index, node);
        let inserted = parent.child(index);
        let selected = if let Some(tail) = moved_selection {
            let mut result = inserted.clone();
            result.0.extend(tail);
            result
        } else {
            selected
                .after_remove(path)
                .expect("unmoved selection")
                .after_insert(&inserted)
        };
        self.selection(selected);
        self.touch();
        Some(inserted)
    }
    pub fn move_up(&mut self, path: &NodePath) -> Option<NodePath> {
        let index = *path.0.last()?;
        if index == 0 {
            return None;
        }
        self.move_node(path, &path.parent(), Some(index - 1))
    }
    pub fn move_down(&mut self, path: &NodePath) -> Option<NodePath> {
        self.node(path)?;
        let index = *path.0.last()?;
        let parent = path.parent();
        if index + 1 >= items(self.node(&parent)?).len() {
            return None;
        }
        self.move_node(path, &parent, Some(index + 2))
    }
    pub fn environments(&self) -> &[Environment] {
        &self.state.environments
    }
    pub fn environment(&self, id: &str) -> Option<&Environment> {
        self.state
            .environments
            .iter()
            .find(|environment| environment.id == id)
    }
    pub fn active_environment(&self) -> Option<&Environment> {
        self.state
            .active_environment_id
            .as_deref()
            .and_then(|id| self.environment(id))
    }
    pub fn active_environment_variables(&self) -> IndexMap<String, String> {
        self.active_environment()
            .into_iter()
            .flat_map(|environment| &environment.values)
            .filter(|value| value.enabled)
            .map(|value| (value.key.clone(), value.text()))
            .collect()
    }
    pub fn create_environment(&mut self, name: &str) -> String {
        let environment = Environment {
            name: if name.trim().is_empty() {
                "New environment".into()
            } else {
                name.into()
            },
            ..Default::default()
        };
        let id = environment.id.clone();
        Arc::make_mut(&mut self.state.environments).push(environment);
        if self.state.active_environment_id.is_none() {
            self.state.active_environment_id = Some(id.clone());
        }
        self.touch();
        id
    }
    pub fn rename_environment(&mut self, id: &str, name: &str) -> bool {
        if name.trim().is_empty() {
            return false;
        }
        let Some(index) = self
            .state
            .environments
            .iter()
            .position(|environment| environment.id == id)
        else {
            return false;
        };
        Arc::make_mut(&mut self.state.environments)[index].name = name.into();
        self.touch();
        true
    }
    pub fn duplicate_environment(&mut self, id: &str, name: Option<&str>) -> Option<String> {
        let mut copy = self.environment(id)?.clone();
        copy.id = uuid::Uuid::new_v4().to_string();
        copy.name = name.map_or_else(|| format!("{} copy", copy.name), str::to_string);
        let id = copy.id.clone();
        Arc::make_mut(&mut self.state.environments).push(copy);
        self.touch();
        Some(id)
    }
    pub fn activate_environment(&mut self, id: Option<&str>) -> bool {
        if id.is_some_and(|id| self.environment(id).is_none()) {
            return false;
        }
        self.state.active_environment_id = id.map(str::to_string);
        self.touch();
        true
    }
    pub fn remove_environment(&mut self, id: &str) -> bool {
        let Some(index) = self
            .state
            .environments
            .iter()
            .position(|environment| environment.id == id)
        else {
            return false;
        };
        Arc::make_mut(&mut self.state.environments).remove(index);
        if self.state.active_environment_id.as_deref() == Some(id) {
            self.state.active_environment_id = self
                .state
                .environments
                .first()
                .map(|environment| environment.id.clone());
        }
        self.touch();
        true
    }
    pub fn set_environment_values(&mut self, id: &str, values: Vec<EnvironmentValue>) -> bool {
        let Some(index) = self
            .state
            .environments
            .iter()
            .position(|environment| environment.id == id)
        else {
            return false;
        };
        if self.state.environments[index].values == values {
            return false;
        }
        Arc::make_mut(&mut self.state.environments)[index].values = values;
        self.touch();
        true
    }
    pub fn update_environment_text(&mut self, id: &str, text: &str) -> bool {
        let Some(environment) = self.environment(id) else {
            return false;
        };
        let mut existing: BTreeMap<&str, VecDeque<&EnvironmentValue>> = BTreeMap::new();
        for value in &environment.values {
            existing.entry(&value.key).or_default().push_back(value);
        }
        let values = environment::parse(text)
            .into_iter()
            .map(|parsed| {
                if let Some(previous) = existing
                    .get_mut(parsed.key.as_str())
                    .and_then(VecDeque::pop_front)
                {
                    EnvironmentValue {
                        value: if parsed.text() == previous.text() {
                            previous.value.clone()
                        } else {
                            parsed.value
                        },
                        ..previous.clone()
                    }
                } else {
                    parsed
                }
            })
            .collect();
        self.set_environment_values(id, values)
    }
}
