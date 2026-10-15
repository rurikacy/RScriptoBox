use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn captured() -> String {
    "terminal".into()
}
fn system_encoding() -> String {
    "system".into()
}
fn success_codes() -> Vec<u32> {
    vec![0]
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SecretSlot {
    pub secret_slot: String,
    #[serde(default)]
    pub required: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InputValue {
    Text(String),
    Secret(SecretSlot),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Script {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub source_path: Option<String>,
    pub kind: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub runtime_id: Option<String>,
    #[serde(default)]
    pub environment_id: Option<String>,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub args: Vec<InputValue>,
    #[serde(default)]
    pub env: BTreeMap<String, InputValue>,
    #[serde(default = "captured")]
    pub run_mode: String,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default = "system_encoding")]
    pub encoding: String,
    #[serde(default)]
    pub execution_policy: Option<String>,
    #[serde(default = "success_codes")]
    pub success_codes: Vec<u32>,
    #[serde(default)]
    pub trusted_hash: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub exists: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Runtime {
    pub id: String,
    pub kind: String,
    pub executable_path: String,
    pub version: String,
    pub architecture: String,
    pub source: String,
    pub status: String,
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub base_prefix: Option<String>,
    #[serde(default)]
    pub has_pip: bool,
    pub checked_at: String,
    #[serde(default)]
    pub execution_policy: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    pub id: String,
    pub base_runtime_id: String,
    pub interpreter_path: String,
    pub ownership: String,
    #[serde(default)]
    pub requirements_path: Option<String>,
    #[serde(default)]
    pub requirements_hash: Option<String>,
    pub dependency_status: String,
    pub created_at: String,
    pub checked_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub script_id: Option<String>,
    pub script_name: String,
    pub script_path: String,
    pub runtime_path: String,
    pub run_mode: String,
    pub state: String,
    pub reason: Option<String>,
    pub root_exit_code: Option<u32>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub log_truncated: bool,
    #[serde(default)]
    pub plan_redacted: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum RunEvent {
    State {
        run_id: String,
        state: String,
        reason: Option<String>,
    },
    Output {
        run_id: String,
        seq: u64,
        stream: String,
        text: String,
    },
    Gap {
        run_id: String,
        next_seq: u64,
    },
    LogTruncated {
        run_id: String,
    },
    Finished {
        run_id: String,
        state: String,
        root_exit_code: Option<u32>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogPage {
    pub text: String,
    pub next_cursor: u64,
    pub has_more: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Settings {
    pub max_concurrent: u32,
    pub retention_days: u32,
    pub log_limit_mib: u64,
    pub total_log_mib: u64,
    pub default_mode: String,
    pub default_encoding: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            retention_days: 30,
            log_limit_mib: 20,
            total_log_mib: 500,
            default_mode: captured(),
            default_encoding: system_encoding(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallJob {
    pub id: String,
    pub kind: String,
    pub target: String,
    pub state: String,
    pub message: String,
    pub raw_exit_code: Option<u32>,
    pub created_at: String,
    pub finished_at: Option<String>,
    pub environment_id: Option<String>,
    pub script_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallPlan {
    pub id: String,
    pub recipe_key: String,
    pub product: String,
    pub version: String,
    pub architecture: String,
    pub scope: String,
    pub source: String,
    pub package_id: String,
    pub license_url: String,
    pub actions: Vec<String>,
    pub warning: String,
}

pub type AppResult<T> = Result<T, String>;
