//! Background-service DTOs; service execution belongs to the host.
use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const HOST_WAKE_SERVICE: &str = "se.host.wake";
pub const HOST_PROCESS_SERVICE: &str = "se.host.process";
pub const HOST_WAKE_METHOD_REQUEST: &str = "request";
pub const HOST_WAKE_DELIVERY_METHOD: &str = "wake";
pub const HOST_PROCESS_METHODS: &[&str] = &["status", "start", "stop", "restart", "logs"];
pub const WAKE_COALESCE_MS: u64 = 50;
pub const WAKE_RATE_PER_SECOND: usize = 20;
pub const WAKE_BURST: usize = 40;
pub const MAX_WAKE_REASONS: usize = 8;
pub const MAX_WAKE_REASON_CHARS: usize = 64;
pub const MAX_MANAGED_PROCESSES: usize = 4;
pub const MAX_PROCESS_ARGS: usize = 64;
pub const MAX_PROCESS_ENV: usize = 64;
pub const MAX_PROCESS_LOG_TAIL: u32 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessRestart {
    Never,
    #[default]
    OnFailure,
    Always,
}
fn default_autostart() -> bool {
    true
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessContribution {
    pub id: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub cwd: Option<String>,
    #[serde(default = "default_autostart")]
    pub autostart: bool,
    #[serde(default)]
    pub restart: ProcessRestart,
}
pub fn is_absolute_process_path(v: &str) -> bool {
    v.starts_with('/')
        || (v.starts_with("\\\\") && v.len() > 2)
        || (v.len() >= 3
            && v.as_bytes()[0].is_ascii_alphabetic()
            && v.as_bytes()[1] == b':'
            && matches!(v.as_bytes()[2], b'/' | b'\\'))
}
fn process_text(v: &str, empty: bool) -> bool {
    (empty || !v.is_empty()) && chars(v) <= 4096 && !v.contains('\0')
}
pub fn validate_process_contribution(v: &ProcessContribution) -> Result<(), PluginError> {
    let code = PLUGIN_INVALID_MANIFEST;
    require(
        is_local_id(&v.id)
            && process_text(&v.command, false)
            && (is_absolute_process_path(&v.command) || is_package_path(&v.command)),
        code,
        "invalid process command",
    )?;
    require(
        v.args.len() <= MAX_PROCESS_ARGS
            && v.args.iter().all(|a| process_text(a, true))
            && v.env.len() <= MAX_PROCESS_ENV,
        code,
        "invalid process arguments",
    )?;
    for (key, value) in &v.env {
        require(
            !key.is_empty()
                && key.len() <= 128
                && (key.as_bytes()[0].is_ascii_alphabetic() || key.starts_with('_'))
                && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                && process_text(value, true),
            code,
            "invalid process environment",
        )?;
    }
    if let Some(cwd) = &v.cwd {
        require(
            process_text(cwd, false)
                && (cwd == "package" || cwd == "profileData" || is_absolute_process_path(cwd)),
            code,
            "invalid process cwd",
        )?;
    }
    Ok(())
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostWakeRequest {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub reason: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostWakeDelivery {
    pub generation: String,
    pub count: u64,
    pub reasons: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostProcessRequest {
    pub id: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostProcessState {
    Stopped,
    Starting,
    Running,
    Exited,
    Crashed,
    Backoff,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostProcessStatus {
    pub state: HostProcessState,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub pid: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub exit_code: Option<i32>,
    pub restarts: u64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub started_at: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostProcessLogsRequest {
    pub id: String,
    pub tail: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostProcessStream {
    Stdout,
    Stderr,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostProcessLogLine {
    pub stream: HostProcessStream,
    pub at: u64,
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostProcessLogsReply {
    pub lines: Vec<HostProcessLogLine>,
    pub truncated: bool,
}

pub fn validate_host_wake_request(v: &HostWakeRequest) -> Result<(), PluginError> {
    if let Some(reason) = &v.reason {
        text(reason, 0, MAX_WAKE_REASON_CHARS, PLUGIN_INVALID_REQUEST)?;
    }
    Ok(())
}
pub fn validate_host_wake_delivery(v: &HostWakeDelivery) -> Result<(), PluginError> {
    let code = PLUGIN_INVALID_REQUEST;
    require(is_uuid(&v.generation), code, "invalid generation")?;
    revision(v.count, code)?;
    let mut seen = BTreeSet::new();
    require(
        v.reasons.len() <= MAX_WAKE_REASONS
            && v.reasons
                .iter()
                .all(|r| chars(r) <= MAX_WAKE_REASON_CHARS && seen.insert(r)),
        code,
        "invalid wake reasons",
    )
}
pub fn validate_host_process_request(v: &HostProcessRequest) -> Result<(), PluginError> {
    require(
        is_local_id(&v.id),
        PLUGIN_INVALID_REQUEST,
        "invalid process id",
    )
}
pub fn validate_host_process_status(v: &HostProcessStatus) -> Result<(), PluginError> {
    require(
        v.restarts <= MAX_SAFE_INTEGER
            && v.pid.is_none_or(|p| p > 0 && p <= MAX_SAFE_INTEGER)
            && v.started_at.is_none_or(|t| t <= MAX_SAFE_INTEGER),
        PLUGIN_INVALID_REQUEST,
        "invalid process status",
    )
}
pub fn validate_host_process_logs_request(v: &HostProcessLogsRequest) -> Result<(), PluginError> {
    require(
        is_local_id(&v.id) && v.tail <= MAX_PROCESS_LOG_TAIL,
        PLUGIN_INVALID_REQUEST,
        "invalid process log request",
    )
}
pub fn validate_host_process_logs_reply(v: &HostProcessLogsReply) -> Result<(), PluginError> {
    require(
        v.lines.len() <= MAX_PROCESS_LOG_TAIL as usize
            && v.lines
                .iter()
                .all(|l| l.at <= MAX_SAFE_INTEGER && chars(&l.text) <= 4096),
        PLUGIN_INVALID_REQUEST,
        "invalid process logs",
    )?;
    json_size(v, PLUGIN_INVALID_REQUEST)
}
macro_rules! parser {
    ($name:ident, $ty:ty, $validate:ident) => {
        pub fn $name(bytes: &[u8]) -> Result<$ty, PluginError> {
            let v =
                crate::host::strict_contract_json(bytes, PLUGIN_INVALID_REQUEST, MAX_JSON_BYTES)?;
            $validate(&v)?;
            Ok(v)
        }
    };
}
parser!(
    parse_host_wake_request,
    HostWakeRequest,
    validate_host_wake_request
);
parser!(
    parse_host_wake_delivery,
    HostWakeDelivery,
    validate_host_wake_delivery
);
parser!(
    parse_host_process_request,
    HostProcessRequest,
    validate_host_process_request
);
parser!(
    parse_host_process_status,
    HostProcessStatus,
    validate_host_process_status
);
parser!(
    parse_host_process_logs_request,
    HostProcessLogsRequest,
    validate_host_process_logs_request
);
parser!(
    parse_host_process_logs_reply,
    HostProcessLogsReply,
    validate_host_process_logs_reply
);
