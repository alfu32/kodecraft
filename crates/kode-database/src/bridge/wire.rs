//! Query values are separate from persisted source DTOs. Binary and SQL NULL
//! cannot accidentally collapse into ordinary display text on the wire.
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Cell {
    Null,
    Text {
        text: String,
        #[serde(default)]
        truncated: bool,
    },
    Binary {
        base64: String,
        #[serde(default)]
        truncated: bool,
    },
}
impl std::fmt::Debug for Cell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Null => "Cell::Null",
            Self::Text { .. } => "Cell::Text(..)",
            Self::Binary { .. } => "Cell::Binary(..)",
        })
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub label: String,
    pub jdbc_type: i32,
    pub type_name: String,
    pub nullable: bool,
}
impl std::fmt::Debug for Column {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A JDBC label may be the original SQL expression, including a literal.
        f.debug_struct("Column")
            .field("jdbc_type", &self.jdbc_type)
            .field("nullable", &self.nullable)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlError {
    pub message: String,
    #[serde(default)]
    pub sql_state: Option<String>,
    pub vendor_code: i32,
    #[serde(default)]
    pub chained: Vec<SqlError>,
}
impl std::fmt::Debug for SqlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlError")
            .field("sql_state", &self.sql_state)
            .field("vendor_code", &self.vendor_code)
            .finish_non_exhaustive()
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum QueryEvent {
    Started,
    ResultSetStart {
        result_set_id: String,
        columns: Vec<Column>,
    },
    Rows {
        result_set_id: String,
        rows: Vec<Vec<Cell>>,
    },
    ResultSetEnd {
        result_set_id: String,
        limited: bool,
        #[serde(default)]
        incomplete: bool,
    },
    UpdateCount {
        count: i64,
    },
    Warning {
        error: SqlError,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryNotification {
    pub execution_id: String,
    pub sequence: u64,
    pub event: QueryEvent,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    Completed,
    Cancelled,
    Failed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryFinal {
    pub execution_id: String,
    pub outcome: Outcome,
    pub elapsed_ms: u64,
    pub transaction_usable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_count: Option<u64>,
    #[serde(default)]
    pub preview_incomplete: bool,
    #[serde(default)]
    pub error: Option<SqlError>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CancelStatus {
    Requested,
    AlreadyFinished,
    Unsupported,
}
