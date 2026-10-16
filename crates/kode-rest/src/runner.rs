//! Sequential execution over one immutable selection. UI selection/history and
//! final workflow qualification are separate from this native execution boundary.
use crate::{
    http::{ExecutionKey, Response},
    model::{Environment, NodePath},
    resolve::RequestResolver,
    stream::{self, EventSender},
};
use kode_core::{DomainError, JobTag, RequestId, Result, WorkspaceGeneration};
use kode_platform::jobs::CancellationToken;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::mpsc;
pub struct Definition {
    pub runtime_tokens: std::collections::HashMap<String, crate::oauth::BoundTokens>,
    pub collection: Arc<serde_json::Value>,
    pub root: PathBuf,
    pub environment: Option<Environment>,
    pub items: Vec<Item>,
    pub workspace: WorkspaceGeneration,
}
#[derive(Clone)]
pub struct Item {
    pub id: String,
    pub path: NodePath,
    pub name: String,
}
#[derive(Clone)]
pub struct Outcome {
    pub item: Item,
    pub status: Option<u16>,
    pub error: Option<crate::http::Failure>,
    pub validation_error: Option<String>,
    pub duration: Duration,
    pub received_bytes: u64,
}
pub struct Summary {
    pub outcomes: Vec<Outcome>,
    pub cancelled: bool,
}
pub const PROGRESS_CAPACITY: usize = 64;
#[derive(Clone)]
pub enum Progress {
    Started(Item),
    Completed(Outcome),
}
pub type ProgressSender = mpsc::Sender<Progress>;
pub type ProgressReceiver = mpsc::Receiver<Progress>;
pub fn progress_channel() -> (ProgressSender, ProgressReceiver) {
    mpsc::channel(PROGRESS_CAPACITY)
}
async fn publish(
    sender: &Option<ProgressSender>,
    event: Progress,
    cancel: &CancellationToken,
) -> Result<()> {
    let Some(sender) = sender else {
        return Ok(());
    };
    match sender.try_send(event) {
        Ok(()) => Ok(()),
        Err(mpsc::error::TrySendError::Closed(_)) => Err(DomainError::Cancelled),
        Err(mpsc::error::TrySendError::Full(event)) => {
            tokio::select! {biased;_=cancel.cancelled()=>Err(DomainError::Cancelled),sent=sender.send(event)=>sent.map_err(|_|DomainError::Cancelled)}
        }
    }
}
pub async fn execute(
    definition: Definition,
    client: stream::Executor,
    cancel: CancellationToken,
    events: Option<EventSender>,
    responses: mpsc::Sender<Arc<Response>>,
) -> Result<Summary> {
    execute_with_progress(definition, client, cancel, events, responses, None).await
}
pub async fn execute_with_progress(
    definition: Definition,
    client: stream::Executor,
    cancel: CancellationToken,
    events: Option<EventSender>,
    responses: mpsc::Sender<Arc<Response>>,
    progress: Option<ProgressSender>,
) -> Result<Summary> {
    // A disposed scheduler may drop this future while a blocking file read is
    // still running. That read must observe cancellation too.
    let _cancel_on_drop = cancel.clone().drop_guard();
    if responses.max_capacity() > 1 {
        return Err(DomainError::Unsupported(
            "REST response queue exceeds one response".into(),
        ));
    }
    if progress
        .as_ref()
        .is_some_and(|sender| sender.max_capacity() > PROGRESS_CAPACITY)
    {
        return Err(DomainError::Unsupported(
            "REST run progress queue exceeds 64 events".into(),
        ));
    }
    let mut outcomes = vec![];
    for item in definition.items {
        if responses.is_closed() {
            return Err(DomainError::Cancelled);
        }
        if cancel.is_cancelled() {
            break;
        }
        if let Err(error) = publish(&progress, Progress::Started(item.clone()), &cancel).await {
            if cancel.is_cancelled() {
                break;
            }
            return Err(error);
        }
        let collection = definition.collection.clone();
        let environment = definition.environment.clone();
        let root = definition.root.clone();
        let path = item.path.clone();
        let bound = definition.runtime_tokens.get(&item.id).cloned();
        let token = cancel.clone();
        let started = std::time::Instant::now();
        let prepared = tokio::task::spawn_blocking(move || {
            if token.is_cancelled() {
                return Err(DomainError::Cancelled);
            }
            let mut resolver =
                RequestResolver::new(&collection, path, &root, environment.as_ref())?;
            if let Some(bound) = bound {
                let token = bound.for_resolver(&resolver)?;
                resolver = resolver.with_runtime_tokens(token);
            }
            resolver.plan()?.materialize(64 * 1024 * 1024, &token)
        })
        .await
        .map_err(|_| DomainError::Unsupported("REST request preparation worker failed".into()))?;
        let result = match prepared {
            Ok(prepared) if !cancel.is_cancelled() => {
                client
                    .execute(
                        ExecutionKey {
                            tag: JobTag {
                                workspace: definition.workspace,
                                request: RequestId::fresh()?,
                                document: None,
                            },
                            item_id: item.id.clone(),
                        },
                        prepared,
                        cancel.clone(),
                        events.clone(),
                    )
                    .await
            }
            Ok(_) => Err(DomainError::Cancelled),
            Err(error) => Err(error),
        };
        match result {
            Ok(response) => {
                outcomes.push(Outcome {
                    item,
                    status: response.status_code,
                    error: response.error,
                    validation_error: None,
                    duration: response.duration,
                    received_bytes: response.received_bytes,
                });
                // Prefer an available slot even on cancellation so the last
                // partial response remains inspectable. Never wait on a closed
                // or stalled owner after cancellation.
                match responses.try_send(Arc::new(response)) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(response)) => {
                        tokio::select! { biased;
                            _ = cancel.cancelled() => break,
                            sent = responses.send(response) => {sent.map_err(|_|DomainError::Cancelled)?;}
                        }
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        return Err(DomainError::Cancelled);
                    }
                }
            }
            Err(DomainError::Cancelled) => break,
            Err(error) => outcomes.push(Outcome {
                item,
                status: None,
                error: None,
                validation_error: Some(error.to_string()),
                duration: started.elapsed(),
                received_bytes: 0,
            }),
        }
        if let Some(outcome) = outcomes.last()
            && let Err(error) =
                publish(&progress, Progress::Completed(outcome.clone()), &cancel).await
        {
            if cancel.is_cancelled() {
                break;
            }
            return Err(error);
        }
    }
    Ok(Summary {
        outcomes,
        cancelled: cancel.is_cancelled(),
    })
}
