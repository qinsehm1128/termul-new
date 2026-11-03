//! Strict, runtime-neutral contracts for the native plugin protocol v1.
//! JSON entry points enforce byte limits before deserializing; validators also
//! check programmatically constructed DTOs. No plugin code is executed here.
pub mod config;
pub mod error;
pub mod host;
pub mod manifest;
pub mod request_validation;
pub mod rpc;
pub mod ui;

pub use config::*;
pub use error::*;
pub use host::*;
pub use manifest::*;
pub use request_validation::*;
pub use rpc::*;
pub use ui::*;

use serde::{de::DeserializeOwned, Deserialize, Deserializer, Serialize};

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_JSON_BYTES: usize = 256 * 1024;
pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
pub const MAX_UI_NODES: usize = 1024;
pub const MAX_UI_DEPTH: usize = 32;

pub(crate) fn version(version: u32, code: &str) -> Result<(), PluginError> {
    require(version == SCHEMA_VERSION, code, "unsupported schemaVersion")
}
pub(crate) fn require(ok: bool, code: &str, message: &str) -> Result<(), PluginError> {
    if ok {
        Ok(())
    } else {
        Err(PluginError::new(code, message))
    }
}
pub(crate) fn chars(value: &str) -> usize {
    value.chars().count()
}
pub(crate) fn text(value: &str, min: usize, max: usize, code: &str) -> Result<(), PluginError> {
    require(
        (min..=max).contains(&chars(value)),
        code,
        "string length is outside the allowed range",
    )
}
pub fn is_local_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=128).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(b))
}
pub fn is_plugin_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    if !(1..=128).contains(&bytes.len()) || !bytes[0].is_ascii_lowercase() {
        return false;
    }
    let mut segments = 0;
    for segment in value.split(['.', '-']) {
        if segment.is_empty()
            || !segment
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        {
            return false;
        }
        segments += 1;
    }
    segments >= 2
}
pub fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
pub(crate) fn revision(value: u64, code: &str) -> Result<(), PluginError> {
    require(
        (1..=MAX_SAFE_INTEGER).contains(&value),
        code,
        "revision must be a positive safe integer",
    )
}
pub(crate) fn json<T: DeserializeOwned>(bytes: &[u8], code: &str) -> Result<T, PluginError> {
    require(bytes.len() <= MAX_JSON_BYTES, code, "JSON exceeds 256 KiB")?;
    // Do not include serde's value-bearing diagnostics in a public error.
    serde_json::from_slice(bytes).map_err(|_| PluginError::new(code, "invalid JSON structure"))
}
pub(crate) fn json_size<T: Serialize>(value: &T, code: &str) -> Result<(), PluginError> {
    let bytes =
        serde_json::to_vec(value).map_err(|_| PluginError::new(code, "invalid JSON value"))?;
    require(bytes.len() <= MAX_JSON_BYTES, code, "JSON exceeds 256 KiB")
}
pub(crate) fn required_value<'de, D: Deserializer<'de>>(
    de: D,
) -> Result<serde_json::Value, D::Error> {
    serde_json::Value::deserialize(de)
}
/// A required field whose JSON value may be null; using this deserializer
/// without serde(default) rejects a missing field.
pub(crate) fn nullable_option<'de, D, T>(de: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(de)
}
/// Optional means absent, not null. Use on optional typed schema fields.
pub(crate) fn present_option<'de, D, T>(de: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(de).map(Some)
}
