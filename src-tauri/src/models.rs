use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;
pub const FASTGIT_ID: &str = "builtin-fastgit";
pub const TEST_REPOSITORY: &str = "https://github.com/octocat/Hello-World.git";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub schema_version: u32,
    pub acceleration_enabled: bool,
    pub route_scope: RouteScope,
    pub line_mode: LineMode,
    pub fixed_node_id: Option<String>,
    pub current_node_id: Option<String>,
    pub health_check_minutes: u32,
    pub launch_at_login: bool,
    pub log_level: String,
    #[serde(default = "default_true")]
    pub usage_logging_enabled: bool,
    pub last_applied_at: Option<DateTime<Utc>>,
}

fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            acceleration_enabled: false,
            route_scope: RouteScope::Allowlist,
            line_mode: LineMode::Automatic,
            fixed_node_id: None,
            current_node_id: None,
            health_check_minutes: 30,
            launch_at_login: false,
            log_level: "info".into(),
            usage_logging_enabled: true,
            last_applied_at: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RouteScope {
    Allowlist,
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LineMode {
    Automatic,
    Fixed,
    Direct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NodeStatus {
    Untested,
    Available,
    Slow,
    Incompatible,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeDefinition {
    pub id: String,
    pub name: String,
    pub rewrite_base: String,
    pub enabled: bool,
    pub built_in: bool,
}

impl NodeDefinition {
    pub fn fastgit() -> Self {
        Self {
            id: FASTGIT_ID.into(),
            name: "FastGit".into(),
            rewrite_base: "https://fastgit.cc/https://github.com/".into(),
            enabled: true,
            built_in: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthSummary {
    pub status: NodeStatus,
    pub success_count: u32,
    pub attempt_count: u32,
    pub median_latency_ms: Option<u64>,
    pub consecutive_failures: u32,
    pub checked_at: Option<DateTime<Utc>>,
    pub failure_reason: Option<String>,
    #[serde(default)]
    pub recent_latencies_ms: Vec<u64>,
}

impl Default for HealthSummary {
    fn default() -> Self {
        Self {
            status: NodeStatus::Untested,
            success_count: 0,
            attempt_count: 0,
            median_latency_ms: None,
            consecutive_failures: 0,
            checked_at: None,
            failure_reason: None,
            recent_latencies_ms: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeEntry {
    #[serde(flatten)]
    pub node: NodeDefinition,
    pub health: HealthSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteEntry {
    pub id: String,
    pub repository_url: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentSummary {
    pub git_available: bool,
    pub git_path: Option<String>,
    pub git_version: Option<String>,
    pub include_registered: bool,
    pub config_path: String,
    pub conflicts: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
    pub settings: Settings,
    pub nodes: Vec<NodeEntry>,
    pub routes: Vec<RouteEntry>,
    pub environment: EnvironmentSummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RejectedImport {
    pub input: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub imported: usize,
    pub duplicates: usize,
    pub rejected: Vec<RejectedImport>,
    pub nodes: Vec<NodeEntry>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadTarget {
    pub original_url: String,
    pub accelerated_url: String,
    pub file_name: String,
    pub node_name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticReport {
    pub generated_at: DateTime<Utc>,
    pub git_path: Option<String>,
    pub git_version: Option<String>,
    pub config_path: String,
    pub include_registered: bool,
    pub conflicts: Vec<String>,
    pub original_url: String,
    pub fetch_url: Option<String>,
    pub push_url: Option<String>,
    pub explicit_push_url: Option<String>,
    pub warnings: Vec<String>,
    pub report_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UsageRoute {
    Accelerated,
    Direct,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEvent {
    pub id: String,
    pub occurred_at: DateTime<Utc>,
    pub command: String,
    pub repository: String,
    pub route: UsageRoute,
    pub node_name: Option<String>,
    pub connection_host: String,
    pub succeeded: bool,
    pub exit_code: i32,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLogSnapshot {
    pub enabled: bool,
    pub listening: bool,
    pub configured: bool,
    pub events: Vec<UsageEvent>,
    pub storage_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeImportFile {
    pub schema_version: u32,
    pub nodes: Vec<NodeImportItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeImportItem {
    pub name: Option<String>,
    pub rewrite_base: String,
}
