use super::*;
use crate::transport::encode;
use tokio::sync::OwnedSemaphorePermit;
pub(super) struct Frame {
    pub bytes: Vec<u8>,
    pub ack: Option<oneshot::Sender<Result<()>>>,
    pub _budget: OwnedSemaphorePermit,
}
struct Pending {
    shared: Arc<Shared>,
    id: u64,
    _slot: Option<OwnedSemaphorePermit>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        if self
            .shared
            .pending
            .lock()
            .expect("LSP pending lock")
            .remove(&self.id)
            .is_some()
            && !self.shared.stop.is_cancelled()
        {
            self.shared.try_notification(
                json!({"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":self.id}}),
            );
        }
    }
}
impl Shared {
    fn frame(&self, value: Value, ack: Option<oneshot::Sender<Result<()>>>) -> Result<Frame> {
        let bytes = encode(&value)?;
        let budget = self
            .budget
            .clone()
            .try_acquire_many_owned(bytes.len().try_into().map_err(|_| DomainError::QueueFull)?)
            .map_err(|_| DomainError::QueueFull)?;
        Ok(Frame {
            bytes,
            ack,
            _budget: budget,
        })
    }
    pub(super) fn try_notification(&self, value: Value) {
        if let Ok(frame) = self.frame(value, None) {
            let _ = self.writer.try_send(frame);
        }
    }
    pub(super) async fn send(&self, value: Value) -> Result<()> {
        if self.stop.is_cancelled() {
            return Err(self.error("server transport is stopped"));
        }
        let (reply, receive) = oneshot::channel();
        let frame = self.frame(value, Some(reply))?;
        self.writer.try_send(frame).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => DomainError::QueueFull,
            mpsc::error::TrySendError::Closed(_) => self.error("server writer is closed"),
        })?;
        tokio::select! { biased;_=self.stop.cancelled()=>Err(self.error("server transport is stopped")),result=receive=>result.map_err(|_|self.error("server writer stopped before acknowledgement"))? }
    }
}
impl LspClient {
    pub(super) async fn request_inner(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: &CancellationToken,
        internal: bool,
    ) -> Result<Value> {
        let shared = &self.owner.shared;
        if cancel.is_cancelled() {
            return Err(DomainError::Cancelled);
        }
        if shared.stop.is_cancelled() || !internal && shared.state() != ClientState::Running {
            return Err(shared.error("server is not running"));
        }
        let slot = if internal {
            None
        } else {
            Some(
                shared
                    .slots
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| DomainError::QueueFull)?,
            )
        };
        let id = shared
            .next_id
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current
                    .checked_add(1)
                    .filter(|next| *next <= i64::MAX as u64)
            })
            .map_err(|_| protocol("JSON-RPC request IDs exhausted"))?;
        let (reply, receive) = oneshot::channel();
        shared
            .pending
            .lock()
            .expect("LSP pending lock")
            .insert(id, reply);
        let _pending = Pending {
            shared: shared.clone(),
            id,
            _slot: slot,
        };
        let request = async {
            shared
                .send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
                .await?;
            receive
                .await
                .map_err(|_| shared.error("server stopped before responding"))?
        };
        tokio::select! {
            biased;
            _=cancel.cancelled()=>Err(DomainError::Cancelled),
            result=tokio::time::timeout(timeout,request)=>result.map_err(|_|shared.error(format!("LSP {method} timed out after {} ms",timeout.as_millis())))?,
        }
    }
}
