use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

macro_rules! string_enum {
    ($name:ident, $default:ident, $($variant:ident => $wire:literal),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name { $(#[serde(rename = $wire)] $variant),+ }
        impl Default for $name { fn default() -> Self { Self::$default } }
    };
}
string_enum!(Gap, Sm, None => "none", Xs => "xs", Sm => "sm", Md => "md", Lg => "lg");
string_enum!(Align, Stretch, Start => "start", Center => "center", End => "end", Stretch => "stretch");
string_enum!(Tone, Default, Default => "default", Muted => "muted", Danger => "danger");
string_enum!(ButtonVariant, Default, Default => "default", Secondary => "secondary", Outline => "outline", Destructive => "destructive");
string_enum!(Orientation, Horizontal, Horizontal => "horizontal", Vertical => "vertical");
fn true_value() -> bool {
    true
}
fn heading_level() -> u8 {
    2
}
fn input_max_length() -> u32 {
    4096
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct LayoutProps {
    #[serde(default)]
    pub gap: Gap,
    #[serde(default)]
    pub align: Align,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextProps {
    pub text: String,
    #[serde(default)]
    pub tone: Tone,
    #[serde(default = "true_value")]
    pub wrap: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeadingProps {
    pub text: String,
    #[serde(default = "heading_level")]
    pub level: u8,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ButtonProps {
    pub label: String,
    pub action_id: String,
    #[serde(default)]
    pub variant: ButtonVariant,
    #[serde(default)]
    pub disabled: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InputProps {
    pub label: String,
    pub value: String,
    pub action_id: String,
    #[serde(default)]
    pub placeholder: String,
    #[serde(default = "input_max_length")]
    pub max_length: u32,
    #[serde(default)]
    pub disabled: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToggleProps {
    pub label: String,
    pub checked: bool,
    pub action_id: String,
    #[serde(default)]
    pub disabled: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectProps {
    pub label: String,
    pub value: String,
    pub action_id: String,
    pub options: Vec<SelectOption>,
    #[serde(default)]
    pub disabled: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListProps {
    pub items: Vec<ListItem>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListItem {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub description: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct DividerProps {
    #[serde(default)]
    pub orientation: Orientation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum UiNode {
    Column {
        id: String,
        props: LayoutProps,
        children: Vec<UiNode>,
    },
    Row {
        id: String,
        props: LayoutProps,
        children: Vec<UiNode>,
    },
    Text {
        id: String,
        props: TextProps,
    },
    Heading {
        id: String,
        props: HeadingProps,
    },
    Button {
        id: String,
        props: ButtonProps,
    },
    Input {
        id: String,
        props: InputProps,
    },
    Toggle {
        id: String,
        props: ToggleProps,
    },
    Select {
        id: String,
        props: SelectProps,
    },
    List {
        id: String,
        props: ListProps,
    },
    Divider {
        id: String,
        props: DividerProps,
    },
}
impl UiNode {
    pub fn id(&self) -> &str {
        match self {
            Self::Column { id, .. }
            | Self::Row { id, .. }
            | Self::Text { id, .. }
            | Self::Heading { id, .. }
            | Self::Button { id, .. }
            | Self::Input { id, .. }
            | Self::Toggle { id, .. }
            | Self::Select { id, .. }
            | Self::List { id, .. }
            | Self::Divider { id, .. } => id,
        }
    }
    pub fn children(&self) -> &[UiNode] {
        match self {
            Self::Column { children, .. } | Self::Row { children, .. } => children,
            _ => &[],
        }
    }
    pub fn find(&self, id: &str) -> Option<&UiNode> {
        let mut stack = vec![self];
        while let Some(node) = stack.pop() {
            if node.id() == id {
                return Some(node);
            }
            stack.extend(node.children());
        }
        None
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiTree {
    pub schema_version: u32,
    pub view_id: String,
    pub root: UiNode,
}

fn interaction(label: &str, action_id: &str) -> Result<(), PluginError> {
    text(label, 1, 128, PLUGIN_INVALID_UI)?;
    require(
        is_local_id(action_id),
        PLUGIN_INVALID_UI,
        "invalid actionId",
    )
}
pub fn validate_ui_tree(tree: &UiTree) -> Result<(), PluginError> {
    let code = PLUGIN_INVALID_UI;
    version(tree.schema_version, code)?;
    require(is_local_id(&tree.view_id), code, "invalid viewId")?;
    let mut seen = BTreeSet::new();
    let mut stack = vec![(&tree.root, 1usize)];
    while let Some((node, depth)) = stack.pop() {
        require(
            depth <= MAX_UI_DEPTH && seen.len() < MAX_UI_NODES,
            code,
            "UI depth or node count exceeds limit",
        )?;
        require(
            is_local_id(node.id()) && seen.insert(node.id()),
            code,
            "invalid or duplicate node id",
        )?;
        match node {
            UiNode::Column { children, .. } | UiNode::Row { children, .. } => {
                stack.extend(children.iter().map(|n| (n, depth + 1)))
            }
            UiNode::Text { props, .. } => text(&props.text, 0, 8192, code)?,
            UiNode::Heading { props, .. } => {
                text(&props.text, 0, 8192, code)?;
                require(
                    (1..=3).contains(&props.level),
                    code,
                    "invalid heading level",
                )?;
            }
            UiNode::Button { props, .. } => interaction(&props.label, &props.action_id)?,
            UiNode::Input { props, .. } => {
                interaction(&props.label, &props.action_id)?;
                require(
                    (1..=4096).contains(&props.max_length),
                    code,
                    "invalid input maxLength",
                )?;
                text(&props.value, 0, props.max_length as usize, code)?;
                text(&props.placeholder, 0, 8192, code)?;
            }
            UiNode::Toggle { props, .. } => interaction(&props.label, &props.action_id)?,
            UiNode::Select { props, .. } => {
                interaction(&props.label, &props.action_id)?;
                require(props.options.len() <= 64, code, "too many select options")?;
                let mut options = BTreeSet::new();
                for option in &props.options {
                    text(&option.value, 1, 8192, code)?;
                    text(&option.label, 0, 8192, code)?;
                    require(
                        options.insert(&option.value),
                        code,
                        "duplicate select value",
                    )?;
                }
                require(
                    options.contains(&props.value),
                    code,
                    "select value missing from options",
                )?;
            }
            UiNode::List { props, .. } => {
                require(props.items.len() <= 256, code, "too many list items")?;
                let mut items = BTreeSet::new();
                for item in &props.items {
                    text(&item.id, 0, 8192, code)?;
                    require(items.insert(&item.id), code, "duplicate list item id")?;
                    text(&item.text, 0, 8192, code)?;
                    text(&item.description, 0, 8192, code)?;
                }
            }
            UiNode::Divider { .. } => {}
        }
    }
    json_size(tree, code)
}
pub fn parse_ui_tree(bytes: &[u8]) -> Result<UiTree, PluginError> {
    let value = json(bytes, PLUGIN_INVALID_UI)?;
    validate_ui_tree(&value)?;
    Ok(value)
}
impl UiTree {
    pub fn validate(&self) -> Result<(), PluginError> {
        validate_ui_tree(self)
    }
    pub fn from_json(bytes: &[u8]) -> Result<Self, PluginError> {
        parse_ui_tree(bytes)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionKind {
    Click,
    Change,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ActionValue {
    Null,
    String(String),
    Boolean(bool),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActionEvent {
    pub schema_version: u32,
    pub event_id: String,
    pub plugin_id: String,
    pub generation: String,
    pub view_id: String,
    pub view_revision: u64,
    pub node_id: String,
    pub action_id: String,
    pub kind: ActionKind,
    pub value: ActionValue,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewSnapshot {
    pub plugin_id: String,
    pub generation: String,
    pub revision: u64,
    pub config_revision: u64,
    pub tree: UiTree,
}
pub fn validate_action_event(event: &ActionEvent) -> Result<(), PluginError> {
    let code = PLUGIN_INVALID_ACTION;
    version(event.schema_version, code)?;
    require(
        matches!(
            (event.kind, &event.value),
            (ActionKind::Click, ActionValue::Null)
                | (
                    ActionKind::Change,
                    ActionValue::String(_) | ActionValue::Boolean(_)
                )
        ),
        code,
        "action value does not match kind",
    )?;
    require(
        is_uuid(&event.event_id) && is_uuid(&event.generation) && is_plugin_id(&event.plugin_id),
        code,
        "invalid action identity",
    )?;
    require(
        is_local_id(&event.view_id) && is_local_id(&event.node_id) && is_local_id(&event.action_id),
        code,
        "invalid action local id",
    )?;
    revision(event.view_revision, code)?;
    if let ActionValue::String(s) = &event.value {
        text(s, 0, 8192, code)?;
    }
    json_size(event, code)
}
pub fn parse_action_event(bytes: &[u8]) -> Result<ActionEvent, PluginError> {
    let value = json(bytes, PLUGIN_INVALID_ACTION)?;
    validate_action_event(&value)?;
    Ok(value)
}
pub fn validate_view_snapshot(snapshot: &ViewSnapshot) -> Result<(), PluginError> {
    require(
        is_plugin_id(&snapshot.plugin_id) && is_uuid(&snapshot.generation),
        PLUGIN_INVALID_UI,
        "invalid snapshot identity",
    )?;
    revision(snapshot.revision, PLUGIN_INVALID_UI)?;
    revision(snapshot.config_revision, PLUGIN_INVALID_UI)?;
    validate_ui_tree(&snapshot.tree)
}
pub fn validate_action_against_snapshot(
    event: &ActionEvent,
    snapshot: &ViewSnapshot,
) -> Result<(), PluginError> {
    validate_action_event(event)?;
    validate_view_snapshot(snapshot)?;
    require(
        event.plugin_id == snapshot.plugin_id,
        PLUGIN_INVALID_ACTION,
        "action plugin does not match snapshot",
    )?;
    require(
        event.generation == snapshot.generation,
        PLUGIN_STALE_GENERATION,
        "action generation is stale",
    )?;
    require(
        event.view_id == snapshot.tree.view_id && event.view_revision == snapshot.revision,
        PLUGIN_STALE_VIEW,
        "action view is stale",
    )?;
    let node = snapshot
        .tree
        .root
        .find(&event.node_id)
        .ok_or_else(|| PluginError::new(PLUGIN_INVALID_ACTION, "action node not found"))?;
    let valid = match (node, event.kind, &event.value) {
        (UiNode::Button { props, .. }, ActionKind::Click, ActionValue::Null) => {
            !props.disabled && event.action_id == props.action_id
        }
        (UiNode::Input { props, .. }, ActionKind::Change, ActionValue::String(s)) => {
            !props.disabled
                && event.action_id == props.action_id
                && chars(s) <= props.max_length as usize
        }
        (UiNode::Toggle { props, .. }, ActionKind::Change, ActionValue::Boolean(_)) => {
            !props.disabled && event.action_id == props.action_id
        }
        (UiNode::Select { props, .. }, ActionKind::Change, ActionValue::String(s)) => {
            !props.disabled
                && event.action_id == props.action_id
                && props.options.iter().any(|o| &o.value == s)
        }
        _ => false,
    };
    require(
        valid,
        PLUGIN_INVALID_ACTION,
        "action does not match enabled control",
    )
}
