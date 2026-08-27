//! Typed durable usage and plan replacement schemas.
//!
//! Replay and import validate these types before publishing
//! [`crate::conversation::event_log::ConversationFrontier`] replacements.
//! Invalid payloads become `ConversationRecoveryRequired` and never clone raw
//! JSON onto the frontier.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable schema failure without payload or path provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsagePlanSchemaError {
    InvalidUsage,
    InvalidPlan,
}

impl UsagePlanSchemaError {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidUsage => "CONVERSATION_RECOVERY_REQUIRED",
            Self::InvalidPlan => "CONVERSATION_RECOVERY_REQUIRED",
        }
    }
}

impl std::fmt::Display for UsagePlanSchemaError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::error::Error for UsagePlanSchemaError {}

/// Optional cost object on a usage replacement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageCostV1 {
    pub amount: f64,
    pub currency: String,
}

/// Full canonical usage replacement (`UsageUpdate`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageUpdateV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub used: u64,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<UsageCostV1>,
}

/// One plan entry. Extra ACP fields are ignored so required structure stays
/// `content` / `priority` / `status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanEntryV1 {
    pub content: String,
    pub priority: String,
    pub status: String,
}

/// Plan body. An empty `entries` array is a valid durable clear.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanBodyV1 {
    pub entries: Vec<PlanEntryV1>,
}

/// Full canonical plan replacement (`PlanUpdate`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanUpdateV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub plan: PlanBodyV1,
}

impl UsageUpdateV1 {
    pub fn parse(value: &Value) -> Result<Self, UsagePlanSchemaError> {
        serde_json::from_value(value.clone()).map_err(|_| UsagePlanSchemaError::InvalidUsage)
    }
}

impl PlanUpdateV1 {
    pub fn parse(value: &Value) -> Result<Self, UsagePlanSchemaError> {
        serde_json::from_value(value.clone()).map_err(|_| UsagePlanSchemaError::InvalidPlan)
    }

    #[must_use]
    pub fn entries_value(&self) -> Value {
        serde_json::to_value(&self.plan.entries).unwrap_or(Value::Array(Vec::new()))
    }
}

/// Validate a durable usage payload before frontier publication.
pub fn validate_usage_update(value: &Value) -> Result<UsageUpdateV1, UsagePlanSchemaError> {
    UsageUpdateV1::parse(value)
}

/// Validate a durable plan payload before frontier publication.
///
/// Required structure is `plan.entries` as an array. Empty `entries` is a
/// valid durable clear.
pub fn validate_plan_update(value: &Value) -> Result<PlanUpdateV1, UsagePlanSchemaError> {
    PlanUpdateV1::parse(value)
}
