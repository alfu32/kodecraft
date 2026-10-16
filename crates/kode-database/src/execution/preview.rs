use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use std::collections::BTreeSet;
use tokio::sync::OwnedSemaphorePermit;

const CELL_BYTES: usize = 64 * 1024;
const ROW_BYTES: usize = 512 * 1024;
const PREVIEW_BYTES: usize = 32 * 1024 * 1024;
#[derive(Default)]
struct Retention {
    permits: Mutex<Vec<OwnedSemaphorePermit>>,
}
impl std::fmt::Debug for Retention {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Retention(..)")
    }
}

#[derive(Debug, Clone)]
pub struct ResultSet {
    pub id: String,
    pub columns: Arc<[Column]>,
    pub chunks: Vec<Arc<[Vec<Cell>]>>,
    pub received_rows: usize,
    pub limited: bool,
    pub incomplete: bool,
}
/// Clones share row chunks and their retained-byte permits. Keeping a previous
/// execution in the UI therefore keeps its budget charged until it is released.
#[derive(Debug, Clone, Default)]
pub struct Preview {
    pub result_sets: Vec<ResultSet>,
    pub update_counts: Vec<i64>,
    pub warnings: Vec<SqlError>,
    pub incomplete: bool,
    pub retention_limited: bool,
    pub retained_bytes: usize,
    retention: Arc<Retention>,
}
pub(super) struct Accumulator {
    pub snapshot: Snapshot,
    pub next: u64,
    pub limit_reached: bool,
    id: String,
    started: bool,
    current: Option<(String, usize, Option<usize>)>,
    seen: BTreeSet<String>,
    budget: Arc<Semaphore>,
}
fn invalid() -> BridgeError {
    BridgeError::new(
        Failure::Protocol,
        "Invalid SQL result event, sequence or preview bound",
    )
}
fn bounded_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}
fn sql_error(value: &SqlError) -> bool {
    let mut stack = vec![value];
    let mut count = 0;
    while let Some(value) = stack.pop() {
        count += 1;
        if count > 17
            || value.message.len() > crate::bridge::wire::SQL_ERROR_MESSAGE_BYTES
            || value
                .sql_state
                .as_ref()
                .is_some_and(|value| value.len() > 8192)
        {
            return false;
        }
        stack.extend(&value.chained);
    }
    true
}
fn row(value: &[Cell], width: usize) -> bool {
    if value.len() != width {
        return false;
    }
    let mut bytes = 0;
    for cell in value {
        let size = match cell {
            Cell::Null => 0,
            Cell::Text { text, .. } => text.len(),
            Cell::Binary { base64, .. } => {
                if base64.len() > 87384 {
                    return false;
                }
                let Ok(decoded) = STANDARD.decode(base64) else {
                    return false;
                };
                decoded.len()
            }
        };
        if size > CELL_BYTES {
            return false;
        }
        bytes += size;
    }
    bytes <= ROW_BYTES
}
impl Accumulator {
    pub fn new(id: String, budget: Arc<Semaphore>) -> Self {
        Self {
            snapshot: Snapshot {
                execution_id: Some(id.clone()),
                ..Default::default()
            },
            next: 0,
            limit_reached: false,
            id,
            started: false,
            current: None,
            seen: BTreeSet::new(),
            budget,
        }
    }
    pub fn accept(
        &mut self,
        event: QueryNotification,
        size: usize,
    ) -> std::result::Result<bool, BridgeError> {
        if size == 0
            || size > 8 * 1024 * 1024
            || event.execution_id != self.id
            || event.sequence != self.next
            || self.snapshot.phase == Phase::Finished
        {
            return Err(invalid());
        }
        if !self.started && !matches!(event.event, QueryEvent::Started) {
            return Err(invalid());
        }
        match &event.event {
            QueryEvent::Started => {
                if self.started || self.next != 0 {
                    return Err(invalid());
                }
            }
            QueryEvent::ResultSetStart {
                result_set_id,
                columns,
            } => {
                if self.current.is_some()
                    || !bounded_id(result_set_id)
                    || self.seen.contains(result_set_id)
                    || self.seen.len() >= 1024
                    || columns.is_empty()
                    || columns.len() > 256
                    || columns
                        .iter()
                        .any(|column| column.label.len() > 1024 || column.type_name.len() > 1024)
                {
                    return Err(invalid());
                }
            }
            QueryEvent::Rows {
                result_set_id,
                rows,
            } => {
                let Some((current, width, _)) = &self.current else {
                    return Err(invalid());
                };
                if current != result_set_id
                    || rows.is_empty()
                    || rows.len() > 200
                    || !rows.iter().all(|value| row(value, *width))
                {
                    return Err(invalid());
                }
            }
            QueryEvent::ResultSetEnd { result_set_id, .. } => {
                if self
                    .current
                    .as_ref()
                    .is_none_or(|(current, _, _)| current != result_set_id)
                {
                    return Err(invalid());
                }
            }
            QueryEvent::UpdateCount { count } => {
                if self.current.is_some() || *count < 0 {
                    return Err(invalid());
                }
            }
            QueryEvent::Warning { error } => {
                if !sql_error(error) {
                    return Err(invalid());
                }
            }
        }
        self.next = self.next.checked_add(1).ok_or_else(invalid)?;
        let retained =
            if self.limit_reached || size > PREVIEW_BYTES - self.snapshot.preview.retained_bytes {
                false
            } else if let Ok(permit) = self.budget.clone().try_acquire_many_owned(size as u32) {
                self.snapshot
                    .preview
                    .retention
                    .permits
                    .lock()
                    .expect("SQL retention")
                    .push(permit);
                true
            } else {
                false
            };
        let new_limit = !retained && !self.limit_reached;
        if !retained {
            self.limit_reached = true;
            self.snapshot.phase = Phase::Cancelling;
            let preview = Arc::make_mut(&mut self.snapshot.preview);
            preview.incomplete = true;
            preview.retention_limited = true;
        } else {
            Arc::make_mut(&mut self.snapshot.preview).retained_bytes += size;
        }
        match event.event {
            QueryEvent::Started => {
                self.started = true;
                if !self.limit_reached {
                    self.snapshot.phase = Phase::Running;
                }
            }
            QueryEvent::ResultSetStart {
                result_set_id,
                columns,
            } => {
                self.seen.insert(result_set_id.clone());
                let width = columns.len();
                let index = if retained {
                    let preview = Arc::make_mut(&mut self.snapshot.preview);
                    let index = preview.result_sets.len();
                    preview.result_sets.push(ResultSet {
                        id: result_set_id.clone(),
                        columns: columns.into(),
                        chunks: Vec::new(),
                        received_rows: 0,
                        limited: false,
                        incomplete: false,
                    });
                    Some(index)
                } else {
                    None
                };
                self.current = Some((result_set_id, width, index));
            }
            QueryEvent::Rows { rows, .. } => {
                if let Some((_, _, Some(index))) = self.current.as_ref() {
                    let preview = Arc::make_mut(&mut self.snapshot.preview);
                    let result = &mut preview.result_sets[*index];
                    if retained {
                        result.received_rows += rows.len();
                        result.chunks.push(rows.into());
                    } else {
                        result.incomplete = true;
                    }
                }
            }
            QueryEvent::ResultSetEnd {
                limited,
                incomplete,
                ..
            } => {
                if let Some((_, _, Some(index))) = self.current.take() {
                    let preview = Arc::make_mut(&mut self.snapshot.preview);
                    let result = &mut preview.result_sets[index];
                    result.limited = limited;
                    result.incomplete |= incomplete || !retained;
                    preview.incomplete |= incomplete;
                }
            }
            QueryEvent::UpdateCount { count } => {
                if retained {
                    Arc::make_mut(&mut self.snapshot.preview)
                        .update_counts
                        .push(count);
                }
            }
            QueryEvent::Warning { error } => {
                if retained {
                    Arc::make_mut(&mut self.snapshot.preview)
                        .warnings
                        .push(error);
                }
            }
        }
        Ok(new_limit)
    }
    pub fn finish(&mut self, mut result: QueryFinal) -> std::result::Result<(), BridgeError> {
        if result.execution_id != self.id
            || result.event_count != Some(self.next)
            || result.error.as_ref().is_some_and(|value| !sql_error(value))
            || result.outcome == Outcome::Completed && (!self.started || self.current.is_some())
        {
            return Err(invalid());
        }
        result.preview_incomplete |= self.limit_reached;
        let preview = Arc::make_mut(&mut self.snapshot.preview);
        preview.incomplete |= result.preview_incomplete || result.outcome != Outcome::Completed;
        if let Some((_, _, Some(index))) = self.current.take() {
            preview.result_sets[index].incomplete = true;
            preview.incomplete = true;
        }
        self.snapshot.phase = Phase::Finished;
        self.snapshot.completion = Some(result);
        Ok(())
    }
    pub fn fail(&mut self, failure: BridgeError) {
        let preview = Arc::make_mut(&mut self.snapshot.preview);
        preview.incomplete = true;
        if let Some((_, _, Some(index))) = self.current.take() {
            preview.result_sets[index].incomplete = true;
        }
        self.snapshot.phase = Phase::Finished;
        self.snapshot.unknown_outcome = failure.unknown_outcome;
        self.snapshot.error = Some(failure);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn event(sequence: u64, value: serde_json::Value) -> QueryNotification {
        serde_json::from_value(
            json!({"executionId":"synthetic-execution","sequence":sequence,"event":value}),
        )
        .unwrap()
    }
    fn start() -> serde_json::Value {
        json!({"type":"resultSetStart","resultSetId":"r","columns":[{"label":"value","jdbcType":12,"typeName":"VARCHAR","nullable":true}]})
    }
    fn finish(count: u64, outcome: &str) -> QueryFinal {
        serde_json::from_value(json!({"executionId":"synthetic-execution","outcome":outcome,"elapsedMs":1,"transactionUsable":true,"previewIncomplete":false,"eventCount":count})).unwrap()
    }
    #[test]
    fn shared_wire_rows_preserve_null_binary_exact_decimal_and_all_events_before_final() {
        let fixture: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../../protocol/jdbc-v1/query-fixtures.json"
        ))
        .unwrap();
        let mut accumulator = Accumulator::new(
            "synthetic-execution".into(),
            Arc::new(Semaphore::new(1024 * 1024)),
        );
        for value in fixture["events"].as_array().unwrap() {
            accumulator
                .accept(
                    serde_json::from_value(value.clone()).unwrap(),
                    serde_json::to_vec(value).unwrap().len(),
                )
                .unwrap();
        }
        let mut final_value = fixture["final"].clone();
        final_value["eventCount"] = json!(accumulator.next);
        accumulator
            .finish(serde_json::from_value(final_value).unwrap())
            .unwrap();
        let preview = &accumulator.snapshot.preview;
        assert_eq!(preview.result_sets.len(), 1);
        assert_eq!(preview.result_sets[0].received_rows, 3);
        assert!(preview.result_sets[0].limited);
        let rows = &preview.result_sets[0].chunks[0];
        assert_eq!(rows[0][0], Cell::Null);
        assert!(matches!(&rows[0][1],Cell::Binary{base64,..}if base64=="AP8="));
        assert!(
            matches!(&rows[2][0],Cell::Text{text,..}if text=="12345678901234567890.000000000000000001")
        );
        assert_eq!(preview.update_counts, [i64::MAX]);
        assert_eq!(preview.warnings.len(), 1);
        assert_eq!(accumulator.snapshot.phase, Phase::Finished);
    }
    #[test]
    fn retained_preview_budget_survives_result_clones_and_limit_is_explicit_not_silent_loss() {
        let budget = Arc::new(Semaphore::new(512));
        let mut accumulator = Accumulator::new("synthetic-execution".into(), budget.clone());
        accumulator
            .accept(event(0, json!({"type":"started"})), 100)
            .unwrap();
        accumulator.accept(event(1, start()), 200).unwrap();
        assert!(accumulator.accept(event(2,json!({"type":"rows","resultSetId":"r","rows":[[{"kind":"text","text":"kept only if capacity allows"}]]})),300).unwrap());
        assert_eq!(accumulator.snapshot.phase, Phase::Cancelling);
        accumulator
            .accept(
                event(
                    3,
                    json!({"type":"resultSetEnd","resultSetId":"r","limited":false}),
                ),
                100,
            )
            .unwrap();
        accumulator.finish(finish(4, "completed")).unwrap();
        assert!(accumulator.snapshot.preview.incomplete);
        assert!(
            accumulator
                .snapshot
                .completion
                .as_ref()
                .unwrap()
                .preview_incomplete
        );
        assert!(accumulator.snapshot.preview.result_sets[0].incomplete);
        assert_eq!(accumulator.snapshot.preview.result_sets[0].received_rows, 0);
        let snapshot = accumulator.snapshot.clone();
        drop(accumulator);
        assert_eq!(budget.available_permits(), 212);
        drop(snapshot);
        assert_eq!(budget.available_permits(), 512);
    }
    #[test]
    fn malformed_order_shapes_and_cell_previews_fail_without_discarding_prior_rows() {
        let mut accumulator = Accumulator::new(
            "synthetic-execution".into(),
            Arc::new(Semaphore::new(1024 * 1024)),
        );
        assert!(accumulator.accept(event(0, start()), 100).is_err());
        accumulator
            .accept(event(0, json!({"type":"started"})), 100)
            .unwrap();
        assert!(
            accumulator
                .accept(event(0, json!({"type":"started"})), 100)
                .is_err()
        );
        accumulator.accept(event(1, start()), 100).unwrap();
        assert!(
            accumulator
                .accept(
                    event(2, json!({"type":"rows","resultSetId":"r","rows":[[]]})),
                    100
                )
                .is_err()
        );
        assert!(accumulator.accept(event(2,json!({"type":"rows","resultSetId":"r","rows":[[{"kind":"binary","base64":"***"}]]})),100).is_err());
        assert!(accumulator.accept(event(2,json!({"type":"rows","resultSetId":"r","rows":[[{"kind":"text","text":"x".repeat(65537)}]]})),100).is_err());
        assert!(
            accumulator
                .accept(
                    event(
                        2,
                        json!({"type":"rows","resultSetId":"r","rows":[[{"kind":"null"}]]})
                    ),
                    0
                )
                .is_err()
        );
        accumulator.accept(event(2,json!({"type":"rows","resultSetId":"r","rows":[[{"kind":"text","text":"synthetic-private-row"}]]})),100).unwrap();
        assert!(accumulator.finish(finish(3, "completed")).is_err());
        let mut failure = BridgeError::new(Failure::Transport, "Helper failed");
        failure.unknown_outcome = true;
        accumulator.fail(failure);
        assert_eq!(accumulator.snapshot.preview.result_sets[0].received_rows, 1);
        assert!(accumulator.snapshot.preview.result_sets[0].incomplete);
        assert!(accumulator.snapshot.unknown_outcome);
        assert!(!format!("{:?}", accumulator.snapshot).contains("synthetic-private-row"));
        let column = Column {
            label: "synthetic-private-sql-literal".into(),
            jdbc_type: 12,
            type_name: "VARCHAR".into(),
            nullable: true,
        };
        assert!(!format!("{column:?}").contains("synthetic-private-sql-literal"));
    }
    #[test]
    fn long_redacted_error_payloads_survive_the_query_final_boundary_but_remain_bounded() {
        let mut accumulator = Accumulator::new(
            "synthetic-execution".into(),
            Arc::new(Semaphore::new(1024 * 1024)),
        );
        accumulator
            .accept(event(0, json!({"type":"started"})), 100)
            .unwrap();
        let mut result = finish(1, "failed");
        let message = format!("FIRST\n{}\nTAIL", "diagnostic ".repeat(2000));
        result.error = Some(SqlError {
            message: message.clone(),
            sql_state: Some("42000".into()),
            vendor_code: 73,
            chained: vec![],
        });
        accumulator.finish(result).unwrap();
        assert_eq!(
            accumulator
                .snapshot
                .completion
                .as_ref()
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .message,
            message
        );
        let mut too_large = finish(1, "failed");
        too_large.error = Some(SqlError {
            message: "x".repeat(crate::bridge::wire::SQL_ERROR_MESSAGE_BYTES + 1),
            sql_state: None,
            vendor_code: 0,
            chained: vec![],
        });
        assert!(accumulator.finish(too_large).is_err());
    }
}
