use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConfigValue {
    String(String),
    Number(f64),
    Boolean(bool),
}
pub type ConfigValues = BTreeMap<String, ConfigValue>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigurationType {
    String,
    Number,
    Boolean,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationProperty {
    #[serde(rename = "type")]
    pub kind: ConfigurationType,
    pub title: String,
    pub description: String,
    pub default: ConfigValue,
    #[serde(
        rename = "enum",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub enum_values: Option<Vec<ConfigValue>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub enum_labels: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub min_length: Option<u32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub max_length: Option<u32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub minimum: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub maximum: Option<f64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationSchema {
    pub title: String,
    pub properties: BTreeMap<String, ConfigurationProperty>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigPayload {
    pub schema_version: u32,
    pub plugin_id: String,
    pub revision: u64,
    pub values: ConfigValues,
    pub changed_keys: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivationPayload {
    pub schema_version: u32,
    pub plugin_id: String,
    pub generation: String,
    pub config: ConfigPayload,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigSnapshot {
    pub plugin_id: String,
    pub schema: ConfigurationSchema,
    pub revision: u64,
    pub values: ConfigValues,
    pub overrides: ConfigValues,
}

fn type_matches(property: &ConfigurationProperty, value: &ConfigValue) -> bool {
    matches!(
        (property.kind, value),
        (ConfigurationType::String, ConfigValue::String(_))
            | (ConfigurationType::Number, ConfigValue::Number(_))
            | (ConfigurationType::Boolean, ConfigValue::Boolean(_))
    )
}
pub fn validate_config_value(
    property: &ConfigurationProperty,
    value: &ConfigValue,
) -> Result<(), PluginError> {
    let code = PLUGIN_CONFIG_INVALID;
    require(
        type_matches(property, value),
        code,
        "configuration type mismatch",
    )?;
    match value {
        ConfigValue::String(s) => text(
            s,
            property.min_length.unwrap_or(0) as usize,
            property.max_length.unwrap_or(8192) as usize,
            code,
        )?,
        ConfigValue::Number(n) => {
            require(n.is_finite(), code, "configuration number must be finite")?;
            require(
                property.minimum.is_none_or(|min| *n >= min)
                    && property.maximum.is_none_or(|max| *n <= max),
                code,
                "configuration number outside bounds",
            )?;
        }
        ConfigValue::Boolean(_) => {}
    }
    if let Some(values) = &property.enum_values {
        require(
            values.contains(value),
            code,
            "configuration value not in enum",
        )?;
    }
    Ok(())
}
pub fn validate_configuration_schema(schema: &ConfigurationSchema) -> Result<(), PluginError> {
    let code = PLUGIN_CONFIG_INVALID;
    text(&schema.title, 1, 128, code)?;
    require(
        schema.properties.len() <= 64,
        code,
        "too many configuration properties",
    )?;
    for (key, p) in &schema.properties {
        require(is_local_id(key), code, "invalid configuration key")?;
        text(&p.title, 1, 128, code)?;
        text(&p.description, 0, 2048, code)?;
        match p.kind {
            ConfigurationType::String => {
                require(
                    p.minimum.is_none() && p.maximum.is_none(),
                    code,
                    "numeric constraint on string",
                )?;
                let min = p.min_length.unwrap_or(0);
                let max = p.max_length.unwrap_or(8192);
                require(
                    min <= max && max <= 8192,
                    code,
                    "invalid string length bounds",
                )?;
            }
            ConfigurationType::Number => {
                require(
                    p.min_length.is_none() && p.max_length.is_none(),
                    code,
                    "length constraint on number",
                )?;
                require(
                    p.minimum.is_none_or(f64::is_finite) && p.maximum.is_none_or(f64::is_finite),
                    code,
                    "non-finite numeric bounds",
                )?;
                if let (Some(min), Some(max)) = (p.minimum, p.maximum) {
                    require(min <= max, code, "inverted numeric bounds")?;
                }
            }
            ConfigurationType::Boolean => require(
                p.min_length.is_none()
                    && p.max_length.is_none()
                    && p.minimum.is_none()
                    && p.maximum.is_none(),
                code,
                "constraint on boolean",
            )?,
        }
        if let Some(values) = &p.enum_values {
            require(
                (1..=64).contains(&values.len()),
                code,
                "invalid enum length",
            )?;
            for (i, v) in values.iter().enumerate() {
                require(type_matches(p, v), code, "enum type mismatch")?;
                require(!values[..i].contains(v), code, "duplicate enum value")?;
                if let ConfigValue::Number(n) = v {
                    require(n.is_finite(), code, "non-finite enum number")?;
                }
                if let ConfigValue::String(s) = v {
                    text(s, 0, 8192, code)?;
                }
            }
        }
        if let Some(labels) = &p.enum_labels {
            require(
                p.enum_values
                    .as_ref()
                    .is_some_and(|v| v.len() == labels.len()),
                code,
                "enumLabels requires equally sized enum",
            )?;
            for label in labels {
                text(label, 0, 8192, code)?;
            }
        }
        validate_config_value(p, &p.default)?;
    }
    json_size(schema, code)
}
/// Validate supplied values, reject unknown keys, and fill omitted keys from defaults.
pub fn complete_config_values(
    schema: &ConfigurationSchema,
    supplied: &ConfigValues,
) -> Result<ConfigValues, PluginError> {
    validate_configuration_schema(schema)?;
    for (key, value) in supplied {
        let p = schema
            .properties
            .get(key)
            .ok_or_else(|| PluginError::new(PLUGIN_CONFIG_INVALID, "unknown configuration key"))?;
        validate_config_value(p, value)?;
    }
    let values = schema
        .properties
        .iter()
        .map(|(key, p)| (key.clone(), supplied.get(key).unwrap_or(&p.default).clone()))
        .collect();
    Ok(values)
}
pub fn validate_config_payload(
    schema: &ConfigurationSchema,
    payload: &ConfigPayload,
) -> Result<(), PluginError> {
    let code = PLUGIN_CONFIG_INVALID;
    version(payload.schema_version, code)?;
    require(is_plugin_id(&payload.plugin_id), code, "invalid pluginId")?;
    revision(payload.revision, code)?;
    let complete = complete_config_values(schema, &payload.values)?;
    require(
        complete == payload.values,
        code,
        "configuration payload must contain all values",
    )?;
    let mut seen = BTreeSet::new();
    for key in &payload.changed_keys {
        require(
            schema.properties.contains_key(key) && seen.insert(key),
            code,
            "invalid or duplicate changed key",
        )?;
    }
    json_size(payload, code)
}
pub fn parse_config_payload(
    bytes: &[u8],
    schema: &ConfigurationSchema,
) -> Result<ConfigPayload, PluginError> {
    let payload = json(bytes, PLUGIN_CONFIG_INVALID)?;
    validate_config_payload(schema, &payload)?;
    Ok(payload)
}
pub fn validate_activation_payload(
    schema: &ConfigurationSchema,
    payload: &ActivationPayload,
) -> Result<(), PluginError> {
    version(payload.schema_version, PLUGIN_CONFIG_INVALID)?;
    require(
        is_plugin_id(&payload.plugin_id)
            && payload.plugin_id == payload.config.plugin_id
            && is_uuid(&payload.generation),
        PLUGIN_CONFIG_INVALID,
        "invalid activation identity",
    )?;
    validate_config_payload(schema, &payload.config)?;
    require(
        payload.config.changed_keys.is_empty(),
        PLUGIN_CONFIG_INVALID,
        "initial activation changedKeys must be empty",
    )?;
    json_size(payload, PLUGIN_CONFIG_INVALID)
}
pub fn validate_config_snapshot(snapshot: &ConfigSnapshot) -> Result<(), PluginError> {
    revision(snapshot.revision, PLUGIN_CONFIG_INVALID)?;
    require(
        is_plugin_id(&snapshot.plugin_id),
        PLUGIN_CONFIG_INVALID,
        "invalid pluginId",
    )?;
    let complete = complete_config_values(&snapshot.schema, &snapshot.overrides)?;
    require(
        complete == snapshot.values,
        PLUGIN_CONFIG_INVALID,
        "snapshot values do not match defaults plus overrides",
    )?;
    json_size(snapshot, PLUGIN_CONFIG_INVALID)
}
pub fn parse_configuration_schema(bytes: &[u8]) -> Result<ConfigurationSchema, PluginError> {
    let schema = json(bytes, PLUGIN_CONFIG_INVALID)?;
    validate_configuration_schema(&schema)?;
    Ok(schema)
}
pub fn parse_activation_payload(
    bytes: &[u8],
    schema: &ConfigurationSchema,
) -> Result<ActivationPayload, PluginError> {
    let payload = json(bytes, PLUGIN_CONFIG_INVALID)?;
    validate_activation_payload(schema, &payload)?;
    Ok(payload)
}
impl ConfigurationSchema {
    pub fn validate(&self) -> Result<(), PluginError> {
        validate_configuration_schema(self)
    }
    pub fn complete(&self, values: &ConfigValues) -> Result<ConfigValues, PluginError> {
        complete_config_values(self, values)
    }
}
