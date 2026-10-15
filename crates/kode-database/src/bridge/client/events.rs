//! One independently drained consumer per feature notification method. Query
//! data is never stolen by a concurrent driver's progress consumer.
use super::*;
use std::sync::Weak;
use tokio::sync::OwnedSemaphorePermit;
pub(super) struct Frame {
    pub event: Event,
    _budget: OwnedSemaphorePermit,
}
pub(super) struct Route {
    ticket: u64,
    sender: mpsc::Sender<Frame>,
}
pub(super) struct Events {
    pub queue: VecDeque<Frame>,
    pub routes: BTreeMap<String, Route>,
}
pub struct EventSubscription {
    shared: Weak<Shared>,
    method: String,
    ticket: u64,
    receiver: mpsc::Receiver<Frame>,
}
impl EventSubscription {
    /// Drain already received data before reporting a stopped epoch. Callers own
    /// bounded result storage after receiving; this permit bounds only transport.
    pub async fn recv(&mut self) -> Option<Event> {
        if let Ok(frame) = self.receiver.try_recv() {
            return Some(frame.event);
        }
        let shared = self.shared.upgrade()?;
        tokio::select! {biased;frame=self.receiver.recv()=>frame.map(|frame|frame.event),_=shared.stop.cancelled()=>None}
    }
}
impl Drop for EventSubscription {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.upgrade() {
            let mut events = shared.events.lock().expect("bridge events");
            if events
                .routes
                .get(&self.method)
                .is_some_and(|route| route.ticket == self.ticket)
            {
                events.routes.remove(&self.method);
            }
            let incomplete = !self.receiver.is_empty() && self.method != "driver.progress";
            drop(events);
            if incomplete {
                shared.fail(BridgeError::new(
                    Failure::Busy,
                    "JDBC event consumer closed with pending data; preview incomplete",
                ));
            }
        }
    }
}
impl Client {
    pub fn subscribe_events(&self, method: &str) -> Result<EventSubscription> {
        let shared = &self.owner.shared;
        protocol::validate(&json!({"jsonrpc":"2.0","method":method,"params":{}})).map_err(
            |_| BridgeError::new(Failure::Protocol, "Invalid JDBC notification subscription"),
        )?;
        if self.state() != State::Running {
            return Err(BridgeError::new(
                Failure::Transport,
                "JDBC helper is not running",
            ));
        }
        let mut events = shared.events.lock().expect("bridge events");
        if events.routes.len() >= 16 || events.routes.contains_key(method) {
            return Err(BridgeError::new(
                Failure::Busy,
                "JDBC notification method already has a consumer or subscription capacity is full",
            ));
        }
        let ticket = shared
            .next_id
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| BridgeError::new(Failure::Protocol, "JDBC IDs exhausted"))?;
        let (sender, receiver) = mpsc::channel(32);
        events
            .routes
            .insert(method.into(), Route { ticket, sender });
        Ok(EventSubscription {
            shared: Arc::downgrade(shared),
            method: method.into(),
            ticket,
            receiver,
        })
    }
}
impl Shared {
    pub(super) fn receive_event(&self, event: Event, size: usize) -> Result<()> {
        let budget = self
            .event_budget
            .clone()
            .try_acquire_many_owned(size as u32)
            .map_err(|_| {
                BridgeError::new(
                    Failure::Busy,
                    "JDBC notification byte capacity exhausted; preview incomplete",
                )
            })?;
        let frame = Frame {
            event,
            _budget: budget,
        };
        let mut events = self.events.lock().expect("bridge events");
        if let Some(route) = events.routes.get(&frame.event.method) {
            route.sender.try_send(frame).map_err(|_| {
                BridgeError::new(
                    Failure::Busy,
                    "JDBC event consumer overflow; preview incomplete and helper disconnected",
                )
            })?;
        } else {
            if events.queue.len() >= 32 {
                return Err(BridgeError::new(
                    Failure::Busy,
                    "JDBC result queue overflow; preview incomplete and helper disconnected",
                ));
            }
            events.queue.push_back(frame);
        }
        Ok(())
    }
}
