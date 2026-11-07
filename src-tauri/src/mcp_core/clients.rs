//! Connect local AI clients to the selected MCP gateway source by writing its
//! fixed `se-mcp` or `se-mcp-aggregator` entry into their MCP configuration.
//!
//! Each client keeps its file format: JSON files get an entry under their MCP
//! key with every other key left in place and in order; Codex's TOML is edited
//! with `toml_edit` so comments and layout survive. Before the first change to
//! a file its original is copied next to it once, so the user can always go
//! back to what they had.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use se_mcp_bridge::Source;
use serde::Serialize;
use serde_json::{json, Map, Value};

// Both sources edit the same client files. Hold this process-wide lock for the
// entire read/modify/backup/atomic-write transaction, not just the final write.
static MUTATION_LOCK: Mutex<()> = Mutex::new(());
const BACKUP_SUFFIX: &str = "before-se-mcp.bak";
/// Codex stops waiting for a tool after 60s by default; gateway calls may run
/// as long as the gateway's own limit.
const CODEX_TOOL_TIMEOUT_SEC: i64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    /// JSON object; servers live under `key`. `typed` adds `"type": "stdio"`.
    Json { key: &'static str, typed: bool },
    /// Codex `config.toml`, `[mcp_servers.<name>]`.
    CodexToml,
}

struct ClientSpec {
    id: &'static str,
    name: &'static str,
    format: Format,
    /// Config file, relative to the home directory, per platform.
    config: fn(&Path) -> PathBuf,
    /// Present when the client is installed.
    marker: fn(&Path) -> PathBuf,
}

fn app_support(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library").join("Application Support")
    } else if cfg!(windows) {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Roaming"))
    } else {
        home.join(".config")
    }
}

const CLIENTS: &[ClientSpec] = &[
    ClientSpec {
        id: "claude-code",
        name: "Claude Code",
        format: Format::Json {
            key: "mcpServers",
            typed: true,
        },
        config: |home| home.join(".claude.json"),
        marker: |home| home.join(".claude"),
    },
    ClientSpec {
        id: "codex",
        name: "Codex",
        format: Format::CodexToml,
        config: |home| home.join(".codex").join("config.toml"),
        marker: |home| home.join(".codex"),
    },
    ClientSpec {
        id: "claude-desktop",
        name: "Claude Desktop",
        format: Format::Json {
            key: "mcpServers",
            typed: false,
        },
        config: |home| {
            app_support(home)
                .join("Claude")
                .join("claude_desktop_config.json")
        },
        marker: |home| app_support(home).join("Claude"),
    },
    ClientSpec {
        id: "cursor",
        name: "Cursor",
        format: Format::Json {
            key: "mcpServers",
            typed: false,
        },
        config: |home| home.join(".cursor").join("mcp.json"),
        marker: |home| home.join(".cursor"),
    },
    ClientSpec {
        id: "gemini",
        name: "Gemini CLI",
        format: Format::Json {
            key: "mcpServers",
            typed: false,
        },
        config: |home| home.join(".gemini").join("settings.json"),
        marker: |home| home.join(".gemini"),
    },
    ClientSpec {
        id: "windsurf",
        name: "Windsurf",
        format: Format::Json {
            key: "mcpServers",
            typed: false,
        },
        config: |home| {
            home.join(".codeium")
                .join("windsurf")
                .join("mcp_config.json")
        },
        marker: |home| home.join(".codeium").join("windsurf"),
    },
    ClientSpec {
        id: "vscode",
        name: "VS Code",
        format: Format::Json {
            key: "servers",
            typed: true,
        },
        config: |home| app_support(home).join("Code").join("User").join("mcp.json"),
        marker: |home| app_support(home).join("Code").join("User"),
    },
];

