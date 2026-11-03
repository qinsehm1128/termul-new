use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub abi: AbiRange,
    pub contributes: Contributions,
    pub services: Services,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AbiRange {
    pub min: u32,
    pub max: u32,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contributions {
    pub commands: Vec<CommandContribution>,
    pub configuration: ConfigurationSchema,
    pub views: Vec<ViewContribution>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandContribution {
    pub id: String,
    pub title: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub description: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewContribution {
    pub id: String,
    pub title: String,
    pub location: ViewLocation,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewLocation {
    Panel,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Services {
    pub provides: Vec<ServiceDeclaration>,
    pub requires: Vec<ServiceDeclaration>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceDeclaration {
    pub name: String,
    pub version: u32,
}

pub fn validate_manifest(manifest: &Manifest) -> Result<(), PluginError> {
    let code = PLUGIN_INVALID_MANIFEST;
    version(manifest.schema_version, code)?;
    require(is_plugin_id(&manifest.id), code, "invalid plugin id")?;
    text(&manifest.name, 1, 128, code)?;
    require(
        semver::Version::parse(&manifest.version).is_ok(),
        code,
        "invalid semantic version",
    )?;
    require(
        manifest.abi.min > 0 && manifest.abi.min <= manifest.abi.max,
        code,
        "invalid ABI range",
    )?;
    let c = &manifest.contributes;
    require(
        c.commands.len() <= 64 && c.views.len() <= 8,
        code,
        "too many contributions",
    )?;
    let mut commands = BTreeSet::new();
    for command in &c.commands {
        require(
            is_local_id(&command.id) && commands.insert(&command.id),
            code,
            "invalid or duplicate command id",
        )?;
        text(&command.title, 1, 128, code)?;
        if let Some(description) = &command.description {
            text(description, 0, 2048, code)?;
        }
    }
    let mut views = BTreeSet::new();
    for view in &c.views {
        require(
            is_local_id(&view.id) && views.insert(&view.id),
            code,
            "invalid or duplicate view id",
        )?;
        text(&view.title, 1, 128, code)?;
    }
    validate_configuration_schema(&c.configuration)
        .map_err(|_| PluginError::new(code, "invalid configuration schema"))?;
    for services in [&manifest.services.provides, &manifest.services.requires] {
        require(services.len() <= 32, code, "too many service declarations")?;
        let mut seen = std::collections::BTreeSet::new();
        for service in services {
            require(is_plugin_id(&service.name), code, "invalid service name")?;
            require(
                (1..=i32::MAX as u32).contains(&service.version),
                code,
                "invalid service version",
            )?;
            require(
                seen.insert((service.name.as_str(), service.version)),
                code,
                "duplicate service declaration",
            )?;
        }
    }
    json_size(manifest, code)
}
/// Decode and validate untrusted bytes, including UTF-8 and the 256 KiB cap.
pub fn parse_manifest(bytes: &[u8]) -> Result<Manifest, PluginError> {
    let value = json(bytes, PLUGIN_INVALID_MANIFEST)?;
    validate_manifest(&value)?;
    Ok(value)
}
impl Manifest {
    pub fn validate(&self) -> Result<(), PluginError> {
        validate_manifest(self)
    }
    pub fn from_json(bytes: &[u8]) -> Result<Self, PluginError> {
        parse_manifest(bytes)
    }
    pub fn supports_abi(&self, abi: u32) -> bool {
        (self.abi.min..=self.abi.max).contains(&abi)
    }
}
