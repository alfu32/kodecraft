use kode_core::WorkspaceGeneration;
use kode_platform::jobs::CancellationToken;
use kode_rest::{
    model::NodePath,
    runner::{self, Definition, Item, Progress},
    stream::Executor,
};
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::mpsc;
fn definition(count: usize) -> Definition {
    Definition {
        collection: Arc::new(
            json!({"info":{},"item":(0..count).map(|index|json!({"id":"duplicate","name":format!("Item {index}"),"request":{"url":"{{missing}}"}})).collect::<Vec<_>>()}),
        ),
        root: PathBuf::from("."),
        environment: None,
        items: (0..count)
            .map(|index| Item {
                id: format!("runtime-{index}"),
                path: NodePath(vec![index]),
                name: format!("Item {index}"),
            })
            .collect(),
        workspace: WorkspaceGeneration(1),
    }
}
#[tokio::test]
async fn progress_reports_ordered_start_and_validation_outcomes_without_reaching_transport() {
    let (progress, mut receiver) = runner::progress_channel();
    let (responses, mut response_receiver) = mpsc::channel(1);
    let task = tokio::spawn(runner::execute_with_progress(
        definition(3),
        Executor::default(),
        CancellationToken::new(),
        None,
        responses,
        Some(progress),
    ));
    let mut events = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(progress) = receiver.recv().await {
            match progress {
                Progress::Started(item) => events.push((item.id, false)),
                Progress::Completed(outcome) => {
                    assert!(
                        outcome
                            .validation_error
                            .as_ref()
                            .unwrap()
                            .contains("Unresolved")
                    );
                    events.push((outcome.item.id, true));
                }
            }
        }
    })
    .await
    .unwrap();
    let summary = task.await.unwrap().unwrap();
    assert_eq!(summary.outcomes.len(), 3);
    assert!(!summary.cancelled);
    assert_eq!(
        events,
        vec![
            ("runtime-0".into(), false),
            ("runtime-0".into(), true),
            ("runtime-1".into(), false),
            ("runtime-1".into(), true),
            ("runtime-2".into(), false),
            ("runtime-2".into(), true)
        ]
    );
    assert!(response_receiver.try_recv().is_err());
}
#[tokio::test]
async fn cancelling_a_full_progress_queue_returns_partial_summary_and_does_not_start_the_next_item()
{
    let (progress, mut receiver) = mpsc::channel(1);
    let (responses, _response_receiver) = mpsc::channel(1);
    let cancel = CancellationToken::new();
    let task = tokio::spawn(runner::execute_with_progress(
        definition(2),
        Executor::default(),
        cancel.clone(),
        None,
        responses,
        Some(progress),
    ));
    assert!(
        matches!(receiver.recv().await.unwrap(),Progress::Started(item) if item.id=="runtime-0")
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while receiver.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    let summary = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(summary.cancelled);
    assert_eq!(summary.outcomes.len(), 1);
    assert_eq!(summary.outcomes[0].item.id, "runtime-0");
    assert!(
        matches!(receiver.recv().await.unwrap(),Progress::Completed(outcome) if outcome.item.id=="runtime-0")
    );
    assert!(receiver.try_recv().is_err());
}
#[tokio::test]
async fn disposed_and_oversized_progress_owners_fail_before_item_preparation() {
    let (progress, receiver) = runner::progress_channel();
    drop(receiver);
    let (responses, _response_receiver) = mpsc::channel(1);
    assert!(matches!(
        runner::execute_with_progress(
            definition(1),
            Executor::default(),
            CancellationToken::new(),
            None,
            responses,
            Some(progress)
        )
        .await,
        Err(kode_core::DomainError::Cancelled)
    ));
    let (progress, _receiver) = mpsc::channel(65);
    let (responses, _response_receiver) = mpsc::channel(1);
    assert!(matches!(
        runner::execute_with_progress(
            definition(1),
            Executor::default(),
            CancellationToken::new(),
            None,
            responses,
            Some(progress)
        )
        .await,
        Err(kode_core::DomainError::Unsupported(_))
    ));
}
