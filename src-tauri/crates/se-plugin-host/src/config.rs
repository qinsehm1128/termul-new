//! Pure configuration preparation. The runtime owns callback/registry commit ordering.

use crate::registry::RegistryEntry;
use se_plugin_protocol::{
    complete_config_values, validate_config_payload, validate_config_snapshot,
    validate_config_value, validate_manifest, validate_set_config_request, ConfigPayload,
    ConfigSnapshot, Manifest, PluginError, SetConfigRequest, MAX_SAFE_INTEGER,
    PLUGIN_CONFIG_CONFLICT, PLUGIN_CONFIG_INVALID, PLUGIN_CONFIG_MIGRATION_REQUIRED,
    SCHEMA_VERSION,
};

pub(crate) fn snapshot(entry: &RegistryEntry) -> Result<ConfigSnapshot, PluginError> {
    let schema = entry.manifest.contributes.configuration.clone();
    let result = ConfigSnapshot {
        plugin_id: entry.id.clone(),
        values: complete_config_values(&schema, &entry.config_overrides)?,
        schema,
        revision: entry.config_revision,
        overrides: entry.config_overrides.clone(),
    };
    validate_config_snapshot(&result)?;
    Ok(result)
}

pub(crate) fn payload(
    entry: &RegistryEntry,
    changed_keys: Vec<String>,
) -> Result<ConfigPayload, PluginError> {
    let current = snapshot(entry)?;
    let result = ConfigPayload {
        schema_version: SCHEMA_VERSION,
        plugin_id: current.plugin_id,
        revision: current.revision,
        values: current.values,
        changed_keys,
    };
    validate_config_payload(&current.schema, &result)?;
    Ok(result)
}

/// Prepare, but never persist, a replacement entry. Missing keys reset to defaults.
/// Overrides equal to defaults are removed so restoring defaults unblocks upgrades.
pub(crate) fn update(
    entry: &RegistryEntry,
    request: &SetConfigRequest,
) -> Result<Option<RegistryEntry>, PluginError> {
    if request.id != entry.id {
        return Err(PluginError::new(
            PLUGIN_CONFIG_INVALID,
            "configuration plugin id mismatch",
        ));
    }
    if request.expected_revision != entry.config_revision {
        return Err(PluginError::new(
            PLUGIN_CONFIG_CONFLICT,
            "configuration revision changed; reload before saving",
        ));
    }
    let current = snapshot(entry)?;
    let values = validate_set_config_request(request, &current.schema)?;
    if values == current.values {
        return Ok(None);
    }
    if entry.config_revision >= MAX_SAFE_INTEGER {
        return Err(PluginError::new(
            PLUGIN_CONFIG_INVALID,
            "configuration revision exhausted",
        ));
    }
    let mut next = entry.clone();
    next.config_revision += 1;
    next.config_overrides = values
        .into_iter()
        .filter(|(key, value)| current.schema.properties[key].default != *value)
        .collect();
    // Include aggregate wire-size constraints, not just individual value validation.
    snapshot(&next)?;
    Ok(Some(next))
}

