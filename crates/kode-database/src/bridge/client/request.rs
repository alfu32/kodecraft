use super::*;
use tokio::sync::OwnedSemaphorePermit;
pub(super) struct Frame {
    pub bytes: Vec<u8>,
    pub submitted: Arc<AtomicU8>,
    pub _budget: OwnedSemaphorePermit,
}
struct Pending {
    shared: Arc<Shared>,
    id: String,
    submitted: Arc<AtomicU8>,
    _slot: OwnedSemaphorePermit,
}
impl Drop for Pending {
    fn drop(&mut self) {
        let _ = self
            .submitted
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire);
        self.shared
            .pending
            .lock()
            .expect("bridge pending")
            .remove(&self.id);
    }
}
pub(super) fn control(method: &str) -> bool {
    matches!(
        method,
        "hello"
            | "ping"
            | "shutdown"
            | "query.cancel"
            | "driver.cancel"
            | "connection.close"
            | "metadata.cancel"
    )
}
impl Client {
    pub(super) async fn request_inner(
        &self,
        method: &str,
        params: Value,
        mutating: bool,
        internal: bool,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        let shared = &self.owner.shared;
        let mutating = mutating
            || matches!(
                method,
                "query.execute"
                    | "connection.commit"
                    | "connection.rollback"
                    | "connection.setContext"
                    | "connection.open"
                    | "connection.test"
                    | "connection.close"
            );
        if cancel.is_cancelled() {
            return Err(BridgeError::new(
                Failure::Cancelled,
                "JDBC request cancelled before submission",
            ));
        }
        if shared.stop.is_cancelled() || !internal && self.state() != State::Running {
            return Err(BridgeError::new(
                Failure::Transport,
                "JDBC helper is not running; reconnect explicitly",
            ));
        }
        let priority = control(method);
        let slot = if priority {
            &shared.control_slots
        } else {
            &shared.slots
        }
        .clone()
        .try_acquire_owned()
        .map_err(|_| BridgeError::new(Failure::Busy, "JDBC request capacity exhausted"))?;
        let id = shared
            .next_id
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| BridgeError::new(Failure::Protocol, "JDBC request IDs exhausted"))?
            .to_string();
        let bytes =
            protocol::payload(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
                .map_err(|_| {
                    BridgeError::new(Failure::Protocol, "Invalid or oversized JDBC request")
                })?;
        let budget = if priority {
            &shared.control_budget
        } else {
            &shared.budget
        }
        .clone()
        .try_acquire_many_owned(bytes.len() as u32)
        .map_err(|_| BridgeError::new(Failure::Busy, "JDBC queued byte capacity exhausted"))?;
        // 0 queued, 1 may have reached helper, 2 abandoned before write.
        let submitted = Arc::new(AtomicU8::new(0));
        let (reply, receive) = oneshot::channel();
        shared.pending.lock().expect("bridge pending").insert(
            id.clone(),
            Waiting {
                reply,
                mutating,
                submitted: submitted.clone(),
            },
        );
        let _pending = Pending {
            shared: shared.clone(),
            id,
            submitted: submitted.clone(),
            _slot: slot,
        };
        let frame = Frame {
            bytes,
            submitted: submitted.clone(),
            _budget: budget,
        };
        if priority {
            &shared.control
        } else {
            &shared.writer
        }
        .try_send(frame)
        .map_err(|_| BridgeError::new(Failure::Busy, "JDBC writer capacity exhausted"))?;
        let result = tokio::select! {biased;
            _=cancel.cancelled()=>Err(BridgeError::new(Failure::Cancelled,"JDBC request wait cancelled; use the operation's cancellation command to stop it")),
            result=tokio::time::timeout(timeout,receive)=>match result {Err(_)=>Err(BridgeError::new(Failure::Timeout,"JDBC request timed out; it was not replayed")),Ok(Err(_))=>Err(BridgeError::new(Failure::Transport,"JDBC helper disconnected")),Ok(Ok(result))=>result},
        };
        result.map_err(|mut error| {
            let may_have_started =
                submitted.compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire) == Err(1);
            error.unknown_outcome |=
                mutating && may_have_started && !matches!(error.kind, Failure::Remote);
            error
        })
    }
}
