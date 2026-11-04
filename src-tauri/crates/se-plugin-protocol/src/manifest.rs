use crate::*;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::LazyLock};

static VIEW_ICONS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    serde_json::from_str(include_str!(
        "../../../../src/shared/plugin-fixtures/view-icons.json"
    ))
    .expect("embedded view icon allowlist is valid JSON")
});

/// Shared host-controlled icon tokens for views and status items.
pub fn is_view_icon(value: &str) -> bool {
    VIEW_ICONS.contains(&value)
}

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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Contributions {
    pub commands: Vec<CommandContribution>,
    pub configuration: ConfigurationSchema,
    pub views: Vec<ViewContribution>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub event_subscriptions: Vec<EventSubscriptionContribution>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub menus: Vec<MenuContribution>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keybindings: Vec<KeybindingContribution>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status_items: Vec<StatusItemContribution>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub processes: Vec<ProcessContribution>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostScope {
    Window,
    ActiveProject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostEventTopic {
    #[serde(rename = "project.activeChanged")]
    ProjectActiveChanged,
    #[serde(rename = "terminal.created")]
    TerminalCreated,
    #[serde(rename = "terminal.closed")]
    TerminalClosed,
    #[serde(rename = "terminal.activeChanged")]
    TerminalActiveChanged,
    #[serde(rename = "agent.session.stateChanged")]
    AgentSessionStateChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventSubscriptionContribution {
    pub id: String,
    pub topic: HostEventTopic,
    pub scope: HostScope,
}

/// A finite conjunction, not an expression language. Missing predicates are unrestricted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContributionWhen {
    #[serde(
        rename = "project.exists",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub project_exists: Option<bool>,
    #[serde(
        rename = "terminal.exists",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub terminal_exists: Option<bool>,
    #[serde(
        rename = "agentSession.exists",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub agent_session_exists: Option<bool>,
    #[serde(
        rename = "textInput.focused",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub text_input_focused: Option<bool>,
    #[serde(
        rename = "terminal.focused",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub terminal_focused: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MenuLocation {
    #[serde(rename = "terminal.tab")]
    TerminalTab,
    #[serde(rename = "terminal.context")]
    TerminalContext,
    #[serde(rename = "project.context")]
    ProjectContext,
    #[serde(rename = "agentSession.context")]
    AgentSessionContext,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MenuContribution {
    pub id: String,
    pub location: MenuLocation,
    pub command_id: String,
    #[serde(default)]
    pub order: i32,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub when: Option<ContributionWhen>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeybindingContribution {
    pub id: String,
    pub command_id: String,
    pub key: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub mac: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub when: Option<ContributionWhen>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StatusItemLocation {
    StatusBar,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatusItemContribution {
    pub id: String,
    pub label: String,
    pub location: StatusItemLocation,
    pub scope: HostScope,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub command_id: Option<String>,
    #[serde(default)]
    pub order: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostServiceAvailability {
    Available,
    Unavailable,
}

/// Reserved names may never be supplied by plugins, including unknown future names.
pub fn is_reserved_host_service(name: &str) -> bool {
    name.starts_with("se.host.")
}

/// Only supported catalog versions are recognized. Secrets is known but deliberately
/// unavailable: no plaintext fallback or credential backend is part of this contract.
pub fn known_host_service(name: &str, version: u32) -> Option<HostServiceAvailability> {
    if version != 1 {
        return None;
    }
    match name {
        "se.host.context" | "se.host.events" | "se.host.storage" | "se.host.feedback"
        | "se.host.status" | "se.host.ui" | "se.host.wake" | "se.host.process" => {
            Some(HostServiceAvailability::Available)
        }
        "se.host.secrets" => Some(HostServiceAvailability::Unavailable),
        _ => None,
    }
}

/// Canonical lowercase accelerator, or empty for an intentionally unbound shortcut.
pub fn is_manifest_keybinding(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    let parts: Vec<_> = value.split('+').collect();
    let Some((key, modifiers)) = parts.split_last() else {
        return false;
    };
    let mut previous = None;
    let mut primary = false;
    for modifier in modifiers {
        let rank = match *modifier {
            "ctrl" => {
                primary = true;
                0
            }
            "cmd" => {
                primary = true;
                1
            }
            "shift" => 2,
            "alt" => 3,
            _ => return false,
        };
        if previous.is_some_and(|last| last >= rank) {
            return false;
        }
        previous = Some(rank);
    }
    primary
        && (matches!(
            *key,
            "enter"
                | "space"
                | "tab"
                | "escape"
                | "backspace"
                | "delete"
                | "home"
                | "end"
                | "pageup"
                | "pagedown"
                | "arrowup"
                | "arrowdown"
                | "arrowleft"
                | "arrowright"
                | "f1"
                | "f2"
                | "f3"
                | "f4"
                | "f5"
                | "f6"
                | "f7"
                | "f8"
                | "f9"
                | "f10"
                | "f11"
                | "f12"
        ) || (key.len() == 1
            && key
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())))
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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub kind: Option<ViewKind>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub entry: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub allow: Option<Vec<WebAllow>>,
    pub title: String,
    pub location: ViewLocation,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_option"
    )]
    pub icon: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewLocation {
    Panel,
    Sidebar,
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
    require(
        c.views
            .iter()
            .filter(|view| view.location == ViewLocation::Sidebar)
            .count()
            <= 2,
        code,
        "too many sidebar views",
    )?;
    let mut views = BTreeSet::new();
    for view in &c.views {
        require(
            is_local_id(&view.id) && views.insert(&view.id),
            code,
            "invalid or duplicate view id",
        )?;
        text(&view.title, 1, 128, code)?;
        if let Some(icon) = &view.icon {
            require(is_view_icon(icon), code, "invalid view icon")?;
        }
    }
    require(
        c.event_subscriptions.len() <= 16
            && c.menus.len() <= 32
            && c.keybindings.len() <= 16
            && c.status_items.len() <= 2,
        code,
        "too many host contributions",
    )?;
    let mut subscriptions = BTreeSet::new();
    for subscription in &c.event_subscriptions {
        require(
            is_local_id(&subscription.id) && subscriptions.insert(&subscription.id),
            code,
            "invalid or duplicate subscription id",
        )?;
        require(
            subscription.topic != HostEventTopic::ProjectActiveChanged
                || subscription.scope == HostScope::Window,
            code,
            "project selection subscriptions require window scope",
        )?;
    }
    if !c.event_subscriptions.is_empty() {
        for name in ["se.host.context", "se.host.events"] {
            require(
                manifest
                    .services
                    .requires
                    .iter()
                    .any(|s| s.name == name && s.version == 1),
                code,
                "event subscriptions require context and events v1",
            )?;
        }
    }
    let mut menus = BTreeSet::new();
    for menu in &c.menus {
        require(
            is_local_id(&menu.id) && menus.insert(&menu.id),
            code,
            "invalid or duplicate menu id",
        )?;
        require(
            commands.contains(&menu.command_id),
            code,
            "undeclared menu command",
        )?;
        require(
            (-100..=100).contains(&menu.order),
            code,
            "invalid menu order",
        )?;
    }
    let mut keybindings = BTreeSet::new();
    for binding in &c.keybindings {
        require(
            is_local_id(&binding.id) && keybindings.insert(&binding.id),
            code,
            "invalid or duplicate keybinding id",
        )?;
        require(
            commands.contains(&binding.command_id),
            code,
            "undeclared keybinding command",
        )?;
        require(
            is_manifest_keybinding(&binding.key)
                && binding.mac.as_deref().is_none_or(is_manifest_keybinding),
            code,
            "invalid keybinding accelerator",
        )?;
        if let Some(when) = &binding.when {
            require(
                when.text_input_focused != Some(true) && when.terminal_focused != Some(true),
                code,
                "keybindings cannot require input or terminal focus",
            )?;
        }
    }
    let mut status_items = BTreeSet::new();
    for item in &c.status_items {
        require(
            is_local_id(&item.id) && status_items.insert(&item.id),
            code,
            "invalid or duplicate status item id",
        )?;
        text(&item.label, 1, 128, code)?;
        require(
            (-100..=100).contains(&item.order),
            code,
            "invalid status item order",
        )?;
        if let Some(command_id) = &item.command_id {
            require(
                commands.contains(command_id),
                code,
                "undeclared status item command",
            )?;
        }
    }
    if !c.status_items.is_empty() {
        require(
            manifest
                .services
                .requires
                .iter()
                .any(|s| s.name == "se.host.status" && s.version == 1),
            code,
            "status items require status v1",
        )?;
    }
    require(
        manifest
            .services
            .provides
            .iter()
            .all(|s| !is_reserved_host_service(&s.name)),
        code,
        "host service provider names are reserved",
    )?;
    let mut web_entries = BTreeSet::new();
    for view in &c.views {
        if view.kind.unwrap_or_default() == ViewKind::Web {
            let entry = view.entry.as_deref().unwrap_or("");
            require(
                is_web_entry(entry) && web_entries.insert(entry.to_ascii_lowercase()),
                code,
                "invalid or duplicate web entry",
            )?;
            let mut allow = BTreeSet::new();
            for permission in view.allow.as_deref().unwrap_or(&[]) {
                require(allow.insert(*permission), code, "duplicate web permission")?;
                if let Some(service) = permission.required_service() {
                    require(
                        manifest
                            .services
                            .requires
                            .iter()
                            .any(|r| r.name == service && r.version == 1),
                        code,
                        "missing web host service",
                    )?;
                }
            }
        } else {
            require(
                view.entry.is_none() && view.allow.is_none(),
                code,
                "tree view cannot declare web fields",
            )?;
        }
    }
    require(
        c.processes.len() <= MAX_MANAGED_PROCESSES,
        code,
        "too many processes",
    )?;
    let mut process_ids = BTreeSet::new();
    if !c.processes.is_empty() {
        require(
            manifest
                .services
                .requires
                .iter()
                .any(|r| r.name == HOST_PROCESS_SERVICE && r.version == 1),
            code,
            "missing process host service",
        )?;
    }
    for process in &c.processes {
        require(
            process_ids.insert(&process.id),
            code,
            "duplicate process id",
        )?;
        validate_process_contribution(process)?;
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
    let value = crate::host::strict_contract_json(bytes, PLUGIN_INVALID_MANIFEST, MAX_JSON_BYTES)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn legacy() -> Value {
        json!({
            "schemaVersion": 1, "id": "example.capabilities", "name": "Capabilities",
            "version": "1.0.0", "abi": {"min": 1, "max": 1},
            "contributes": {"commands": [{"id": "example.summary", "title": "Summary"}],
                "configuration": {"title": "Settings", "properties": {}},
                "views": [{"id": "example.sidebar", "title": "Sidebar", "location": "sidebar", "icon": "terminal"}]},
            "services": {"provides": [], "requires": []}
        })
    }

    fn complete() -> Value {
        let mut value = legacy();
        value["services"]["requires"] = json!([
            {"name": "se.host.context", "version": 1},
            {"name": "se.host.events", "version": 1},
            {"name": "se.host.status", "version": 1}
        ]);
        value["contributes"]["eventSubscriptions"] = json!([
            {"id": "project", "topic": "project.activeChanged", "scope": "window"},
            {"id": "created", "topic": "terminal.created", "scope": "activeProject"},
            {"id": "closed", "topic": "terminal.closed", "scope": "window"},
            {"id": "selection", "topic": "terminal.activeChanged", "scope": "window"},
            {"id": "session", "topic": "agent.session.stateChanged", "scope": "activeProject"}
        ]);
        value["contributes"]["menus"] = json!([
            {"id": "menu", "location": "terminal.context", "commandId": "example.summary",
             "when": {"project.exists": true, "terminal.exists": true, "agentSession.exists": false,
                      "textInput.focused": false, "terminal.focused": true}}
        ]);
        value["contributes"]["keybindings"] = json!([
            {"id": "shortcut", "commandId": "example.summary", "key": "", "mac": "cmd+shift+alt+y", "when": {}}
        ]);
        value["contributes"]["statusItems"] = json!([
            {"id": "status", "label": "Activity", "location": "statusBar", "scope": "activeProject", "commandId": "example.summary"}
        ]);
        value
    }

    fn parse(value: &Value) -> Result<Manifest, PluginError> {
        parse_manifest(&serde_json::to_vec(value).unwrap())
    }

    fn invalid(value: &Value) {
        assert_eq!(parse(value).unwrap_err().code, PLUGIN_INVALID_MANIFEST);
    }

    #[test]
    fn host_manifest_arrays_default_empty_and_preserve_sidebar() {
        let manifest = parse(&legacy()).unwrap();
        assert!(manifest.contributes.event_subscriptions.is_empty());
        assert!(manifest.contributes.menus.is_empty());
        assert!(manifest.contributes.keybindings.is_empty());
        assert!(manifest.contributes.status_items.is_empty());
        assert_eq!(serde_json::to_value(manifest).unwrap(), legacy());
        let manifest = parse(&complete()).unwrap();
        assert_eq!(
            manifest.contributes.views[0].location,
            ViewLocation::Sidebar
        );
        assert_eq!(
            manifest.contributes.views[0].icon.as_deref(),
            Some("terminal")
        );
        assert_eq!(manifest.contributes.menus[0].order, 0);
        assert_eq!(manifest.contributes.status_items[0].order, 0);
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn host_manifest_null_unknown_and_duplicate_fields_are_rejected() {
        for array in ["eventSubscriptions", "menus", "keybindings", "statusItems"] {
            let mut value = complete();
            value["contributes"][array] = Value::Null;
            invalid(&value);
            let mut value = complete();
            value["contributes"][array][0]["unknown"] = json!(true);
            invalid(&value);
        }
        for (array, fields) in [
            ("menus", vec!["order", "when"]),
            ("keybindings", vec!["mac", "when"]),
            ("statusItems", vec!["order", "commandId"]),
        ] {
            for field in fields {
                let mut value = complete();
                value["contributes"][array][0][field] = Value::Null;
                invalid(&value);
            }
        }
        for array in ["menus", "keybindings"] {
            for when in [
                json!({"project.exists": null}),
                json!({"project.exists": "true"}),
                json!({"unknown": true}),
                json!({"and": []}),
                json!("project.exists"),
            ] {
                let mut value = complete();
                value["contributes"][array][0]["when"] = when;
                invalid(&value);
            }
        }
        let bytes = serde_json::to_string(&complete())
            .unwrap()
            .replace("\"id\":\"menu\"", "\"id\":\"menu\",\"id\":\"duplicate\"");
        assert!(parse_manifest(bytes.as_bytes()).is_err());
    }

    #[test]
    fn host_manifest_caps_and_local_ids_are_per_array() {
        for (array, limit) in [
            ("eventSubscriptions", 16),
            ("menus", 32),
            ("keybindings", 16),
            ("statusItems", 2),
        ] {
            let mut value = complete();
            let entry = value["contributes"][array][0].clone();
            value["contributes"][array] = Value::Array(
                (0..limit)
                    .map(|i| {
                        let mut entry = entry.clone();
                        entry["id"] = json!(format!("entry-{i}"));
                        entry
                    })
                    .collect(),
            );
            assert!(parse(&value).is_ok(), "{array} exact cap");
            value["contributes"][array]
                .as_array_mut()
                .unwrap()
                .push(entry.clone());
            invalid(&value);
            for id in ["", "contains space", "/path", "é", &"x".repeat(129)] {
                let mut value = complete();
                value["contributes"][array][0]["id"] = json!(id);
                invalid(&value);
            }
            let mut value = complete();
            value["contributes"][array] = json!([entry.clone(), entry]);
            invalid(&value);
        }
        let mut value = complete();
        for array in ["eventSubscriptions", "menus", "keybindings", "statusItems"] {
            value["contributes"][array][0]["id"] = json!("shared-id");
        }
        assert!(
            parse(&value).is_ok(),
            "same id across distinct contribution kinds is valid"
        );
    }

    #[test]
    fn host_manifest_finite_locations_scopes_topics_and_predicates() {
        for location in [
            "terminal.tab",
            "terminal.context",
            "project.context",
            "agentSession.context",
        ] {
            let mut value = complete();
            value["contributes"]["menus"][0]["location"] = json!(location);
            assert!(parse(&value).is_ok());
        }
        for (array, field, bad) in [
            ("eventSubscriptions", "topic", "terminal.output"),
            ("eventSubscriptions", "scope", "profile"),
            ("eventSubscriptions", "scope", "activeProject"),
            ("menus", "location", "global.menu"),
            ("statusItems", "location", "panel"),
            ("statusItems", "scope", "project"),
        ] {
            let mut value = complete();
            value["contributes"][array][0][field] = json!(bad);
            invalid(&value);
        }
        for key in ["textInput.focused", "terminal.focused"] {
            let mut value = complete();
            value["contributes"]["keybindings"][0]["when"] = json!({key: true});
            invalid(&value);
            value["contributes"]["keybindings"][0]["when"] = json!({key: false});
            assert!(parse(&value).is_ok());
        }
    }

    #[test]
    fn host_manifest_command_requirements_orders_and_unicode_labels() {
        for array in ["menus", "keybindings", "statusItems"] {
            let mut value = complete();
            value["contributes"][array][0]["commandId"] = json!("undeclared.command");
            invalid(&value);
        }
        for array in ["menus", "statusItems"] {
            for order in [-101, 101] {
                let mut value = complete();
                value["contributes"][array][0]["order"] = json!(order);
                invalid(&value);
            }
            for order in [-100, 100] {
                let mut value = complete();
                value["contributes"][array][0]["order"] = json!(order);
                assert!(parse(&value).is_ok());
            }
        }
        for label in ["".to_owned(), "界".repeat(129)] {
            let mut value = complete();
            value["contributes"]["statusItems"][0]["label"] = json!(label);
            invalid(&value);
        }
        let mut value = complete();
        value["contributes"]["statusItems"][0]["label"] = json!("🦀".repeat(128));
        value["contributes"]["statusItems"][0]
            .as_object_mut()
            .unwrap()
            .remove("commandId");
        assert!(parse(&value).is_ok());
        for index in 0..3 {
            let mut value = complete();
            value["services"]["requires"]
                .as_array_mut()
                .unwrap()
                .remove(index);
            invalid(&value);
            let mut value = complete();
            value["services"]["requires"][index]["version"] = json!(2);
            invalid(&value);
        }
    }

    #[test]
    fn host_manifest_key_grammar_is_canonical_and_bounded() {
        for key in [
            "",
            "ctrl+a",
            "cmd+9",
            "ctrl+cmd+shift+alt+f12",
            "cmd+space",
            "ctrl+shift+alt+arrowleft",
        ] {
            assert!(is_manifest_keybinding(key), "{key}");
        }
        for key in [
            "a",
            "ctrl",
            "ctrl+",
            "Ctrl+a",
            "ctrl+A",
            "shift+a",
            "ctrl+ctrl+a",
            "cmd+ctrl+a",
            "ctrl+alt+shift+a",
            "ctrl+a ctrl+b",
            "ctrl+f0",
            "cmd+f13",
            "ctrl+é",
            "ctrl+enter ",
            "ctrl++",
        ] {
            assert!(!is_manifest_keybinding(key), "{key}");
            for field in ["key", "mac"] {
                let mut value = complete();
                value["contributes"]["keybindings"][0][field] = json!(key);
                invalid(&value);
            }
        }
        for key in [
            "enter",
            "space",
            "tab",
            "escape",
            "backspace",
            "delete",
            "home",
            "end",
            "pageup",
            "pagedown",
            "arrowup",
            "arrowdown",
            "arrowleft",
            "arrowright",
        ] {
            assert!(is_manifest_keybinding(&format!("ctrl+{key}")));
        }
        for i in 1..=12 {
            assert!(is_manifest_keybinding(&format!("cmd+f{i}")));
        }
    }

    #[test]
    fn host_catalog_reserves_future_providers_but_allows_unresolved_requirements() {
        for name in [
            "se.host.context",
            "se.host.events",
            "se.host.storage",
            "se.host.feedback",
            "se.host.status",
        ] {
            assert_eq!(
                known_host_service(name, 1),
                Some(HostServiceAvailability::Available)
            );
            assert_eq!(known_host_service(name, 2), None);
        }
        assert_eq!(
            known_host_service("se.host.secrets", 1),
            Some(HostServiceAvailability::Unavailable)
        );
        assert_eq!(known_host_service("se.host.future", 1), None);
        for name in ["se.host.secrets", "se.host.future", "se.host.context"] {
            let mut value = legacy();
            value["services"]["provides"] = json!([{"name": name, "version": 1}]);
            invalid(&value);
            value["services"]["provides"] = json!([]);
            value["services"]["requires"] = json!([{"name": name, "version": 2}]);
            assert!(parse(&value).is_ok());
        }
        assert!(!is_reserved_host_service("se.hosted.context"));
    }
}
