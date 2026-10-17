use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
pub type Extra = BTreeMap<String, Value>;
pub type DataSourceId = String;
fn yes() -> bool {
    true
}
fn generic() -> String {
    "generic".into()
}
fn generic_name() -> String {
    "Generic JDBC Driver".into()
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JdbcRepositoryType {
    #[default]
    MavenCentral,
    MavenCustom,
    VendorUrl,
    LocalFile,
}
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct JdbcDriverArtifact {
    pub repository_type: JdbcRepositoryType,
    pub group_id: Option<String>,
    pub artifact_id: Option<String>,
    pub version: Option<String>,
    pub classifier: Option<String>,
    #[serde(
        serialize_with = "crate::security::serialize_optional_locator",
        deserialize_with = "crate::security::deserialize_optional_locator"
    )]
    pub repository: Option<String>,
    #[serde(
        serialize_with = "crate::security::serialize_optional_locator",
        deserialize_with = "crate::security::deserialize_optional_locator"
    )]
    pub vendor_download_page: Option<String>,
    #[serde(
        serialize_with = "crate::security::serialize_optional_locator",
        deserialize_with = "crate::security::deserialize_optional_locator"
    )]
    pub download_url: Option<String>,
    #[serde(flatten, serialize_with = "crate::security::serialize_extra")]
    pub extra: Extra,
}
impl std::fmt::Debug for JdbcDriverArtifact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JdbcDriverArtifact")
            .field("repository_type", &self.repository_type)
            .field("group_id", &self.group_id)
            .field("artifact_id", &self.artifact_id)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}