/// The stdio command every client is configured to launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeCommand {
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedClient {
    pub id: String,
    pub name: String,
    pub config_path: String,
    pub installed: bool,
    /// An `se-mcp` entry exists.
    pub synced: bool,
    /// The entry runs exactly the expected command.
    pub up_to_date: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncOutcome {
    pub config_path: String,
    /// Where the untouched original was saved, when this call saved it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_path: Option<String>,
}

fn spec(id: &str) -> Result<&'static ClientSpec, String> {
    CLIENTS
        .iter()
        .find(|client| client.id == id)
        .ok_or_else(|| format!("unknown client `{id}`"))
}

pub fn detect(home: &Path, source: Source, bridge: &BridgeCommand) -> Vec<DetectedClient> {
    CLIENTS
        .iter()
        .map(|client| {
            let path = (client.config)(home);
            let installed = (client.marker)(home).exists() || path.exists();
            let (synced, up_to_date, error) = match read_entry(client.format, &path, source) {
                Ok(Some((command, args))) => {
                    (true, command == bridge.command && args == bridge.args, None)
                }
                Ok(None) => (false, false, None),
                Err(error) => (false, false, Some(error)),
            };
            DetectedClient {
                id: client.id.to_owned(),
                name: client.name.to_owned(),
                config_path: path.display().to_string(),
                installed,
                synced,
                up_to_date,
                error,
            }
        })
        .collect()
}

/// Add or refresh only the selected source's fixed entry in one client.
pub fn sync(
    home: &Path,
    id: &str,
    source: Source,
    bridge: &BridgeCommand,
) -> Result<SyncOutcome, String> {
    let client = spec(id)?;
    let _mutation = MUTATION_LOCK
        .lock()
        .map_err(|_| "MCP_CLIENT_MUTATION_LOCK_POISONED".to_owned())?;
    let path = (client.config)(home);
    let original = read_optional(&path)?;
    let updated = match client.format {
        Format::Json { key, typed } => {
            let mut root = parse_json_object(original.as_deref())?;
            let servers = root
                .entry(key)
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .ok_or_else(|| format!("`{key}` in {} is not an object", path.display()))?;
            let mut entry = Map::new();
            if typed {
                entry.insert("type".into(), json!("stdio"));
            }
            entry.insert("command".into(), json!(bridge.command));
            entry.insert("args".into(), json!(bridge.args));
            servers.insert(source.client_key().into(), Value::Object(entry));
            json_text(&root)?
        }
        Format::CodexToml => {
            let mut document = parse_toml(original.as_deref())?;
            let servers = document
                .entry("mcp_servers")
                .or_insert_with(|| {
                    let mut table = toml_edit::Table::new();
                    table.set_implicit(true);
                    toml_edit::Item::Table(table)
                })
                .as_table_mut()
                .ok_or("`mcp_servers` in config.toml is not a table")?;
            let mut entry = toml_edit::Table::new();
            entry.insert("command", toml_edit::value(bridge.command.as_str()));
            let mut args = toml_edit::Array::new();
            for argument in &bridge.args {
                args.push(argument.as_str());
            }
            entry.insert("args", toml_edit::value(args));
            entry.insert("tool_timeout_sec", toml_edit::value(CODEX_TOOL_TIMEOUT_SEC));
            servers.insert(source.client_key(), toml_edit::Item::Table(entry));
            document.to_string()
        }
    };
    let backup_path = backup_once(&path, original.as_deref())?;
    write_file(&path, &updated)?;
    Ok(SyncOutcome {
        config_path: path.display().to_string(),
        backup_path,
    })
}

