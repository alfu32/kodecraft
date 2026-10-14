use kode_core::{DocumentId, JobTag, RequestId, Revision, WorkspaceGeneration};
use std::collections::HashMap;

/// Main-thread result gate. Each logical operation gets a current request tag.
#[derive(Debug)]
pub struct ResultGate {
    workspace: WorkspaceGeneration,
    requests: HashMap<String, JobTag>,
    documents: HashMap<DocumentId, Revision>,
}
impl ResultGate {
    pub fn new(workspace: WorkspaceGeneration) -> Self {
        Self {
            workspace,
            requests: HashMap::new(),
            documents: HashMap::new(),
        }
    }
    pub fn request(
        &mut self,
        operation: String,
        document: Option<(DocumentId, Revision)>,
    ) -> JobTag {
        let tag = JobTag {
            workspace: self.workspace,
            request: RequestId::fresh().expect("request identity exhausted"),
            document,
        };
        self.requests.insert(operation, tag);
        if let Some((id, revision)) = document {
            self.documents
                .entry(id)
                .and_modify(|current| *current = (*current).max(revision))
                .or_insert(revision);
        }
        tag
    }
    pub fn document_changed(&mut self, id: DocumentId, revision: Revision) {
        self.documents.insert(id, revision);
    }
    pub fn document_closed(&mut self, id: DocumentId) {
        self.documents.remove(&id);
        self.requests
            .retain(|_, tag| tag.document.is_none_or(|(document, _)| document != id));
    }
    pub fn accepts(&self, operation: &str, tag: JobTag) -> bool {
        tag.workspace == self.workspace
            && self
                .requests
                .get(operation)
                .is_some_and(|current| current.is_current(tag))
            && tag
                .document
                .is_none_or(|(id, revision)| self.documents.get(&id) == Some(&revision))
    }
    pub fn dismiss(&mut self, operation: &str) {
        self.requests.remove(operation);
    }
    pub fn switch_workspace(&mut self) -> WorkspaceGeneration {
        self.workspace.0 = self
            .workspace
            .0
            .checked_add(1)
            .expect("workspace identity exhausted");
        self.requests.clear();
        self.documents.clear();
        self.workspace
    }
}
