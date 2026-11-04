//! Static package inventory and browser-safe path contracts. No archive I/O.
use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_PACKAGE_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_PACKAGE_EXPANDED_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_PACKAGE_FILES: usize = 2048;
pub const MAX_PACKAGE_NATIVE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_PACKAGE_UI_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_PACKAGE_DESCRIPTOR_BYTES: usize = 256 * 1024;
pub const MAX_PACKAGE_PATH_BYTES: usize = 512;

/// Portable ASCII paths; the same rules apply to archive entries and web entries.
pub fn is_package_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PACKAGE_PATH_BYTES
        && path.split('/').all(|segment| {
            let stem = segment.split('.').next().unwrap_or("").to_ascii_uppercase();
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && !segment.ends_with('.')
                && !segment.eq_ignore_ascii_case("__se__")
                && !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                && !(stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && matches!(stem.as_bytes()[3], b'1'..=b'9'))
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
}
pub fn is_web_entry(path: &str) -> bool {
    is_package_path(path) && path.ends_with(".html")
}
pub fn is_sha256(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageDescriptor {
    pub schema_version: u32,
    pub native: BTreeMap<String, String>,
    pub ui: String,
    pub files: Vec<PackageFile>,
}
pub fn validate_package_descriptor(v: &PackageDescriptor) -> Result<(), PluginError> {
    let code = PLUGIN_INVALID_PACKAGE;
    version(v.schema_version, code)?;
    require(
        v.ui == "ui"
            && !v.native.is_empty()
            && !v.files.is_empty()
            && v.files.len() <= MAX_PACKAGE_FILES,
        code,
        "invalid package inventory",
    )?;
    let mut paths = BTreeSet::new();
    let mut total = 0u64;
    for file in &v.files {
        require(
            is_package_path(&file.path)
                && file.path != "package.json"
                && paths.insert(file.path.to_ascii_lowercase())
                && is_sha256(&file.sha256),
            code,
            "invalid package file",
        )?;
        let cap = if file.path == "manifest.json" {
            MAX_PACKAGE_DESCRIPTOR_BYTES as u64
        } else if file.path.starts_with("native/") {
            MAX_PACKAGE_NATIVE_BYTES
        } else if file.path.starts_with("ui/") {
            MAX_PACKAGE_UI_BYTES
        } else {
            0
        };
        require(
            cap > 0 && file.size <= cap,
            code,
            "invalid package file size or root",
        )?;
        total = total
            .checked_add(file.size)
            .ok_or_else(|| PluginError::new(code, "package too large"))?;
    }
    require(
        total <= MAX_PACKAGE_EXPANDED_BYTES && v.files.iter().any(|f| f.path == "manifest.json"),
        code,
        "missing manifest or excessive payload",
    )?;
    for (target, path) in &v.native {
        require(
            (1..=128).contains(&target.len())
                && target.split('-').count() >= 3
                && target
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                && is_package_path(path)
                && path.starts_with(&format!("native/{target}/"))
                && v.files.iter().any(|f| &f.path == path),
            code,
            "invalid native target inventory",
        )?;
    }
    json_size(v, code)
}
pub fn parse_package_descriptor(bytes: &[u8]) -> Result<PackageDescriptor, PluginError> {
    let value = crate::host::strict_contract_json(
        bytes,
        PLUGIN_INVALID_PACKAGE,
        MAX_PACKAGE_DESCRIPTOR_BYTES,
    )?;
    validate_package_descriptor(&value)?;
    Ok(value)
}