/// Remove only the selected source's fixed entry, leaving everything else.
pub fn unsync(home: &Path, id: &str, source: Source) -> Result<SyncOutcome, String> {
    let client = spec(id)?;
    let _mutation = MUTATION_LOCK
        .lock()
        .map_err(|_| "MCP_CLIENT_MUTATION_LOCK_POISONED".to_owned())?;
    let path = (client.config)(home);
    let mut outcome = SyncOutcome {
        config_path: path.display().to_string(),
        backup_path: None,
    };
    let Some(original) = read_optional(&path)? else {
        return Ok(outcome);
    };
    let updated = match client.format {
        Format::Json { key, .. } => {
            let mut root = parse_json_object(Some(&original))?;
            let removed = root
                .get_mut(key)
                .and_then(Value::as_object_mut)
                .and_then(|servers| servers.shift_remove(source.client_key()))
                .is_some();
            if !removed {
                return Ok(outcome);
            }
            json_text(&root)?
        }
        Format::CodexToml => {
            let mut document = parse_toml(Some(&original))?;
            let removed = document
                .get_mut("mcp_servers")
                .and_then(toml_edit::Item::as_table_like_mut)
                .and_then(|servers| servers.remove(source.client_key()))
                .is_some();
            if !removed {
                return Ok(outcome);
            }
            document.to_string()
        }
    };
    outcome.backup_path = backup_once(&path, Some(&original))?;
    write_file(&path, &updated)?;
    Ok(outcome)
}

