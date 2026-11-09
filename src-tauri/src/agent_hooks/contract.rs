//! Frozen cross-lane wire contract; mirrored by src/shared/types/agent-hooks.types.ts.
//! These DTOs contain no engine, filesystem, authorization or restore implementation.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// Preserve one canonical enum while the legacy receiver is migrated.
pub use super::HookKind;

/// Dynamic ID. Validation is identical for builtin and custom IDs; reservation is
/// an adapter registry concern, not a restriction on deserialization.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AdapterId(String);
impl AdapterId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for AdapterId {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() <= 64
            && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
            && value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            Ok(Self(value))
        } else {
            Err("invalid adapter id")
        }
    }
}
impl From<AdapterId> for String {
    fn from(value: AdapterId) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DerivedState {
    Busy,
    Idle,
    NeedsApproval,
    Blocked,
    Error,
    Ended,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterStatus {
    NotInstalled,
    Installed,
    Outdated,
    Conflict,
    SourceMissing,
    Invalid,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreState {
    PendingConfirmation,
    Confirmed,
    Unconfirmed,
    Stale,
    Interrupted,
    Ended,
    SnapshotMissing,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HookErrorCode {
    ValidationError,
    Forbidden,
    Conflict,
    UnsupportedPlatform,
    UnsupportedAgentChat,
    IdentityStale,
    PlanExpired,
    InternalError,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookErrorDto {
    pub code: HookErrorCode,
    pub message: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookAction {
    Install,
    Uninstall,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterOrigin {
    Builtin,
    Custom,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookDiagnosticDto {
    pub code: String,
    pub message: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookEventCapabilityDto {
    pub native_event: String,
    pub hook_kinds: Vec<HookKind>,
    pub enabled_by_default: bool,
    pub available: bool,
    pub min_schema_version: Option<String>,
    pub min_delivery_version: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookCapabilitiesDto {
    pub chat: bool,
    pub resume: bool,
    pub events: Vec<HookEventCapabilityDto>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterStatusDto {
    pub id: AdapterId,
    pub display_name: String,
    pub version: Option<String>,
    pub origin: AdapterOrigin,
    pub state: AdapterStatus,
    pub installed_version: Option<String>,
    pub install_id: Option<String>,
    pub capabilities: HookCapabilitiesDto,
    pub diagnostics: Vec<HookDiagnosticDto>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookPreviewRequest {
    pub adapter_id: AdapterId,
    pub action: HookAction,
    pub event_options: BTreeMap<String, bool>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookFileDiffDto {
    pub path: String,
    pub before: Option<String>,
    pub after: Option<String>,
    pub mode: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookKeyDiffDto {
    pub path: String,
    pub key: Vec<String>,
    pub before: Option<String>,
    pub after: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookPreviewDto {
    pub adapter_id: AdapterId,
    pub action: HookAction,
    pub preview_id: String,
    pub fingerprint: String,
    pub files: Vec<HookFileDiffDto>,
    pub keys: Vec<HookKeyDiffDto>,
    pub capabilities: HookCapabilitiesDto,
    pub diagnostics: Vec<HookDiagnosticDto>,
}
/// Manage-only CAS request; no plugin confirmation flow in this release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookConfirmRequest {
    pub adapter_id: AdapterId,
    pub preview_id: String,
    pub fingerprint: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookMutationResult {
    pub status: AdapterStatusDto,
    pub receipt_revision: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookIdentitySource {
    Hook,
    Process,
    #[serde(rename = "legacy-unconfirmed")]
    LegacyUnconfirmed,
}
/// Persistable identity, not runtime state or permission to execute resume_argv.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookIdentityDto {
    pub adapter_id: AdapterId,
    pub agent_session_id: String,
    pub transcript_path: Option<String>,
    pub cwd: Option<String>,
    pub source: HookIdentitySource,
    pub descriptor_id: Option<String>,
    pub resume_argv: Option<Vec<String>>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookStateDto {
    pub instance_id: String,
    pub terminal_id: String,
    pub generation: u64,
    pub adapter_install_id: String,
    pub agent_session_id: String,
    pub project_id: String,
    pub state: DerivedState,
    pub identity: HookIdentityDto,
    pub restore_state: RestoreState,
    pub revision: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookStateTombstone {
    pub instance_id: String,
    pub terminal_id: String,
    pub revision: u64,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookStateSyncRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since_revision: Option<u64>,
}
/// Advance to current_revision even when authorized rows have revision gaps.
/// Snapshot replaces the authorized view and must have no tombstones.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HookStateSyncResponse {
    Snapshot {
        instance_id: String,
        current_revision: u64,
        states: Vec<HookStateDto>,
        tombstones: Vec<HookStateTombstone>,
    },
    Delta {
        instance_id: String,
        current_revision: u64,
        states: Vec<HookStateDto>,
        tombstones: Vec<HookStateTombstone>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumePlanRequest {
    pub descriptor_id: String,
    pub project_id: String,
    pub expected_generation: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumePlanDto {
    pub plan_id: String,
    pub descriptor_id: String,
    pub expected_generation: u64,
    pub expected_identity: HookIdentityDto,
    pub display_argv: Vec<String>,
    pub expires_at: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeSpawnRequest {
    pub plan_id: String,
    pub descriptor_id: String,
    pub expected_generation: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeSpawnResult {
    pub pty_id: String,
    pub generation: u64,
    pub attempt_id: String,
    pub pending_confirmation: bool,
    pub restore_state: RestoreState,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;
    use serde_json::{json, Value};

    fn roundtrip<T: DeserializeOwned + Serialize>(wire: Value) {
        // Compare actual top-level field names with the TS declaration, not just
        // a second Rust spelling. Nested DTOs are round-tripped independently.
        let type_name = std::any::type_name::<T>().rsplit("::").next().unwrap();
        if let Some(object) = wire.as_object() {
            let ts = include_str!("../../../src/shared/types/agent-hooks.types.ts");
            let declaration = format!("export interface {type_name} {{");
            let union = format!("export type {type_name} =");
            let body = if let Some((_, tail)) = ts.split_once(&declaration) {
                tail.split_once("\n}").unwrap().0
            } else {
                ts.split_once(&union)
                    .unwrap()
                    .1
                    .split("\nexport ")
                    .next()
                    .unwrap()
            };
            let fields: std::collections::BTreeSet<_> = body
                .lines()
                .filter_map(|line| {
                    let (field, _) = line.trim().split_once(':')?;
                    let field = field.trim_end_matches('?');
                    field
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_')
                        .then_some(field)
                })
                .collect();
            let actual: std::collections::BTreeSet<_> = object.keys().map(String::as_str).collect();
            if type_name == "HookStateSyncRequest" {
                assert!(actual.is_subset(&fields), "optional TS fields: {type_name}");
            } else {
                assert_eq!(actual, fields, "TS wire fields: {type_name}");
            }
        } else if let Some(value) = wire.as_str().filter(|_| type_name != "AdapterId") {
            let ts = include_str!("../../../src/shared/types/agent-hooks.types.ts");
            let declaration = format!("export type {type_name} =");
            let body = ts
                .split_once(&declaration)
                .unwrap()
                .1
                .split("\nexport ")
                .next()
                .unwrap();
            assert!(
                body.contains(&format!("'{value}'")),
                "TS enum value: {type_name}/{value}"
            );
        }
        let dto: T = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(dto).unwrap(), wire);
    }

    fn identity() -> Value {
        json!({"adapter_id":"pi", "agent_session_id":"session-1",
            "transcript_path":"/fake-home/.pi/agent/sessions/exact.jsonl", "cwd":"/project",
            "source":"hook", "descriptor_id":"descriptor-1",
            "resume_argv":["pi", "--session", "/fake-home/.pi/agent/sessions/exact.jsonl"]})
    }
    fn capabilities() -> Value {
        json!({"chat":true, "resume":true, "events":[{
            "native_event":"Notification", "hook_kinds":["needs_approval", "idle"],
            "enabled_by_default":true, "available":false,
            "min_schema_version":"1.0.23", "min_delivery_version":null
        }]})
    }
    fn status() -> Value {
        json!({"id":"pi", "display_name":"pi", "version":"3.0.0", "origin":"builtin",
            "state":"outdated", "installed_version":"2.0.0", "install_id":"install-1",
            "capabilities":capabilities(), "diagnostics":[{"code":"legacy", "message":"Upgrade available"}]})
    }
    fn state() -> Value {
        json!({"instance_id":"instance-1", "terminal_id":"pty-1", "generation":2,
            "adapter_install_id":"install-1", "agent_session_id":"session-1", "project_id":"project-1",
            "state":"busy", "identity":identity(), "restore_state":"confirmed", "revision":7})
    }

    #[test]
    fn typescript_snake_case_dtos_roundtrip_without_field_loss() {
        roundtrip::<AdapterStatusDto>(status());
        roundtrip::<HookCapabilitiesDto>(capabilities());
        roundtrip::<HookEventCapabilityDto>(capabilities()["events"][0].clone());
        roundtrip::<HookDiagnosticDto>(json!({"code":"legacy", "message":"Upgrade available"}));
        roundtrip::<HookFileDiffDto>(
            json!({"path":"/fake-home/file", "before":"original\n", "after":null, "mode":null}),
        );
        roundtrip::<HookKeyDiffDto>(
            json!({"path":"/fake-home/config", "key":["hooks"], "before":"[]", "after":null}),
        );
        roundtrip::<HookStateTombstone>(
            json!({"instance_id":"instance-1", "terminal_id":"pty-2", "revision":9}),
        );
        roundtrip::<HookIdentityDto>(
            json!({"adapter_id":"my-custom", "agent_session_id":"session-2",
            "transcript_path":null, "cwd":null, "source":"legacy-unconfirmed", "descriptor_id":null, "resume_argv":null}),
        );
        roundtrip::<HookPreviewRequest>(json!({"adapter_id":"pi", "action":"install",
            "event_options":{"tool_execution_start":false, "agent_start":true}}));
        roundtrip::<HookPreviewDto>(
            json!({"adapter_id":"pi", "action":"install", "preview_id":"preview-1",
            "fingerprint":"digest", "files":[{"path":"/fake-home/config", "before":null,
            "after":"exact bytes\n", "mode":"0700"}], "keys":[{"path":"/fake-home/settings.json",
            "key":["hooks","SessionStart"], "before":null, "after":"[]"}],
            "capabilities":capabilities(), "diagnostics":[]}),
        );
        roundtrip::<HookConfirmRequest>(
            json!({"adapter_id":"pi", "preview_id":"preview-1", "fingerprint":"digest"}),
        );
        roundtrip::<HookMutationResult>(json!({"status":status(), "receipt_revision":3}));
        roundtrip::<HookIdentityDto>(identity());
        roundtrip::<HookStateDto>(state());
        roundtrip::<HookStateSyncRequest>(json!({}));
        roundtrip::<HookStateSyncRequest>(json!({"instance_id":"instance-1", "since_revision":5}));
        roundtrip::<HookStateSyncResponse>(json!({"kind":"snapshot", "instance_id":"instance-1",
            "current_revision":8, "states":[state()], "tombstones":[]}));
        roundtrip::<HookStateSyncResponse>(json!({"kind":"delta", "instance_id":"instance-1",
            "current_revision":9, "states":[state()], "tombstones":[{
            "instance_id":"instance-1", "terminal_id":"pty-2", "revision":9}]}));
        roundtrip::<ResumePlanRequest>(
            json!({"descriptor_id":"descriptor-1", "project_id":"project-1", "expected_generation":2}),
        );
        roundtrip::<ResumePlanDto>(
            json!({"plan_id":"plan-1", "descriptor_id":"descriptor-1", "expected_generation":2,
            "expected_identity":identity(), "display_argv":["pi", "--session", "/fake-home/exact.jsonl"], "expires_at":60000}),
        );
        roundtrip::<ResumeSpawnRequest>(
            json!({"plan_id":"plan-1", "descriptor_id":"descriptor-1", "expected_generation":2}),
        );
        roundtrip::<ResumeSpawnResult>(
            json!({"pty_id":"pty-3", "generation":3, "attempt_id":"attempt-1",
            "pending_confirmation":true, "restore_state":"pending_confirmation"}),
        );
        roundtrip::<HookErrorDto>(json!({"code":"IDENTITY_STALE", "message":"Identity mismatch"}));
    }

    #[test]
    fn all_fixed_wire_enums_match_typescript() {
        for value in [
            "session_start",
            "session_end",
            "prompt_submitted",
            "tool_start",
            "tool_end",
            "needs_approval",
            "blocked",
            "idle",
            "turn_done",
            "error",
        ] {
            roundtrip::<HookKind>(json!(value));
        }
        for value in [
            "busy",
            "idle",
            "needs_approval",
            "blocked",
            "error",
            "ended",
        ] {
            roundtrip::<DerivedState>(json!(value));
        }
        for value in [
            "not_installed",
            "installed",
            "outdated",
            "conflict",
            "source_missing",
            "invalid",
        ] {
            roundtrip::<AdapterStatus>(json!(value));
        }
        for value in [
            "pending_confirmation",
            "confirmed",
            "unconfirmed",
            "stale",
            "interrupted",
            "ended",
            "snapshot_missing",
        ] {
            roundtrip::<RestoreState>(json!(value));
        }
        for value in [
            "VALIDATION_ERROR",
            "FORBIDDEN",
            "CONFLICT",
            "UNSUPPORTED_PLATFORM",
            "UNSUPPORTED_AGENT_CHAT",
            "IDENTITY_STALE",
            "PLAN_EXPIRED",
            "INTERNAL_ERROR",
        ] {
            roundtrip::<HookErrorCode>(json!(value));
        }
        for value in ["hook", "process", "legacy-unconfirmed"] {
            roundtrip::<HookIdentitySource>(json!(value));
        }
    }

    #[test]
    fn adapter_ids_and_requests_fail_closed() {
        for value in ["claude-code", "pi", "qin-code", "my-custom-adapter", "a"] {
            roundtrip::<AdapterId>(json!(value));
        }
        for value in [
            "",
            "Pi",
            "../pi",
            "a_b",
            "a/b",
            "1pi",
            "pi\n",
            &"a".repeat(65),
        ] {
            assert!(serde_json::from_value::<AdapterId>(json!(value)).is_err());
        }
        assert!(serde_json::from_value::<HookKind>(json!("turn_start")).is_err());
        assert!(serde_json::from_value::<HookStateSyncResponse>(json!({"kind":"events"})).is_err());
        assert!(serde_json::from_value::<ResumeSpawnRequest>(
            json!({"plan_id":"p", "descriptor_id":"d",
            "expected_generation":1, "argv":["untrusted"]})
        )
        .is_err());
        assert!(serde_json::from_value::<ResumePlanRequest>(
            json!({"descriptor_id":"d", "project_id":"p",
            "expected_generation":-1})
        )
        .is_err());
    }
}
