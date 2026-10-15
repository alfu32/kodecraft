//! Independent feature consumers. Only progress is coalesced; query data and
//! final status never compete with a driver's progress consumer.
use super::*;
use std::sync::Weak;
use tokio::sync::{Notify, OwnedSemaphorePermit};
pub(super) struct Frame {
    pub event: Event,
    _budget: OwnedSemaphorePermit,
}
pub(super) struct Route {
    ticket: u64,
    queue: VecDeque<Frame>,
    ready: Arc<Notify>,
}
pub(super) struct Events {
    pub queue: VecDeque<Frame>,
    pub routes: BTreeMap<String, Route>,
}
pub struct EventSubscription {
    shared: Weak<Shared>,
    method: String,
    ticket: u64,
    ready: Arc<Notify>,
}
impl EventSubscription {
    pub async fn recv(&mut self) -> Option<Event> {
        let shared = self.shared.upgrade()?;
        loop {
            let notified = self.ready.notified();
            {
                let mut events = shared.events.lock().expect("bridge events");
                let route = events.routes.get_mut(&self.method)?;
                if route.ticket != self.ticket {
                    return None;
                }
                if let Some(frame) = route.queue.pop_front() {
                    return Some(frame.event);
                }
            }
            if shared.stop.is_cancelled() {
                return None;
            }
            tokio::select! {biased;_=notified=>{},_=shared.stop.cancelled()=>{}}
        }
    }
}
impl Drop for EventSubscription {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.upgrade() {
            let mut events = shared.events.lock().expect("bridge events");
            let removed = if events
                .routes
                .get(&self.method)
                .is_some_and(|route| route.ticket == self.ticket)
            {
                events.routes.remove(&self.method)
            } else {
                None
            };
            let incomplete = removed
                .as_ref()
                .is_some_and(|route| !route.queue.is_empty())
                && self.method != "driver.progress";
            drop(events);
            drop(removed);
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
        let ready = Arc::new(Notify::new());
        events.routes.insert(
            method.into(),
            Route {
                ticket,
                queue: VecDeque::new(),
                ready: ready.clone(),
            },
        );
        Ok(EventSubscription {
            shared: Arc::downgrade(shared),
            method: method.into(),
            ticket,
            ready,
        })
    }
}
impl Shared {
    pub(super) fn receive_event(&self, event: Event, size: usize) -> Result<()> {
        let mut events = self.events.lock().expect("bridge events");
        let progress = event.method == "driver.progress";
        let (queue, ready) = if let Some(route) = events.routes.get_mut(&event.method) {
            (&mut route.queue, Some(route.ready.clone()))
        } else {
            (&mut events.queue, None)
        };
        if progress {
            let same = queue.iter().position(|frame| {
                frame.event.method == event.method
                    && frame.event.params.get("resolutionId") == event.params.get("resolutionId")
            });
            if let Some(index) = same {
                queue.remove(index);
            }
            if queue.len() >= 32 {
                if let Some(index) = queue
                    .iter()
                    .position(|frame| frame.event.method == "driver.progress")
                {
                    queue.remove(index);
                } else {
                    return Ok(());
                }
            }
        } else if queue.len() >= 32 {
            return Err(BridgeError::new(
                Failure::Busy,
                "JDBC result consumer overflow; preview incomplete and helper disconnected",
            ));
        }
        let budget = match self
            .event_budget
            .clone()
            .try_acquire_many_owned(size as u32)
        {
            Ok(budget) => budget,
            Err(_) if progress => return Ok(()),
            Err(_) => {
                return Err(BridgeError::new(
                    Failure::Busy,
                    "JDBC notification byte capacity exhausted; preview incomplete",
                ));
            }
        };
        queue.push_back(Frame {
            event,
            _budget: budget,
        });
        if let Some(ready) = ready {
            ready.notify_one();
        }
        Ok(())
    }
}
