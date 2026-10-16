//! Inert SQL console identities and bounded execution history. Editor documents
//! remain the composition layer's authority; only explicit snapshots persist.
use crate::{
    execution::{Phase, Snapshot},
    model::{DataSourceDefinition, DatabaseProjectState, DatabaseSessionState},
};
use kode_core::{DomainError, RequestId, Result};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

pub struct HistoryEntry {
    pub started_at_ms: u64,
    pub sql: String,
    pub duration_ms: u64,
    pub success: bool,
    pub unknown_outcome: bool,
    sequence: u64,
}
impl std::fmt::Debug for HistoryEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistoryEntry")
            .field("duration_ms", &self.duration_ms)
            .field("success", &self.success)
            .field("unknown_outcome", &self.unknown_outcome)
            .finish_non_exhaustive()
    }
}
pub struct Console {
    pub state: Arc<DatabaseSessionState>,
    pub history: VecDeque<HistoryEntry>,
    pub result: Option<Snapshot>,
    pub request: Option<RequestId>,
}
impl std::fmt::Debug for Console {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Console")
            .field("id", &self.state.id)
            .field("running", &self.request.is_some())
            .field("history_entries", &self.history.len())
            .finish_non_exhaustive()
    }
}
#[derive(Default)]
pub struct Manager {
    consoles: BTreeMap<String, Console>,
    order: Vec<String>,
    counters: BTreeMap<String, u64>,
    base: Arc<DatabaseProjectState>,
    history_bytes: usize,
    history_sequence: u64,
    history_order: BTreeMap<u64, String>,
}
impl Manager {
    pub fn create(&mut self, source: &DataSourceDefinition) -> Result<&Console> {
        if self.consoles.len() >= 4096 {
            return Err(DomainError::Unsupported(
                "SQL console limit reached; close an existing session".into(),
            ));
        }
        let next = self.counters.entry(source.id.clone()).or_default();
        let id = loop {
            *next = next
                .checked_add(1)
                .ok_or_else(|| DomainError::Unsupported("SQL session counter exhausted".into()))?;
            let id = format!("{}:{}", source.id, next);
            if !self.consoles.contains_key(&id) {
                break id;
            }
        };
        let safe_name: String = source
            .name
            .chars()
            .map(|ch| if ch.is_control() { ' ' } else { ch })
            .collect();
        let state = DatabaseSessionState {
            id: id.clone(),
            data_source_id: source.id.clone(),
            title: format!("sql@{}:{}", source.name, next),
            catalog: None,
            schema: None,
            auto_commit: source.auto_commit,
            buffer: format!("-- {safe_name}\n"),
            extra: Default::default(),
        };
        self.order.push(id.clone());
        self.consoles.insert(
            id.clone(),
            Console {
                state: Arc::new(state),
                history: VecDeque::new(),
                result: None,
                request: None,
            },
        );
        Ok(&self.consoles[&id])
    }
    pub fn get(&self, id: &str) -> Option<&Console> {
        self.consoles.get(id)
    }
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Console> {
        self.consoles.get_mut(id)
    }
    pub fn all(&self) -> impl Iterator<Item = &Console> {
        self.order.iter().filter_map(|id| self.consoles.get(id))
    }
    /// Restores transaction preferences, never live connections or results.
    /// Missing datasource sessions are returned to the caller for recovery.
    pub fn restore(
        &mut self,
        mut state: DatabaseProjectState,
        valid: Option<&BTreeSet<String>>,
    ) -> Result<Vec<DatabaseSessionState>> {
        if state.sessions.len() > 4096 {
            return Err(DomainError::Unsupported(
                "Too many saved SQL sessions".into(),
            ));
        }
        let mut restored = Self::default();
        let mut rejected = vec![];
        for session in std::mem::take(&mut state.sessions) {
            if valid.is_some_and(|ids| !ids.contains(&session.data_source_id)) {
                rejected.push(session);
                continue;
            }
            if session.id.is_empty()
                || session.id.len() > 1024
                || session.title.len() > 8192
                || restored.consoles.contains_key(&session.id)
            {
                return Err(DomainError::Unsupported(
                    "Invalid or duplicate saved SQL session identity".into(),
                ));
            }
            let counter = session
                .id
                .rsplit(':')
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or_default();
            let current = restored
                .counters
                .entry(session.data_source_id.clone())
                .or_default();
            *current = (*current).max(counter);
            restored.order.push(session.id.clone());
            restored.consoles.insert(
                session.id.clone(),
                Console {
                    state: Arc::new(session),
                    history: VecDeque::new(),
                    result: None,
                    request: None,
                },
            );
        }
        restored.base = Arc::new(state);
        *self = restored;
        Ok(rejected)
    }
    pub fn project_state(&self) -> DatabaseProjectState {
        self.snapshot().into_state(std::iter::empty())
    }
    /// Cheap application-thread capture; stringify buffers on the save worker.
    pub fn snapshot(&self) -> PersistenceSnapshot {
        PersistenceSnapshot {
            base: self.base.clone(),
            sessions: self.all().map(|console| console.state.clone()).collect(),
        }
    }
    pub fn set_data_sources(&mut self, sources: Vec<DataSourceDefinition>) {
        Arc::make_mut(&mut self.base).data_sources = sources;
    }
    pub fn set_context(
        &mut self,
        id: &str,
        catalog: Option<String>,
        schema: Option<String>,
    ) -> bool {
        if let Some(console) = self.consoles.get_mut(id) {
            let state = Arc::make_mut(&mut console.state);
            state.catalog = catalog;
            state.schema = schema;
            true
        } else {
            false
        }
    }
    pub fn set_auto_commit(&mut self, source: &str, value: bool) -> bool {
        let mut changed = false;
        for console in self.consoles.values_mut().filter(|console| {
            console.state.data_source_id == source && console.state.auto_commit != value
        }) {
            Arc::make_mut(&mut console.state).auto_commit = value;
            changed = true;
        }
        changed
    }
    pub fn update_buffer(&mut self, id: &str, buffer: String) -> bool {
        if let Some(console) = self.consoles.get_mut(id) {
            Arc::make_mut(&mut console.state).buffer = buffer;
            true
        } else {
            false
        }
    }
    pub fn begin(&mut self, id: &str, request: RequestId) -> Result<()> {
        let console = self
            .consoles
            .get_mut(id)
            .ok_or_else(|| DomainError::Unsupported("SQL session no longer exists".into()))?;
        if console.request.is_some() {
            return Err(DomainError::Unsupported(
                "This SQL session is already running; wait or cancel it".into(),
            ));
        }
        console.request = Some(request);
        console.result = None;
        Ok(())
    }
    pub fn progress(&mut self, id: &str, request: RequestId, result: Snapshot) -> bool {
        let Some(console) = self
            .consoles
            .get_mut(id)
            .filter(|console| console.request == Some(request))
        else {
            return false;
        };
        console.result = Some(result);
        true
    }
    pub fn finish(
        &mut self,
        id: &str,
        request: RequestId,
        sql: String,
        started_at_ms: u64,
        result: Snapshot,
    ) -> bool {
        let Some(console) = self
            .consoles
            .get_mut(id)
            .filter(|console| console.request == Some(request))
        else {
            return false;
        };
        if result.phase != Phase::Finished {
            return false;
        }
        console.request = None;
        self.history_sequence = self.history_sequence.saturating_add(1);
        let history =
            HistoryEntry {
                started_at_ms,
                sql,
                duration_ms: result
                    .completion
                    .as_ref()
                    .map_or(0, |result| result.elapsed_ms),
                success: result.completion.as_ref().is_some_and(|result| {
                    result.outcome == crate::bridge::wire::Outcome::Completed
                }) && result.error.is_none(),
                unknown_outcome: result.unknown_outcome,
                sequence: self.history_sequence,
            };
        self.history_bytes = self.history_bytes.saturating_add(history.sql.len());
        self.history_order.insert(history.sequence, id.to_string());
        console.history.push_back(history);
        console.result = Some(result);
        while console.history.len() > 512 {
            if let Some(removed) = console.history.pop_front() {
                self.history_bytes = self.history_bytes.saturating_sub(removed.sql.len());
                self.history_order.remove(&removed.sequence);
            }
        }
        while self.history_bytes > 16 * 1024 * 1024 || self.history_order.len() > 4096 {
            let Some((_, id)) = self.history_order.pop_first() else {
                break;
            };
            if let Some(removed) = self
                .consoles
                .get_mut(&id)
                .and_then(|console| console.history.pop_front())
            {
                self.history_bytes = self.history_bytes.saturating_sub(removed.sql.len());
            }
        }
        true
    }
    pub fn remove(&mut self, id: &str) -> Result<Option<Console>> {
        if self
            .consoles
            .get(id)
            .is_some_and(|console| console.request.is_some())
        {
            return Err(DomainError::Unsupported(
                "Cancel the running SQL operation before removing its session".into(),
            ));
        }
        let removed = self.consoles.remove(id);
        self.order.retain(|entry| entry != id);
        if let Some(console) = &removed {
            for entry in &console.history {
                self.history_order.remove(&entry.sequence);
            }
            self.history_bytes = self
                .history_bytes
                .saturating_sub(console.history.iter().map(|entry| entry.sql.len()).sum());
        }
        Ok(removed)
    }
}

pub struct PersistenceSnapshot {
    base: Arc<DatabaseProjectState>,
    sessions: Vec<Arc<DatabaseSessionState>>,
}
impl PersistenceSnapshot {
    /// Worker-only materialization combines captured metadata with document
    /// buffers from the exact same application revision.
    pub fn into_state(
        self,
        buffers: impl IntoIterator<Item = (String, String)>,
    ) -> DatabaseProjectState {
        let mut buffers: BTreeMap<_, _> = buffers.into_iter().collect();
        let mut state = (*self.base).clone();
        state.sessions = self
            .sessions
            .into_iter()
            .map(|session| DatabaseSessionState {
                id: session.id.clone(),
                data_source_id: session.data_source_id.clone(),
                title: session.title.clone(),
                catalog: session.catalog.clone(),
                schema: session.schema.clone(),
                auto_commit: session.auto_commit,
                buffer: buffers
                    .remove(&session.id)
                    .unwrap_or_else(|| session.buffer.clone()),
                extra: session.extra.clone(),
            })
            .collect();
        state
    }
}