fn read_entry(
    format: Format,
    path: &Path,
    source: Source,
) -> Result<Option<(String, Vec<String>)>, String> {
    let Some(text) = read_optional(path)? else {
        return Ok(None);
    };
    let entry = match format {
        Format::Json { key, .. } => {
            let root = parse_json_object(Some(&text))?;
            let Some(entry) = root
                .get(key)
                .and_then(|servers| servers.get(source.client_key()))
            else {
                return Ok(None);
            };
            let command = entry["command"].as_str().unwrap_or_default().to_owned();
            let args = entry["args"]
                .as_array()
                .map(|args| {
                    args.iter()
                        .filter_map(|arg| arg.as_str().map(ToOwned::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            (command, args)
        }
        Format::CodexToml => {
            let document = parse_toml(Some(&text))?;
            let Some(entry) = document
                .get("mcp_servers")
                .and_then(|servers| servers.get(source.client_key()))
            else {
                return Ok(None);
            };
            let command = entry
                .get("command")
                .and_then(toml_edit::Item::as_str)
                .unwrap_or_default()
                .to_owned();
            let args = entry
                .get("args")
                .and_then(toml_edit::Item::as_array)
                .map(|args| {
                    args.iter()
                        .filter_map(|arg| arg.as_str().map(ToOwned::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            (command, args)
        }
    };
    Ok(Some(entry))
}

fn read_optional(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

fn parse_json_object(text: Option<&str>) -> Result<Map<String, Value>, String> {
    let text = text.unwrap_or_default();
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(object)) => Ok(object),
        Ok(_) => Err("the configuration file is not a JSON object".into()),
        Err(error) => Err(format!(
            "the configuration file is not plain JSON ({error}); edit it by hand"
        )),
    }
}

fn parse_toml(text: Option<&str>) -> Result<toml_edit::DocumentMut, String> {
    text.unwrap_or_default()
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("config.toml is not valid TOML: {error}"))
}

fn json_text(root: &Map<String, Value>) -> Result<String, String> {
    serde_json::to_string_pretty(root)
        .map(|mut text| {
            text.push('\n');
            text
        })
        .map_err(|error| error.to_string())
}

/// Keep the user's original once, before the first change we make.
fn backup_once(path: &Path, original: Option<&str>) -> Result<Option<String>, String> {
    let Some(original) = original else {
        return Ok(None);
    };
    let backup = path.with_extension(match path.extension().and_then(|value| value.to_str()) {
        Some(extension) => format!("{extension}.{BACKUP_SUFFIX}"),
        None => BACKUP_SUFFIX.to_owned(),
    });
    if backup.exists() {
        return Ok(None);
    }
    fs::write(&backup, original)
        .map_err(|error| format!("cannot back up {}: {error}", path.display()))?;
    Ok(Some(backup.display().to_string()))
}

fn write_file(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    crate::acp::atomic_file::replace(path, text.as_bytes())
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge() -> BridgeCommand {
        source_bridge(Source::Internal)
    }

    fn source_bridge(source: Source) -> BridgeCommand {
        BridgeCommand {
            command: "/Applications/Se Manager.app/Contents/MacOS/se-mcp".into(),
            args: vec![
                "--source".into(),
                source.as_str().into(),
                "--mode".into(),
                "entry".into(),
            ],
        }
    }

    #[test]
    fn json_clients_keep_every_other_key_in_place() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".claude.json");
        let original = r#"{"numStartups":3,"mcpServers":{"other":{"command":"x"}},"projects":{}}"#;
        fs::write(&path, original).unwrap();

        let outcome = sync(home.path(), "claude-code", Source::Internal, &bridge()).unwrap();
        let written: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let keys = written.as_object().unwrap().keys().collect::<Vec<_>>();
        assert_eq!(keys, ["numStartups", "mcpServers", "projects"]);
        assert_eq!(written["mcpServers"]["other"]["command"], "x");
        assert_eq!(written["mcpServers"]["se-mcp"]["type"], "stdio");
        assert_eq!(
            written["mcpServers"]["se-mcp"]["args"],
            json!(bridge().args)
        );
        let backup = outcome.backup_path.unwrap();
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);

        // A second sync never replaces the original backup.
        let again = sync(home.path(), "claude-code", Source::Internal, &bridge()).unwrap();
        assert!(again.backup_path.is_none());
        assert_eq!(fs::read_to_string(&backup).unwrap(), original);

        unsync(home.path(), "claude-code", Source::Internal).unwrap();
        let removed: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(removed["mcpServers"].get("se-mcp").is_none());
        assert_eq!(removed["mcpServers"]["other"]["command"], "x");
    }

    #[test]
    fn codex_toml_keeps_comments_and_other_servers() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(".codex");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(
            &path,
            "# my settings\nmodel = \"gpt-5\"\n\n[mcp_servers.context7]\ncommand = \"npx\" # keep\n",
        )
        .unwrap();

        sync(home.path(), "codex", Source::Internal, &bridge()).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# my settings\nmodel = \"gpt-5\""));
        assert!(text.contains("command = \"npx\" # keep"));
        let parsed: toml::Value = toml::from_str(&text).unwrap();
        let entry = &parsed["mcp_servers"]["se-mcp"];
        assert_eq!(entry["command"].as_str(), Some(bridge().command.as_str()));
        assert_eq!(entry["tool_timeout_sec"].as_integer(), Some(300));

        let detected = detect(home.path(), Source::Internal, &bridge());
        let codex = detected.iter().find(|client| client.id == "codex").unwrap();
        assert!(codex.installed && codex.synced && codex.up_to_date);

        unsync(home.path(), "codex", Source::Internal).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("se-mcp"));
        assert!(text.contains("[mcp_servers.context7]"));
    }

    #[test]
    fn detect_reports_a_stale_entry_and_an_unreadable_file() {
        let home = tempfile::tempdir().unwrap();
        sync(home.path(), "cursor", Source::Internal, &bridge()).unwrap();
        let grouped = BridgeCommand {
            command: bridge().command,
            args: Vec::new(),
        };
        let detected = detect(home.path(), Source::Internal, &grouped);
        let cursor = detected
            .iter()
            .find(|client| client.id == "cursor")
            .unwrap();
        assert!(cursor.synced);
        assert!(!cursor.up_to_date);
        assert!(detected
            .iter()
            .find(|client| client.id == "windsurf")
            .is_some_and(|client| !client.installed && !client.synced));

        let gemini = home.path().join(".gemini");
        fs::create_dir_all(&gemini).unwrap();
        fs::write(gemini.join("settings.json"), "{ // comment\n}").unwrap();
        let detected = detect(home.path(), Source::Internal, &grouped);
        let gemini = detected
            .iter()
            .find(|client| client.id == "gemini")
            .unwrap();
        assert!(gemini.error.as_deref().unwrap().contains("not plain JSON"));
        assert!(sync(home.path(), "gemini", Source::Internal, &grouped).is_err());
    }

    #[test]
    fn json_source_operations_preserve_the_other_source_and_key_order() {
        for client in CLIENTS {
            let Format::Json { key, .. } = client.format else {
                continue;
            };
            let home = tempfile::tempdir().unwrap();
            let path = (client.config)(home.path());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let original = format!(
                r#"{{"before":1,"{key}":{{"third":{{"command":"third","env":{{"KEEP":"yes"}}}},"se-mcp":{{"command":"old-bridge","args":["--mode","entry"],"env":{{"KEEP":"internal"}}}}}},"after":2}}"#
            );
            fs::write(&path, &original).unwrap();
            let initial: Value = serde_json::from_str(&original).unwrap();
            let aggregator = source_bridge(Source::Aggregator);
            let outcome = sync(home.path(), client.id, Source::Aggregator, &aggregator).unwrap();
            let backup = outcome.backup_path.unwrap();
            let written: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(written[key]["se-mcp"], initial[key]["se-mcp"]);
            assert_eq!(written[key]["third"], initial[key]["third"]);
            assert_eq!(
                written
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                ["before", key, "after"]
            );
            assert_eq!(
                written[key]
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                ["third", "se-mcp", "se-mcp-aggregator"]
            );
            let legacy = detect(home.path(), Source::Internal, &bridge());
            let legacy = legacy.iter().find(|item| item.id == client.id).unwrap();
            assert!(legacy.synced && !legacy.up_to_date);
            let current = detect(home.path(), Source::Aggregator, &aggregator);
            assert!(
                current
                    .iter()
                    .find(|item| item.id == client.id)
                    .unwrap()
                    .up_to_date
            );

            sync(home.path(), client.id, Source::Internal, &bridge()).unwrap();
            unsync(home.path(), client.id, Source::Internal).unwrap();
            let removed: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert!(removed[key].get("se-mcp").is_none());
            assert_eq!(
                removed[key]["se-mcp-aggregator"],
                written[key]["se-mcp-aggregator"]
            );
            assert_eq!(removed[key]["third"], initial[key]["third"]);
            unsync(home.path(), client.id, Source::Aggregator).unwrap();
            assert_eq!(fs::read_to_string(backup).unwrap(), original);
        }
    }

    #[test]
    fn codex_source_operations_preserve_sibling_table_and_comments() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".codex/config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = "# user settings\nmodel = \"gpt-5\" # model comment\n\n[mcp_servers.third]\ncommand = \"third\" # keep third\n\n[mcp_servers.se-mcp]\ncommand = \"old-bridge\" # keep internal\nargs = [\"--mode\", \"entry\"]\ncustom = true\n";
        fs::write(&path, original).unwrap();
        let outcome = sync(
            home.path(),
            "codex",
            Source::Aggregator,
            &source_bridge(Source::Aggregator),
        )
        .unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(original));
        let parsed: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(
            parsed["mcp_servers"]["se-mcp-aggregator"]["tool_timeout_sec"].as_integer(),
            Some(300)
        );
        let before: toml::Value = toml::from_str(original).unwrap();
        assert_eq!(
            parsed["mcp_servers"]["se-mcp"],
            before["mcp_servers"]["se-mcp"]
        );
        unsync(home.path(), "codex", Source::Aggregator).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert_eq!(
            fs::read_to_string(outcome.backup_path.unwrap()).unwrap(),
            original
        );
    }

    #[test]
    fn unsync_first_takes_one_backup_and_only_removes_selected_source() {
        for client in CLIENTS {
            let home = tempfile::tempdir().unwrap();
            let path = (client.config)(home.path());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let original = match client.format {
                Format::Json { key, .. } => format!(
                    r#"{{"{key}":{{"third":{{"command":"third"}},"se-mcp":{{"command":"old"}},"se-mcp-aggregator":{{"command":"aggregator"}}}}}}"#
                ),
                Format::CodexToml => "# keep\n[mcp_servers.third]\ncommand = \"third\"\n[mcp_servers.se-mcp]\ncommand = \"old\"\n[mcp_servers.se-mcp-aggregator]\ncommand = \"aggregator\"\n".to_owned(),
            };
            fs::write(&path, &original).unwrap();
            let outcome = unsync(home.path(), client.id, Source::Internal).unwrap();
            let backup = outcome.backup_path.unwrap();
            assert!(read_entry(client.format, &path, Source::Internal)
                .unwrap()
                .is_none());
            assert_eq!(
                read_entry(client.format, &path, Source::Aggregator)
                    .unwrap()
                    .unwrap()
                    .0,
                "aggregator"
            );
            let after = fs::read_to_string(&path).unwrap();
            assert!(unsync(home.path(), client.id, Source::Internal)
                .unwrap()
                .backup_path
                .is_none());
            assert_eq!(fs::read_to_string(&path).unwrap(), after);
            unsync(home.path(), client.id, Source::Aggregator).unwrap();
            assert_eq!(fs::read_to_string(backup).unwrap(), original);
        }
    }

    #[test]
    fn concurrent_two_source_sync_keeps_both_entries_and_original_backup() {
        use std::{sync::Barrier, thread};

        for client in CLIENTS {
            for _ in 0..8 {
                let home = tempfile::tempdir().unwrap();
                let path = (client.config)(home.path());
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                let original = match client.format {
                    Format::Json { key, .. } => format!(
                        r#"{{"before":1,"{key}":{{"third":{{"command":"third"}}}},"after":2}}"#
                    ),
                    Format::CodexToml => "# keep\nmodel = \"gpt-5\"\n[mcp_servers.third]\ncommand = \"third\" # keep third\n".to_owned(),
                };
                fs::write(&path, &original).unwrap();
                let start = Barrier::new(2);
                let outcomes = thread::scope(|scope| {
                    let handles = [Source::Internal, Source::Aggregator].map(|source| {
                        let start = &start;
                        let home = home.path();
                        scope.spawn(move || {
                            start.wait();
                            sync(home, client.id, source, &source_bridge(source)).unwrap()
                        })
                    });
                    handles.map(|handle| handle.join().unwrap())
                });
                let backups = outcomes
                    .iter()
                    .filter_map(|outcome| outcome.backup_path.as_ref())
                    .collect::<Vec<_>>();
                assert_eq!(backups.len(), 1);
                assert_eq!(fs::read_to_string(backups[0]).unwrap(), original);
                for source in [Source::Internal, Source::Aggregator] {
                    let entry = read_entry(client.format, &path, source).unwrap().unwrap();
                    let expected = source_bridge(source);
                    assert_eq!(entry, (expected.command, expected.args));
                }
                let text = fs::read_to_string(&path).unwrap();
                match client.format {
                    Format::Json { key, .. } => {
                        let parsed: Value = serde_json::from_str(&text).unwrap();
                        assert_eq!(parsed[key]["third"], json!({"command":"third"}));
                        assert_eq!(
                            parsed
                                .as_object()
                                .unwrap()
                                .keys()
                                .map(String::as_str)
                                .collect::<Vec<_>>(),
                            ["before", key, "after"]
                        );
                    }
                    Format::CodexToml => assert!(text.starts_with(&original)),
                }
            }
        }
    }

    #[test]
    fn missing_and_invalid_configs_are_not_mutated_by_unsync() {
        for client in CLIENTS {
            let home = tempfile::tempdir().unwrap();
            let path = (client.config)(home.path());
            for source in [Source::Internal, Source::Aggregator] {
                assert!(unsync(home.path(), client.id, source)
                    .unwrap()
                    .backup_path
                    .is_none());
                assert!(!path.exists());
            }
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let invalid = match client.format {
                Format::Json { .. } => "{not json}",
                Format::CodexToml => "[not toml",
            };
            fs::write(&path, invalid).unwrap();
            for source in [Source::Internal, Source::Aggregator] {
                assert!(sync(home.path(), client.id, source, &source_bridge(source)).is_err());
                assert!(unsync(home.path(), client.id, source).is_err());
            }
            assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
            assert!(!path
                .with_file_name(format!(
                    "{}.{}",
                    path.file_name().unwrap().to_str().unwrap(),
                    BACKUP_SUFFIX
                ))
                .exists());
        }
    }
}
