//! Package definitions are uncached, complete snapshots with connection/refresh proofs.
use super::*;
use crate::model::{DatabaseObjectType, MetadataSourceResult};
use kode_core::RequestId;

#[derive(Debug)]
pub struct Source {
    pub result: MetadataSourceResult,
    key: Key,
    generation: u64,
}
impl Metadata {
    pub fn source_is_current(
        &self,
        source: &str,
        request: &MetadataRequest,
        result: &Source,
    ) -> bool {
        !self.owner.shared.stop.is_cancelled()
            && self
                .owner
                .shared
                .key(source, request)
                .is_ok_and(|key| key == result.key)
            && self
                .owner
                .shared
                .cache
                .lock()
                .expect("metadata cache")
                .generation
                == result.generation
    }
    pub async fn source(
        &self,
        source: &str,
        request: MetadataRequest,
        cancel: &CancellationToken,
    ) -> Result<Arc<Source>> {
        if self.owner.shared.stop.is_cancelled() || cancel.is_cancelled() {
            return Err(error(
                Failure::Cancelled,
                "Package source request cancelled",
            ));
        }
        validate(&request)?;
        if request.kind != MetadataRequestKind::Object
            || request.object_type != Some(DatabaseObjectType::Package)
        {
            return Err(error(
                Failure::Unavailable,
                "Source retrieval requires a package object",
            ));
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
            shared
                .load_source(key, request, generation, job_id, token)
                .await
        });
        tokio::select! { biased;
            _ = cancel.cancelled() => { guard.0.cancel(); task.await },
            result = &mut task => result,
        }
        .map_err(|_| error(Failure::Transport, "Package source worker failed"))?
    }
}
impl Shared {
    async fn load_source(
        &self,
        key: Key,
        request: MetadataRequest,
        generation: u64,
        job_id: String,
        token: CancellationToken,
    ) -> Result<Arc<Source>> {
        let client = self.connections.runtime().get(&token).await?;
        if client.info().is_none_or(|info| {
            info.helper_epoch != key.epoch
                || !info
                    .capabilities
                    .iter()
                    .any(|method| method == "metadata.source")
                || !info
                    .capabilities
                    .iter()
                    .any(|method| method == "metadata.cancel")
        }) {
            return Err(error(
                Failure::Unavailable,
                "Current JDBC helper does not provide package source",
            ));
        }
        if self.key(&key.source, &request)? != key {
            return Err(stale());
        }
        let request_token = CancellationToken::new();
        let mut pending = Box::pin(client.request_with_timeout(
            "metadata.source",
            json!({"connectionId":key.connection,"metadataJobId":job_id,"request":request}),
            false,
            Duration::from_secs(60),
            &request_token,
        ));
        let value = tokio::select! {biased;
            _=token.cancelled()=>{
                let cancel_token=CancellationToken::new();
                let _=client.request_with_timeout(
                    "metadata.cancel",
                    json!({"metadataJobId":job_id}),
                    false,
                    Duration::from_secs(2),
                    &cancel_token,
                ).await;
                let _ = tokio::time::timeout(Duration::from_secs(18),&mut pending).await;
                return Err(error(Failure::Cancelled,"Package source request cancelled"));
            },
            result=&mut pending=>result?,
        };
        let bytes = serde_json::to_vec(&value)
            .map_err(|_| error(Failure::Protocol, "Cannot decode package source"))?
            .len();
        let result: MetadataSourceResult = serde_json::from_value(value)
            .map_err(|_| error(Failure::Protocol, "Invalid package source response"))?;
        if result.context_revision != key.context {
            return Err(stale());
        }
        if bytes > 3 * 1024 * 1024
            || result.documents.is_empty()
            || result.documents.len() > 10000
            || result.documents.iter().any(|doc| {
                doc.title.is_empty()
                    || doc.title.len() > 8192
                    || doc.text.trim().is_empty()
                    || doc.text.len() > 2 * 1024 * 1024
            })
        {
            return Err(error(
                Failure::Protocol,
                "Package source exceeds response bounds or is empty",
            ));
        }
        if token.is_cancelled()
            || self.key(&key.source, &request)? != key
            || self.cache.lock().expect("metadata cache").generation != generation
        {
            return Err(stale());
        }
        Ok(Arc::new(Source {
            result,
            key,
            generation,
        }))
    }
}