/// Defaults may change freely; explicit user overrides must remain compatible.
pub(crate) fn migration(entry: &RegistryEntry, candidate: &Manifest) -> Result<(), PluginError> {
    validate_manifest(candidate)?;
    for (key, value) in &entry.config_overrides {
        let compatible = candidate
            .contributes
            .configuration
            .properties
            .get(key)
            .is_some_and(|property| validate_config_value(property, value).is_ok());
        if !compatible {
            return Err(PluginError::new(
                PLUGIN_CONFIG_MIGRATION_REQUIRED,
                format!("configuration key '{key}' is incompatible; restore its default before upgrading"),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use se_plugin_protocol::{ConfigValue, ConfigValues};

    fn entry() -> RegistryEntry {
        RegistryEntry {
            id: "test.plugin".into(),
            manifest: serde_json::from_value(serde_json::json!({
                "schemaVersion": 1, "id": "test.plugin", "name": "Test", "version": "1.0.0",
                "abi": {"min": 1, "max": 1},
                "contributes": {"commands": [], "views": [], "configuration": {
                    "title": "Settings", "properties": {
                        "greeting": {"type": "string", "title": "Greeting", "description": "", "default": "Hello", "maxLength": 20},
                        "step": {"type": "number", "title": "Step", "description": "", "default": 1, "minimum": 1, "maximum": 5},
                        "enabled": {"type": "boolean", "title": "Enabled", "description": "", "default": true},
                        "tone": {"type": "string", "title": "Tone", "description": "", "default": "normal", "enum": ["normal", "muted"]}
                    }
                }}, "services": {"provides": [], "requires": []}
            })).unwrap(),
            source_path: "/source/plugin.dylib".into(),
            artifact_path: "/packages/plugin.dylib".into(),
            sha256: "a".repeat(64),
            package: None,
            desired_enabled: false,
            config_revision: 1,
            config_overrides: ConfigValues::new(),
            quarantine: None,
        }
    }

    fn request(values: ConfigValues) -> SetConfigRequest {
        SetConfigRequest {
            id: "test.plugin".into(),
            expected_revision: 1,
            values,
        }
    }

    #[test]
    fn fills_defaults_and_prepares_only_nondefault_overrides() {
        let original = entry();
        assert_eq!(snapshot(&original).unwrap().values.len(), 4);
        let mut values = ConfigValues::new();
        values.insert("greeting".into(), ConfigValue::String("Hola".into()));
        values.insert("step".into(), ConfigValue::Number(1.0));
        let next = update(&original, &request(values)).unwrap().unwrap();
        assert_eq!(next.config_revision, 2);
        assert_eq!(next.config_overrides.len(), 1);
        assert_eq!(original.config_revision, 1);
        assert!(original.config_overrides.is_empty());
        let wire = payload(&next, vec!["greeting".into()]).unwrap();
        assert_eq!(wire.values.len(), 4);
        assert_eq!(wire.revision, 2);
        let reset = update(
            &next,
            &SetConfigRequest {
                expected_revision: 2,
                ..request(ConfigValues::new())
            },
        )
        .unwrap()
        .unwrap();
        assert!(reset.config_overrides.is_empty());
    }

    #[test]
    fn identical_values_are_noop_but_stale_revision_is_conflict() {
        let original = entry();
        assert!(update(&original, &request(ConfigValues::new()))
            .unwrap()
            .is_none());
        let full = snapshot(&original).unwrap().values;
        assert!(update(&original, &request(full.clone())).unwrap().is_none());
        let stale = SetConfigRequest {
            expected_revision: 2,
            ..request(full)
        };
        assert_eq!(
            update(&original, &stale).unwrap_err().code,
            PLUGIN_CONFIG_CONFLICT
        );
    }

    #[test]
    fn rejects_unknown_keys_types_bounds_enums_and_bad_payload_keys() {
        let original = entry();
        for (key, value) in [
            ("unknown", ConfigValue::Boolean(true)),
            ("step", ConfigValue::Boolean(true)),
            ("step", ConfigValue::Number(6.0)),
            ("step", ConfigValue::Number(f64::NAN)),
            ("greeting", ConfigValue::String("x".repeat(21))),
            ("tone", ConfigValue::String("bad".into())),
        ] {
            let values = [(key.to_owned(), value)].into_iter().collect();
            assert_eq!(
                update(&original, &request(values)).unwrap_err().code,
                PLUGIN_CONFIG_INVALID
            );
        }
        assert!(payload(&original, vec!["unknown".into()]).is_err());
        assert!(payload(&original, vec!["step".into(), "step".into()]).is_err());
        let mut wrong = request(ConfigValues::new());
        wrong.id = "other.plugin".into();
        assert!(update(&original, &wrong).is_err());
    }

    #[test]
    fn migration_keeps_overrides_and_rejects_incompatible_keys_without_values() {
        let mut original = entry();
        original.config_overrides.insert(
            "greeting".into(),
            ConfigValue::String("private-value".into()),
        );
        let mut candidate = original.manifest.clone();
        candidate
            .contributes
            .configuration
            .properties
            .get_mut("step")
            .unwrap()
            .default = ConfigValue::Number(2.0);
        migration(&original, &candidate).unwrap();
        candidate
            .contributes
            .configuration
            .properties
            .get_mut("greeting")
            .unwrap()
            .max_length = Some(5);
        let err = migration(&original, &candidate).unwrap_err();
        assert_eq!(err.code, PLUGIN_CONFIG_MIGRATION_REQUIRED);
        assert!(err.message.contains("greeting"));
        assert!(!err.message.contains("private-value"));
        candidate
            .contributes
            .configuration
            .properties
            .remove("greeting");
        assert_eq!(
            migration(&original, &candidate).unwrap_err().code,
            PLUGIN_CONFIG_MIGRATION_REQUIRED
        );
    }

    #[test]
    fn revision_cannot_overflow_safe_integer() {
        let mut original = entry();
        original.config_revision = MAX_SAFE_INTEGER;
        let mut change = request(
            [(String::from("step"), ConfigValue::Number(2.0))]
                .into_iter()
                .collect(),
        );
        change.expected_revision = MAX_SAFE_INTEGER;
        assert!(update(&original, &change).is_err());
        change.values.clear();
        assert!(update(&original, &change).unwrap().is_none());
    }
}
