//! Lazy JDBC metadata with bounded native caching. No constructor or cache read
//! starts Java. Cache identity includes connection epoch, context and revision.
mod source;
use crate::{
    bridge::client::{BridgeError, Failure},
    connections::{Connections, SessionInfo},
    model::{MetadataRequest, MetadataRequestKind, MetadataResult},
};
use kode_core::RequestId;
use kode_platform::jobs::CancellationToken;
use serde_json::json;
pub use source::Source;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Semaphore;
use tokio_util::task::TaskTracker;

const MAX_CACHE_BYTES: usize = 16 * 1024 * 1024;
const MAX_RESULT_BYTES: usize = 4 * 1024 * 1024;
type Result<T> = std::result::Result<T, BridgeError>;
fn error(kind: Failure, text: &'static str) -> BridgeError {
    BridgeError::new(kind, text)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    source: String,
    epoch: String,
    connection: String,
    context: u64,
    revision: u64,
    request: String,
}
struct Cached {
    result: Arc<MetadataResult>,
    bytes: usize,
}
#[derive(Default)]
struct Cache {
    entries: BTreeMap<Key, Cached>,
    bytes: usize,
    generation: u64,
}
impl Cache {
    fn remove(&mut self, predicate: impl Fn(&Key) -> bool) {
        self.entries.retain(|key, value| {
            if predicate(key) {
                self.bytes -= value.bytes;
                false
            } else {
                true
            }
        });
    }
    fn insert(&mut self, key: Key, result: Arc<MetadataResult>, bytes: usize) {
        self.remove(|entry| entry == &key);
        while !self.entries.is_empty()
            && (self.entries.len() >= 256 || self.bytes + bytes > MAX_CACHE_BYTES)
        {
            let (_, value) = self.entries.pop_first().expect("nonempty metadata cache");
            self.bytes -= value.bytes;
        }
        self.bytes += bytes;
        self.entries.insert(key, Cached { result, bytes });
    }
}
struct Shared {
    connections: Connections,
    cache: Mutex<Cache>,
    stop: CancellationToken,
}
struct Owner {
    shared: Arc<Shared>,
    tasks: TaskTracker,
    slots: Arc<Semaphore>,
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.shared.stop.cancel();
        self.tasks.close();
    }
}
#[derive(Clone)]
pub struct Metadata {
    owner: Arc<Owner>,
}
impl Metadata {
    pub fn new(connections: Connections) -> Self {
        Self {
            owner: Arc::new(Owner {
                shared: Arc::new(Shared {
                    connections,
                    cache: Mutex::new(Cache::default()),
                    stop: CancellationToken::new(),
                }),
                tasks: TaskTracker::new(),
                slots: Arc::new(Semaphore::new(16)),
            }),
        }
    }
    /// Cache lookup is synchronous and inert; unavailable/stale sessions miss.
    pub fn cached(&self, source: &str, request: &MetadataRequest) -> Option<Arc<MetadataResult>> {
        validate(request).ok()?;
        if self.owner.shared.stop.is_cancelled() {
            return None;
        }
        let key = self.owner.shared.key(source, request).ok()?;
        let mut cache = self.owner.shared.cache.lock().expect("metadata cache");
        cache.remove(|entry| {
            entry.source == key.source
                && (entry.epoch != key.epoch
                    || entry.connection != key.connection
                    || entry.context != key.context
                    || entry.revision != key.revision)
        });
        cache.entries.get(&key).map(|value| value.result.clone())
    }
    pub fn invalidate(&self, source: &str, request: &MetadataRequest) {
        let request = serde_json::to_string(request).expect("metadata request serializes");
        let mut cache = self.owner.shared.cache.lock().expect("metadata cache");
        cache.generation = cache.generation.saturating_add(1);
        cache.remove(|key| key.source == source && key.request == request);
    }
    /// The application checks this again before applying an async tree update.
    pub fn is_current(
        &self,
        source: &str,
        request: &MetadataRequest,
        result: &Arc<MetadataResult>,
    ) -> bool {
        self.cached(source, request)
            .is_some_and(|cached| Arc::ptr_eq(&cached, result))
    }
    pub fn refresh_all(&self, source: &str) {
        let mut cache = self.owner.shared.cache.lock().expect("metadata cache");
        cache.generation = cache.generation.saturating_add(1);
        cache.remove(|key| key.source == source);
    }
    pub async fn introspect(
        &self,
        source: &str,
        request: MetadataRequest,
        refresh: bool,
        cancel: &CancellationToken,
    ) -> Result<Arc<MetadataResult>> {
        if self.owner.shared.stop.is_cancelled() || cancel.is_cancelled() {
            return Err(error(Failure::Cancelled, "Metadata request cancelled"));
        }
        validate(&request)?;
        if refresh {
            self.invalidate(source, &request);
        } else if let Some(cached) = self.cached(source, &request) {
            return Ok(cached);
        }
        let key = self.owner.shared.key(source, &request)?;
        let generation = self
            .owner
            .shared
            .cache
            .lock()
            .expect("metadata cache")
            .generation;
        let slot = self
            .owner
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| error(Failure::Busy, "Metadata request capacity exhausted"))?;
        let token = self.owner.shared.stop.child_token();
        let job_id = RequestId::fresh()
            .map_err(|_| error(Failure::Protocol, "Metadata request IDs exhausted"))?
            .0
            .to_string();
        struct Cancel(CancellationToken);
        impl Drop for Cancel {
            fn drop(&mut self) {
                self.0.cancel();
            }
        }
        let guard = Cancel(token.clone());
        let shared = self.owner.shared.clone();
        let mut task = self.owner.tasks.spawn(async move {
            let _slot = slot;
            shared.load(key, request, generation, job_id, token).await
        });
        tokio::select! { biased;
            _ = cancel.cancelled() => { guard.0.cancel(); task.await },
            result = &mut task => result,
        }
        .map_err(|_| error(Failure::Transport, "Metadata worker failed"))?
    }
    pub async fn shutdown(&self) {
        self.owner.shared.stop.cancel();
        self.owner.tasks.close();
        self.owner.tasks.wait().await;
        let mut cache = self.owner.shared.cache.lock().expect("metadata cache");
        cache.entries.clear();
        cache.bytes = 0;
    }
}
impl Shared {
    fn key(&self, source: &str, request: &MetadataRequest) -> Result<Key> {
        let (session, revision) = self
            .connections
            .session_revision(source)
            .map_err(|e| e.bridge)?;
        Ok(key(source, request, &session, revision))
    }
    async fn load(
        &self,
        key: Key,
        request: MetadataRequest,
        generation: u64,
        job_id: String,
        token: CancellationToken,
    ) -> Result<Arc<MetadataResult>> {
        let client = self.connections.runtime().get(&token).await?;
        if client.info().is_none_or(|info| {
            info.helper_epoch != key.epoch
                || !info
                    .capabilities
                    .iter()
                    .any(|method| method == "metadata.list")
                || !info
                    .capabilities
                    .iter()
                    .any(|method| method == "metadata.cancel")
        }) {
            return Err(error(
                Failure::Unavailable,
                "Current JDBC helper does not provide cancellable metadata for this connection",
            ));
        }
        if self.key(&key.source, &request)? != key {
            return Err(stale());
        }
        let session = self
            .connections
            .session(&key.source)
            .map_err(|e| e.bridge)?;
        let request_token = CancellationToken::new();
        let mut pending = Box::pin(client.request_with_timeout(
            "metadata.list",
            json!({"connectionId":key.connection,"metadataJobId":job_id,"request":request}),
            false,
            Duration::from_secs(60),
            &request_token,
        ));
        let value = tokio::select! { biased;
            _=token.cancelled()=>{
                let cancel_token=CancellationToken::new();
                let _=client.request_with_timeout(
                    "metadata.cancel",
                    json!({"metadataJobId":job_id}),
                    false,
                    Duration::from_secs(2),
                    &cancel_token,
                ).await;
                let _=tokio::time::timeout(Duration::from_secs(18),&mut pending).await;
                return Err(error(Failure::Cancelled,"Metadata request cancelled"));
            },
            result=&mut pending=>match result {
                Ok(value)=>value,
                Err(failure)=>{
                    if failure.kind!=Failure::Cancelled { self.connections.execution_failed(&session,failure.clone()); }
                    return Err(failure);
                }
            }
        };
        if value
            .get("contextRevision")
            .and_then(serde_json::Value::as_u64)
            != Some(key.context)
        {
            return Err(stale());
        }
        let bytes = serde_json::to_vec(&value)
            .map_err(|_| error(Failure::Protocol, "Cannot decode JDBC metadata"))?
            .len();
        if bytes > MAX_RESULT_BYTES {
            return Err(error(
                Failure::Protocol,
                "JDBC metadata response exceeds bounds",
            ));
        }
        let result: MetadataResult = serde_json::from_value(value)
            .map_err(|_| error(Failure::Protocol, "Invalid JDBC metadata response"))?;
        validate_result(&result)?;
        let result = Arc::new(result);
        let mut cache = self.cache.lock().expect("metadata cache");
        if token.is_cancelled()
            || cache.generation != generation
            || self.key(&key.source, &request)? != key
        {
            return Err(stale());
        }
        cache.insert(key, result.clone(), bytes);
        Ok(result)
    }
}
fn key(source: &str, request: &MetadataRequest, session: &SessionInfo, revision: u64) -> Key {
    Key {
        source: source.to_owned(),
        epoch: session.helper_epoch.clone(),
        connection: session.connection_id.clone(),
        context: session.context_revision,
        revision,
        request: serde_json::to_string(request).expect("metadata request serializes"),
    }
}
fn stale() -> BridgeError {
    error(
        Failure::Cancelled,
        "Metadata context changed or was refreshed; expand again",
    )
}
fn validate(request: &MetadataRequest) -> Result<()> {
    if [&request.catalog, &request.schema, &request.object_name]
        .into_iter()
        .flatten()
        .any(|s| s.len() > 4096 || s.contains('\0'))
        || request
            .object_id
            .as_ref()
            .is_some_and(|s| s.len() > 4096 || s.contains('\0'))
        || request.kind == MetadataRequestKind::Category && request.category.is_none()
        || request.kind == MetadataRequestKind::Object
            && (request.object_name.as_ref().is_none_or(String::is_empty)
                || request.object_type.is_none())
    {
        return Err(error(Failure::Unavailable, "Invalid metadata request"));
    }
    Ok(())
}
fn validate_result(result: &MetadataResult) -> Result<()> {
    let mut ids = std::collections::BTreeSet::new();
    if result.objects.len() > 10000
        || result.message.as_ref().is_some_and(|s| s.len() > 8192)
        || result.objects.iter().any(|o| {
            o.id.is_empty()
                || !ids.insert(&o.id)
                || [&o.id, &o.name, &o.qualified_name]
                    .iter()
                    .any(|s| s.len() > 8192)
                || [&o.catalog, &o.schema]
                    .into_iter()
                    .flatten()
                    .any(|s| s.len() > 8192)
                || o.attributes.len() > 64
                || o.attributes
                    .iter()
                    .any(|(k, v)| k.len() > 128 || v.len() > 8192)
        })
    {
        return Err(error(
            Failure::Protocol,
            "Invalid JDBC metadata response bounds",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_metadata_fixture_keeps_qualified_names_attributes_and_unsupported_messages() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../protocol/jdbc-v1/metadata-fixtures.json"
        ))
        .unwrap();
        for value in fixture["results"].as_array().unwrap() {
            let result: MetadataResult = serde_json::from_value(value.clone()).unwrap();
            validate_result(&result).unwrap();
            if !result.objects.is_empty() {
                assert_eq!(result.objects[0].qualified_name, "catalog.é界😀");
                assert_eq!(result.objects[1].attributes["referencedTable"], "parent");
                assert!(!result.objects[1].has_children);
            } else {
                assert!(result.message.unwrap().contains("does not support"));
            }
        }
    }
    #[test]
    fn cache_evicts_at_byte_and_entry_bounds_without_invalidating_retained_reader_arcs() {
        let result = Arc::new(MetadataResult {
            objects: Vec::new(),
            capabilities: None,
            message: Some("fixture".into()),
        });
        let mut cache = Cache::default();
        for index in 0..300 {
            cache.insert(
                Key {
                    source: "source".into(),
                    epoch: "epoch".into(),
                    connection: "connection".into(),
                    context: 0,
                    revision: 0,
                    request: format!("{index:03}"),
                },
                result.clone(),
                1024 * 1024,
            );
        }
        assert_eq!(cache.entries.len(), 16);
        assert_eq!(cache.bytes, MAX_CACHE_BYTES);
        assert_eq!(result.message.as_deref(), Some("fixture"));
        cache.remove(|_| true);
        assert_eq!(cache.bytes, 0);
        assert_eq!(Arc::strong_count(&result), 1);
    }
}
