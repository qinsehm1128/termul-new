use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginError {
    pub code: String,
    pub message: String,
}
impl PluginError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}
impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for PluginError {}

macro_rules! error_codes {
    ($($name:ident),* $(,)?) => { $(pub const $name: &str = stringify!($name);)* };
}
error_codes! {
    PLUGIN_CORE_UNAVAILABLE, PLUGIN_INVALID_MANIFEST, PLUGIN_ABI_UNSUPPORTED,
    PLUGIN_STRUCT_TOO_SMALL, PLUGIN_INVALID_UI, PLUGIN_CONFIG_INVALID,
    PLUGIN_CONFIG_CONFLICT, PLUGIN_CONFIG_MIGRATION_REQUIRED, PLUGIN_NOT_FOUND,
    PLUGIN_ALREADY_INSTALLED, PLUGIN_DISABLED, PLUGIN_PENDING, PLUGIN_QUARANTINED,
    PLUGIN_STALE_GENERATION, PLUGIN_STALE_VIEW, PLUGIN_PANIC, PLUGIN_TIMEOUT,
    PLUGIN_BUSY, PLUGIN_IO, PLUGIN_STORE_CORRUPT, PLUGIN_SERVICE_UNAVAILABLE,
    PLUGIN_SERVICE_CYCLE, PLUGIN_NOT_SUPPORTED, PLUGIN_INVALID_ACTION,
    PLUGIN_INVALID_REQUEST, PLUGIN_CALLBACK_ERROR, PLUGIN_REENTRANT,
    PLUGIN_STORAGE_CONFLICT, PLUGIN_QUOTA_EXCEEDED,
}