impl JdbcDriverArtifact {
    pub fn coordinate_label(&self) -> String {
        [
            self.group_id.as_deref(),
            self.artifact_id.as_deref(),
            self.classifier
                .as_deref()
                .filter(|value| !value.trim().is_empty()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(":")
    }
    pub fn from_coordinates(value: &str) -> Option<Self> {
        let parts: Vec<_> = value.split(':').collect();
        if parts.len() < 2 {
            return None;
        }
        Some(Self {
            group_id: Some(parts[0].into()),
            artifact_id: Some(parts[1].into()),
            version: parts.get(2).map(|value| (*value).into()),
            classifier: parts.get(3).map(|value| (*value).into()),
            ..Self::default()
        })
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriverSpec {
    #[serde(default = "generic")]
    pub id: String,
    #[serde(default = "generic_name")]
    pub display_name: String,
    pub driver_class: String,
    #[serde(default)]
    pub coordinates: Option<String>,
    #[serde(default)]
    pub jar_path: Option<String>,
    #[serde(default)]
    pub artifact: Option<JdbcDriverArtifact>,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(flatten, serialize_with = "crate::security::serialize_extra")]
    pub extra: Extra,
}
impl std::fmt::Debug for DriverSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DriverSpec")
            .field("id", &self.id)
            .field("driver_class", &self.driver_class)
            .finish_non_exhaustive()
    }
}
impl DriverSpec {
    pub fn local_classpath(&self, separator: char) -> Vec<std::path::PathBuf> {
        self.jar_path
            .as_deref()
            .into_iter()
            .flat_map(|paths| paths.split(separator))
            .filter(|path| !path.is_empty())
            .map(std::path::PathBuf::from)
            .collect()
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataSourceDefinition {
    pub id: DataSourceId,
    pub name: String,
    pub driver: DriverSpec,
    #[serde(
        serialize_with = "crate::security::serialize_url",
        deserialize_with = "crate::security::deserialize_url"
    )]
    pub jdbc_url: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub credential_reference: Option<String>,
    #[serde(
        default,
        serialize_with = "crate::security::serialize_properties",
        deserialize_with = "crate::security::deserialize_properties"
    )]
    pub properties: BTreeMap<String, String>,
    #[serde(default)]
    pub credential_properties: BTreeMap<String, String>,
    #[serde(default)]
    pub default_catalog: Option<String>,
    #[serde(default)]
    pub default_schema: Option<String>,
    #[serde(default = "yes")]
    pub auto_commit: bool,
    #[serde(default)]
    pub read_only: bool,
    #[serde(flatten, serialize_with = "crate::security::serialize_extra")]
    pub extra: Extra,
}
impl std::fmt::Debug for DataSourceDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataSourceDefinition")
            .field("id", &self.id)
            .field("driver", &self.driver)
            .finish_non_exhaustive()
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Error,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStatus {
    pub data_source_id: DataSourceId,
    #[serde(default)]
    pub state: ConnectionState,
    #[serde(default)]
    pub message: Option<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ConnectionTestResult {
    pub success: bool,
    pub database_product: Option<String>,
    pub database_version: Option<String>,
    pub driver_name: Option<String>,
    pub driver_version: Option<String>,
    pub latency_ms: u64,
    pub sql_state: Option<String>,
    pub vendor_code: Option<i32>,
    pub message: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DatabaseObjectType {
    DataSource,
    Catalog,
    Schema,
    Category,
    Table,
    View,
    Column,
    Procedure,
    Function,
    Package,
    Type,
    Index,
    PrimaryKey,
    ForeignKey,
    Message,
    Error,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DatabaseProjectState {
    pub data_sources: Vec<DataSourceDefinition>,
    pub sessions: Vec<DatabaseSessionState>,
    #[serde(flatten, serialize_with = "crate::security::serialize_extra")]
    pub extra: Extra,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseSessionState {
    pub id: String,
    pub data_source_id: DataSourceId,
    pub title: String,
    #[serde(default)]
    pub catalog: Option<String>,
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default = "yes")]
    pub auto_commit: bool,
    #[serde(default)]
    pub buffer: String,
    #[serde(flatten, serialize_with = "crate::security::serialize_extra")]
    pub extra: Extra,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseObject {
    pub id: String,
    pub name: String,
    pub qualified_name: String,
    pub object_type: DatabaseObjectType,
    #[serde(default)]
    pub catalog: Option<String>,
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
    #[serde(default)]
    pub has_children: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DatabaseCapabilities {
    pub catalogs: bool,
    pub schemas: bool,
    pub tables: bool,
    pub views: bool,
    pub procedures: bool,
    pub functions: bool,
    pub types: bool,
    pub packages: bool,
    pub package_members: bool,
    pub package_source: bool,
    pub package_ddl: bool,
    pub transactions: bool,
    pub savepoints: bool,
}
impl Default for DatabaseCapabilities {
    fn default() -> Self {
        Self {
            catalogs: false,
            schemas: false,
            tables: true,
            views: true,
            procedures: true,
            functions: true,
            types: true,
            packages: false,
            package_members: false,
            package_source: false,
            package_ddl: false,
            transactions: true,
            savepoints: true,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MetadataRequestKind {
    Root,
    Catalog,
    Schema,
    Category,
    Object,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataRequest {
    pub kind: MetadataRequestKind,
    #[serde(default)]
    pub catalog: Option<String>,
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default)]
    pub category: Option<DatabaseObjectType>,
    #[serde(default)]
    pub object_name: Option<String>,
    #[serde(default)]
    pub object_type: Option<DatabaseObjectType>,
    /// Provider-specific stable identity for objects with multiple versions.
    #[serde(default)]
    pub object_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataResult {
    pub objects: Vec<DatabaseObject>,
    #[serde(default)]
    pub capabilities: Option<DatabaseCapabilities>,
    #[serde(default)]
    pub message: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MetadataSourceKind {
    Ddl,
    Source,
    SqlSource,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataSourceDocument {
    pub title: String,
    pub kind: MetadataSourceKind,
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataSourceResult {
    pub context_revision: u64,
    pub documents: Vec<MetadataSourceDocument>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlExecutionRequest {
    pub data_source_id: DataSourceId,
    pub console_id: String,
    pub sql: String,
    #[serde(default)]
    pub catalog: Option<String>,
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default = "max_rows")]
    pub max_rows: u32,
    #[serde(default = "fetch_size")]
    pub fetch_size: u32,
}
fn max_rows() -> u32 {
    500
}
fn fetch_size() -> u32 {
    200
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SqlExecutionResult {
    pub result_sets: Vec<SqlResultSet>,
    pub update_counts: Vec<i64>,
    pub warnings: Vec<String>,
    pub execution_time_ms: u64,
    pub error: Option<SqlExecutionError>,
    pub started_at: Option<String>,
}
impl SqlExecutionResult {
    pub fn success(&self) -> bool {
        self.error.is_none()
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlResultSet {
    pub label: String,
    pub columns: Vec<SqlColumn>,
    pub rows: Vec<Vec<SqlCell>>,
    pub limited: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlColumn {
    pub name: String,
    pub jdbc_type: i32,
    pub type_name: String,
    pub nullable: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlCell {
    pub value: Option<String>,
    pub jdbc_type: i32,
    pub null_value: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqlExecutionError {
    pub message: String,
    #[serde(default)]
    pub sql_state: Option<String>,
    #[serde(default)]
    pub vendor_code: Option<i32>,
}
