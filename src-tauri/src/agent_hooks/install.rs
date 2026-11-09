//! Explicit, reversible managed-hook installation. Merely querying status never writes.
use super::AgentKind;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static INSTALL_LOCK: Mutex<()> = Mutex::new(());
static NONCE: AtomicU64 = AtomicU64::new(0);
const QIN_EVENTS: &[&str] = &[
    "session_start",
    "turn_start",
    "turn_end",
    "session_end",
    "post_tool",
];
// Verified against stablyai/orca src/main/claude/claude-hook-event-versions.ts
// (2026-11-09). Unknown versions use its conservative 1.0.64 floor. Hooks
// never return permission decisions; PermissionRequest is observation only.
// SessionEnd uses the accepted compatibility floor of 2.1.261.
const CLAUDE_EVENTS: &[(&str, (u32, u32, u32))] = &[
    ("PreToolUse", (1, 0, 23)),
    ("PostToolUse", (1, 0, 23)),
    ("Notification", (1, 0, 23)),
    ("Stop", (1, 0, 31)),
    ("SubagentStop", (1, 0, 41)),
    ("UserPromptSubmit", (1, 0, 53)),
    ("SessionStart", (1, 0, 62)),
    ("SessionEnd", (2, 1, 261)),
    ("SubagentStart", (2, 0, 43)),
    ("PermissionRequest", (2, 0, 45)),
    ("PostToolUseFailure", (2, 0, 56)),
];
const PI_EXTENSION: &str = "// Se managed agent hooks v2\n";
const PI_EXTENSION_V1: &str = "// Se managed agent hooks v1\n";

#[derive(Debug, Clone, Serialize)]
pub struct HookInstallStatus {
    pub agent: AgentKind,
    pub state: String,
    pub config_path: String,
    pub detail: Option<String>,
    pub cli_path: Option<String>,
    pub version: Option<String>,
}

#[derive(Default, Clone)]
struct Cli {
    path: Option<PathBuf>,
    version: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Receipt {
    original: Option<Vec<u8>>,
    installed: Vec<u8>,
}

struct Adapter {
    agent: AgentKind,
    home: PathBuf,
    cli: Cli,
}

pub fn statuses() -> Vec<HookInstallStatus> {
    let _guard = INSTALL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    agents()
        .into_iter()
        .map(|agent| match home() {
            Ok(home) => Adapter::new(agent, home, true).status(),
            Err(detail) => HookInstallStatus {
                agent,
                state: "error".into(),
                config_path: String::new(),
                detail: Some(detail),
                cli_path: None,
                version: None,
            },
        })
        .collect()
}

/// Preview only Se-owned changes. Unrelated configuration can contain secrets
/// and is deliberately never returned to renderer/plugin consumers.
pub fn preview(agent: AgentKind) -> Result<String, String> {
    let _guard = INSTALL_LOCK
        .lock()
        .map_err(|_| "Hook installer lock poisoned".to_owned())?;
    Adapter::new(agent, home()?, true).preview()
}

pub fn install(agent: AgentKind) -> Result<HookInstallStatus, String> {
    change(agent, true)
}

pub fn uninstall(agent: AgentKind) -> Result<HookInstallStatus, String> {
    change(agent, false)
}

fn change(agent: AgentKind, installing: bool) -> Result<HookInstallStatus, String> {
    let _guard = INSTALL_LOCK
        .lock()
        .map_err(|_| "Hook installer lock poisoned".to_owned())?;
    let name = agent_name(&agent);
    let operation = if installing {
        "agent_hook_install"
    } else {
        "agent_hook_uninstall"
    };
    log::info!("operation={} agent={} kind=start count=1", operation, name);
    let result = home().and_then(|home| {
        let adapter = Adapter::new(agent, home, true);
        if installing {
            adapter.install()
        } else {
            adapter.uninstall()
        }
    });
    match &result {
        Ok(_) => log::info!(
            "operation={} agent={} kind=complete count=1",
            operation,
            name
        ),
        // Do not log config contents, command output, tokens, or payloads.
        Err(_) => log::warn!("operation={} agent={} kind=failed count=1", operation, name),
    }
    result
}

fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is unavailable; no hook files were changed".into())
}

fn agents() -> Vec<AgentKind> {
    vec![AgentKind::ClaudeCode, AgentKind::Pi, AgentKind::QinCode]
}

fn agent_name(agent: &AgentKind) -> String {
    agent.as_str().to_owned()
}

impl Adapter {
    fn new(agent: AgentKind, home: PathBuf, detect: bool) -> Self {
        let cli = if detect {
            discover(&agent_name(&agent))
        } else {
            Cli::default()
        };
        Self { agent, home, cli }
    }

    fn name(&self) -> String {
        agent_name(&self.agent)
    }
    fn root(&self) -> PathBuf {
        self.home.join(".se-manager/agent-hooks")
    }
    fn script(&self) -> PathBuf {
        self.root().join(format!("{}-hook.sh", self.name()))
    }
    fn receipt(&self) -> PathBuf {
        self.root().join(format!("{}.install.json", self.name()))
    }
    fn config(&self) -> PathBuf {
        self.home.join(match self.name().as_str() {
            "claude-code" => ".claude/settings.json",
            "pi" => ".pi/agent/extensions/se-agent-status.ts",
            _ => ".qin-code/config.toml",
        })
    }
    fn command(&self) -> String {
        shell_quote(&self.script().to_string_lossy())
    }

    fn claude_events(&self) -> Vec<&'static str> {
        let version = self
            .cli
            .version
            .as_deref()
            .and_then(|raw| {
                raw.lines().find_map(|line| {
                    let (prefix, _) = line.split_once("(Claude Code)")?;
                    prefix.split_whitespace().last().and_then(parse_version)
                })
            })
            .unwrap_or((1, 0, 64));
        CLAUDE_EVENTS
            .iter()
            .filter_map(|(event, floor)| (version >= *floor).then_some(*event))
            .collect()
    }

    fn status(&self) -> HookInstallStatus {
        let mut result = HookInstallStatus {
            agent: self.agent,
            state: "not_installed".into(),
            config_path: self.config().to_string_lossy().into_owned(),
            detail: None,
            cli_path: self
                .cli
                .path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            version: self.cli.version.clone(),
        };
        let probe = (|| -> Result<(bool, bool), String> {
            let bytes = read_optional(&self.config())?;
            let script = read_optional(&self.script())?;
            let script_ok = script.as_deref() == Some(self.script_source().as_bytes())
                && executable(&self.script());
            let (config_ok, managed_config) = match self.name().as_str() {
                "pi" => (
                    bytes.as_deref() == Some(self.pi_source().as_bytes()),
                    bytes.as_deref().is_some_and(|bytes| {
                        bytes.starts_with(PI_EXTENSION.as_bytes())
                            || bytes.starts_with(PI_EXTENSION_V1.as_bytes())
                    }),
                ),
                "claude-code" => {
                    let value = parse_json(bytes.as_deref())?;
                    let events = self.claude_events();
                    let count = events
                        .iter()
                        .filter(|event| claude_has(&value, event, &self.command()))
                        .count();
                    let owned_count = value
                        .get("hooks")
                        .and_then(Value::as_object)
                        .map(|hooks| {
                            hooks
                                .keys()
                                .filter(|event| claude_has(&value, event, &self.command()))
                                .count()
                        })
                        .unwrap_or_default();
                    (
                        count == events.len() && owned_count == count,
                        owned_count > 0,
                    )
                }
                _ => {
                    let text = utf8(bytes.as_deref().unwrap_or_default())?;
                    let doc = text
                        .parse::<toml_edit::DocumentMut>()
                        .map_err(|_| "Invalid qin-code TOML; no changes made".to_owned())?;
                    let count = QIN_EVENTS
                        .iter()
                        .filter(|event| qin_has(&doc, event, &self.command()))
                        .count();
                    (count == QIN_EVENTS.len(), count > 0)
                }
            };
            Ok((config_ok && script_ok, managed_config))
        })();
        match probe {
            Ok((true, _)) => result.state = "installed".into(),
            Ok((false, managed_config)) => {
                if managed_config || self.script().exists() || self.receipt().exists() {
                    result.state = "outdated".into();
                } else if self.cli.path.is_none() {
                    result.state = "agent_not_found".into();
                }
            }
            Err(err) => {
                result.state = "error".into();
                result.detail = Some(err);
            }
        }
        if result.detail.is_none() && self.cli.path.is_none() {
            result.detail = Some("CLI not found on the login-shell PATH; installation is manual and can be used after installing the CLI".into());
        }
        result
    }

    fn preview(&self) -> Result<String, String> {
        let original = read_optional(&self.config())?;
        let desired = self.merge(original.as_deref(), true)?;
        let existing_script = read_optional(&self.script())?;
        if existing_script
            .as_deref()
            .is_some_and(|bytes| bytes != self.script_source().as_bytes())
        {
            return Err(
                "Managed script path contains an unrecognized file; refusing to overwrite it"
                    .into(),
            );
        }
        let mut output = format!("Agent: {}\nConfig: {}\nScript: {}\nEndpoint (read only): {}\n\nUnrelated user settings and hooks are retained; their contents are not exposed in this preview.\n", self.name(), self.config().display(), self.script().display(), self.root().join("endpoint.env").display());
        if original.as_deref() == Some(desired.as_slice()) {
            output.push_str("\nNo configuration changes required.\n");
        } else {
            output.push_str(&format!(
                "\n--- {} (managed entries only)\n+++ {} (managed entries only)\n",
                self.config().display(),
                self.config().display()
            ));
            let patch = match self.name().as_str() {
                "claude-code" => {
                    let value = parse_json(original.as_deref())?;
                    let mut patch = String::new();
                    if let Some(hooks) = value.get("hooks").and_then(Value::as_object) {
                        for (event, entries) in hooks {
                            if !self.claude_events().contains(&event.as_str())
                                && entries.as_array().is_some_and(|entries| {
                                    entries
                                        .iter()
                                        .any(|entry| claude_entry_has(entry, &self.command()))
                                })
                            {
                                patch.push_str(&format!(
                                    "- hooks.{event}: Se-owned command {}\n",
                                    self.command()
                                ));
                            }
                        }
                    }
                    for event in self.claude_events() {
                        if !claude_has(&value, event, &self.command()) {
                            patch.push_str(&format!("+ hooks.{event}: {}\n", json!({"matcher":"*", "hooks":[{"type":"command", "command":self.command(), "timeout":3}]})));
                        }
                    }
                    patch
                }
                "qin-code" => {
                    let doc = utf8(original.as_deref().unwrap_or_default())?
                        .parse::<toml_edit::DocumentMut>()
                        .map_err(|_| "Invalid qin-code TOML".to_owned())?;
                    let mut patch = String::new();
                    for event in QIN_EVENTS
                        .iter()
                        .filter(|event| !qin_has(&doc, event, &self.command()))
                    {
                        patch.push_str(&format!(
                            "+ [[hooks.rules]]\n+ event = {}\n+ command = {}\n+ min_level = {}\n",
                            toml_string(event),
                            toml_string(&self.command()),
                            toml_string(if *event == "post_tool" {
                                "standard"
                            } else {
                                "minimal"
                            })
                        ));
                    }
                    patch
                }
                _ => self
                    .pi_source()
                    .lines()
                    .map(|line| format!("+ {line}\n"))
                    .collect(),
            };
            output.push_str(&patch);
        }
        if existing_script.is_none() || !executable(&self.script()) {
            output.push_str(&format!(
                "\n+ {} (executable observer, mode 0755)\n",
                self.script().display()
            ));
        }
        output.push_str("\nA private recovery receipt is kept under ~/.se-manager/agent-hooks. Changed existing configs receive atomic .se-backup-* files. No writes have been made by preview.\n");
        Ok(output)
    }

    fn install(&self) -> Result<HookInstallStatus, String> {
        let original = read_optional(&self.config())?;
        // Validate before creating ANY backup, script or receipt.
        let desired = self.merge(original.as_deref(), true)?;
        let old_receipt = read_optional(&self.receipt())?
            .map(|bytes| {
                serde_json::from_slice::<Receipt>(&bytes).map_err(|_| {
                    "Invalid installer receipt; refusing to overwrite recovery data".to_owned()
                })
            })
            .transpose()?;
        if let Some(existing) = read_optional(&self.script())? {
            if existing != self.script_source().as_bytes() {
                return Err(
                    "Managed script path contains an unrecognized file; refusing to overwrite it"
                        .into(),
                );
            }
        }
        let receipt = Receipt {
            original: match old_receipt {
                Some(ref receipt) if original.as_deref() == Some(receipt.installed.as_slice()) => {
                    receipt.original.clone()
                }
                Some(_) if self.name() == "pi" => None,
                Some(_) => original
                    .as_deref()
                    .map(|bytes| self.merge(Some(bytes), false))
                    .transpose()?,
                None if self.name() == "pi" => None,
                None => original
                    .as_deref()
                    .map(|bytes| self.merge(Some(bytes), false))
                    .transpose()?,
            },
            installed: desired.clone(),
        };
        // Retain a recovery record before mutation. Repeated installation is a no-op.
        write_if_changed(
            &self.receipt(),
            &serde_json::to_vec(&receipt).map_err(|e| e.to_string())?,
            0o600,
        )?;
        write_owned_script(&self.script(), self.script_source().as_bytes())?;
        if original.as_deref() != Some(desired.as_slice()) {
            if let Some(bytes) = &original {
                backup(&self.config(), bytes)?;
            }
            atomic_write(&self.config(), &desired, file_mode(&self.config(), 0o600))?;
        }
        let mut status = self.status();
        if status.state != "installed" {
            return Err("Managed hook read-back verification failed".into());
        }
        if self.name() == "qin-code" {
            if let Some(path) = &self.cli.path {
                match bounded_output(path, &["hooks", "status", "--json"], Duration::from_secs(3)) {
                    Some(output) => {
                        status.detail = Some(match serde_json::from_slice::<Value>(&output).ok()
                            .and_then(|value| value.get("level").and_then(Value::as_str).map(str::to_owned)).as_deref() {
                            Some("none") => "Rules installed, but qin-code hooks are disabled (level=none); existing level was not changed",
                            Some("minimal") => "Lifecycle hooks active at level=minimal; post_tool requires standard/full (existing level was not changed)",
                            Some("standard" | "full") => "Managed rules installed; qin-code hooks status --json confirms observer tiers are enabled",
                            _ => "Rules installed and read back; qin-code diagnostics did not confirm the effective hook level",
                        }.into());
                    }
                    _ => status.detail = Some("Rules installed and read back; qin-code hooks status --json could not verify runtime activation".into()),
                }
            }
        }
        Ok(status)
    }

    fn uninstall(&self) -> Result<HookInstallStatus, String> {
        let current = read_optional(&self.config())?;
        let receipt = read_optional(&self.receipt())?
            .map(|bytes| {
                serde_json::from_slice::<Receipt>(&bytes)
                    .map_err(|_| "Invalid installer receipt; preserve it for recovery".to_owned())
            })
            .transpose()?;
        let desired = match receipt {
            Some(receipt) if current.as_deref() == Some(receipt.installed.as_slice()) => {
                receipt.original
            }
            _ if self.name() == "pi" => {
                // A missing receipt does not authorize deleting arbitrary files.
                self.merge(current.as_deref(), false)?;
                None
            }
            _ => match current.as_deref() {
                Some(bytes) => Some(self.merge(Some(bytes), false)?),
                None => None,
            },
        };
        if current != desired {
            if let Some(bytes) = &current {
                backup(&self.config(), bytes)?;
            }
            match desired {
                Some(bytes) => {
                    atomic_write(&self.config(), &bytes, file_mode(&self.config(), 0o600))?
                }
                None => remove_file(&self.config())?,
            }
        }
        // Never erase a manually replaced/modified file at the managed path.
        if read_optional(&self.script())?.as_deref() == Some(self.script_source().as_bytes()) {
            remove_file(&self.script())?;
        }
        remove_file(&self.receipt())?;
        Ok(self.status())
    }

    fn merge(&self, original: Option<&[u8]>, installing: bool) -> Result<Vec<u8>, String> {
        match self.name().as_str() {
            "pi" => {
                if let Some(bytes) = original {
                    if bytes != self.pi_source().as_bytes()
                        && bytes != self.pi_v1_source().as_bytes()
                    {
                        return Err("The pi extension path contains an unrecognized file; refusing to overwrite or delete it".into());
                    }
                }
                // Missing-file removal is handled in uninstall; a managed pi extension
                // without a receipt is also safe to remove, never an arbitrary TS file.
                if installing {
                    Ok(self.pi_source().into_bytes())
                } else {
                    Ok(Vec::new())
                }
            }
            "claude-code" => self.merge_claude(original, installing),
            _ => self.merge_qin(original, installing),
        }
    }

    fn merge_claude(&self, original: Option<&[u8]>, installing: bool) -> Result<Vec<u8>, String> {
        let mut value = parse_json(original)?;
        let command = self.command();
        let mut changed = false;
        if installing {
            let object = value
                .as_object_mut()
                .ok_or("Claude settings must be a JSON object")?;
            if !object.contains_key("hooks") {
                object.insert("hooks".into(), json!({}));
            }
            let hooks = object
                .get_mut("hooks")
                .and_then(Value::as_object_mut)
                .ok_or("Claude hooks must be an object")?;
            let events = self.claude_events();
            if events.is_empty() {
                return Err(
                    "This Claude version predates supported hooks (requires at least 1.0.23)"
                        .into(),
                );
            }
            let mut emptied = Vec::new();
            for (event, entries) in hooks.iter_mut() {
                if !events.contains(&event.as_str()) {
                    let entries = entries
                        .as_array_mut()
                        .ok_or("Claude hook event must be an array")?;
                    let removed = remove_claude_commands(entries, &command);
                    changed |= removed;
                    if removed && entries.is_empty() {
                        emptied.push(event.clone());
                    }
                }
            }
            for event in emptied {
                hooks.remove(&event);
            }
            for event in events {
                let entries = hooks
                    .entry(event)
                    .or_insert_with(|| json!([]))
                    .as_array_mut()
                    .ok_or("Claude hook event must be an array")?;
                if !entries
                    .iter()
                    .any(|entry| claude_entry_has(entry, &command))
                {
                    entries.push(json!({"matcher":"*", "hooks":[{"type":"command", "command":command, "timeout":3}]}));
                    changed = true;
                }
            }
        } else if let Some(hooks) = value.get_mut("hooks") {
            let hooks = hooks
                .as_object_mut()
                .ok_or("Claude hooks must be an object")?;
            for entries in hooks.values_mut() {
                let entries = entries
                    .as_array_mut()
                    .ok_or("Claude hook event must be an array")?;
                changed |= remove_claude_commands(entries, &command);
            }
        }
        if !changed {
            return Ok(original.unwrap_or(b"{}").to_vec());
        }
        let mut bytes = serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    fn qin_block(&self) -> String {
        let mut block = String::from("\n# BEGIN Se managed agent hooks v1\n");
        for event in QIN_EVENTS {
            let level = if *event == "post_tool" {
                "standard"
            } else {
                "minimal"
            };
            block.push_str(&format!(
                "[[hooks.rules]]\nevent = {}\ncommand = {}\nmin_level = {}\n\n",
                toml_string(event),
                toml_string(&self.command()),
                toml_string(level)
            ));
        }
        block.push_str("# END Se managed agent hooks v1\n");
        block
    }

    fn merge_qin(&self, original: Option<&[u8]>, installing: bool) -> Result<Vec<u8>, String> {
        let text = utf8(original.unwrap_or_default())?;
        let mut doc = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|_| "Invalid qin-code TOML; no changes made".to_owned())?;
        let command = self.command();
        if installing {
            if QIN_EVENTS
                .iter()
                .all(|event| qin_has(&doc, event, &command))
            {
                return Ok(text.as_bytes().to_vec());
            }
            // Append only missing observer rules, leaving legacy fields and user tables byte-for-byte intact.
            let mut desired = text.to_owned();
            if !QIN_EVENTS
                .iter()
                .any(|event| qin_has(&doc, event, &command))
            {
                desired.push_str(&self.qin_block());
            } else {
                for event in QIN_EVENTS
                    .iter()
                    .filter(|event| !qin_has(&doc, event, &command))
                {
                    desired.push_str(&format!(
                        "\n[[hooks.rules]]\nevent = {}\ncommand = {}\nmin_level = {}\n",
                        toml_string(event),
                        toml_string(&command),
                        toml_string(if *event == "post_tool" {
                            "standard"
                        } else {
                            "minimal"
                        })
                    ));
                }
            }
            desired.parse::<toml_edit::DocumentMut>().map_err(|_| {
                "Existing qin-code hooks cannot safely accept additional rules".to_owned()
            })?;
            Ok(desired.into_bytes())
        } else if text.contains(&self.qin_block()) {
            Ok(text.replace(&self.qin_block(), "").into_bytes())
        } else {
            let mut changed = false;
            if let Some(rules) = doc
                .get_mut("hooks")
                .and_then(|h| h.get_mut("rules"))
                .and_then(toml_edit::Item::as_array_of_tables_mut)
            {
                let before = rules.len();
                rules.retain(|rule| !owned_qin_rule(rule, &command));
                changed = before != rules.len();
            }
            Ok(if changed {
                doc.to_string().into_bytes()
            } else {
                text.as_bytes().to_vec()
            })
        }
    }

    fn script_source(&self) -> String {
        let input = if self.name() == "qin-code" {
            "printf '%s' \"${QIN_CODE_HOOK_PAYLOAD:-}\" | "
        } else {
            ""
        };
        format!("#!/bin/sh\n# Se managed agent hooks v1\n{}endpoint={}\n[ -r \"$endpoint\" ] && . \"$endpoint\"\n[ -n \"${{SE_AGENT_HOOK_PORT:-}}\" ] && [ -n \"${{SE_AGENT_HOOK_TOKEN:-}}\" ] && [ -n \"${{SE_TERMINAL_ID:-}}\" ] || exit 0\ncase \"$SE_AGENT_HOOK_PORT\" in *[!0-9]*|'') exit 0;; esac\n[ \"$SE_AGENT_HOOK_PORT\" -ge 1 ] && [ \"$SE_AGENT_HOOK_PORT\" -le 65535 ] || exit 0\n{}curl --silent --show-error --connect-timeout 1 --max-time 2 -o /dev/null \\\n  -H \"Authorization: Bearer $SE_AGENT_HOOK_TOKEN\" -H 'Content-Type: application/json' \\\n  -H \"x-se-terminal-id: $SE_TERMINAL_ID\" -H \"x-se-project-id: ${{SE_PROJECT_ID:-}}\" \\\n  -H \"x-se-hook-event: ${{QIN_CODE_HOOK_EVENT:-}}\" -H \"x-se-session-id: ${{QIN_CODE_HOOK_SESSION_ID:-}}\" \\\n  --data-binary @- \"http://127.0.0.1:$SE_AGENT_HOOK_PORT/agent-hooks/{}\" 2>/dev/null || :\nexit 0\n",
            if self.name() == "claude-code" { "printf '{}\\n'\n" } else { "" },
            shell_quote(&self.root().join("endpoint.env").to_string_lossy()), input, self.name())
    }

    fn pi_source(&self) -> String {
        format!(
            r#"{PI_EXTENSION}import {{ spawn }} from "node:child_process";
export default function (pi: any) {{
  // session_start precedes the CLI's initial prompt/agent_start. Unknown flags,
  // positional prompts and @file inputs therefore fail closed, not briefly idle.
  const flags = new Set(["--continue", "-c", "--resume", "-r", "--no-session", "--no-tools", "--no-extensions", "-ne", "--no-skills", "-ns", "--no-prompt-templates", "-np", "--no-themes", "--offline", "--hide-cwd-in-prompt", "--verbose"]);
  const options = new Set(["--provider", "--model", "--api-key", "--thinking", "--session", "--session-dir", "--tools", "--extension", "-e", "--skill", "--prompt-template", "--theme", "--models", "--keybindings", "--system-prompt", "--append-system-prompt"]);
  const args = process.argv.slice(2);
  let promptFree = true;
  for (let i = 0; i < args.length; i++) {{
    if (flags.has(args[i])) continue;
    if (options.has(args[i]) && args[i + 1] && !args[i + 1].startsWith("-")) {{ i++; continue; }}
    promptFree = false;
    break;
  }}
  let revision = 0;
  let delivery = Promise.resolve();
  const report = (name: string, event: any, ctx: any, ready: any = undefined) => {{
    try {{
      const payload = JSON.stringify({{ event: name, session_id: ctx?.sessionManager?.getSessionId?.(), session_file: ctx?.sessionManager?.getSessionFile?.(), tool_name: event.toolName, ok: !event.isError }});
      if (Buffer.byteLength(payload) > 1048576) return;
      // Delivery is detached from pi callbacks, but ordered: a late idle POST
      // must not overtake agent_start or arrive before its session identity.
      delivery = delivery.then(() => new Promise<void>((done) => {{
        try {{
          if (ready && !ready()) {{ done(); return; }}
          const child = spawn({script}, [], {{ stdio: ["pipe", "ignore", "ignore"] }});
          child.on("error", () => done());
          child.on("close", () => done());
          child.stdin.on("error", () => {{}});
          child.stdin.end(payload);
          child.unref();
        }} catch {{ done(); }}
      }}));
    }} catch {{ /* Observers must never alter tool decisions or interrupt pi. */ }}
  }};
  const events = ["session_start", "agent_start", "agent_end", "tool_execution_start", "tool_execution_end", "tool_approval_requested", "tool_approval_resolved", "ui_prompt_start", "ui_prompt_end", "session_shutdown"];
  for (const name of events) pi.on(name, (event: any, ctx: any) => {{
    if (!process.env.SE_TERMINAL_ID) return;
    const current = ++revision;
    report(name, event, ctx);
    if (name === "session_start") report("idle", event, ctx, () =>
      current === revision && promptFree && process.stdin.isTTY === true &&
      ctx?.hasUI === true && ctx?.isIdle?.() === true && ctx?.hasPendingMessages?.() === false);
  }});
}}
"#,
            script = serde_json::to_string(&self.script().to_string_lossy()).unwrap()
        )
    }

    // Keep the exact v1 bytes: ownership is not inferred from a forgeable header.
    fn pi_v1_source(&self) -> String {
        format!(
            r#"{PI_EXTENSION_V1}import {{ spawn }} from "node:child_process";
export default function (pi: any) {{
  const events = ["session_start", "agent_start", "agent_end", "tool_execution_start", "tool_execution_end", "tool_approval_requested", "tool_approval_resolved", "ui_prompt_start", "ui_prompt_end", "session_shutdown"];
  for (const name of events) pi.on(name, (event: any, ctx: any) => {{
    if (!process.env.SE_TERMINAL_ID) return;
    try {{
      const payload = JSON.stringify({{ event: name, session_id: ctx?.sessionManager?.getSessionId?.(), session_file: ctx?.sessionManager?.getSessionFile?.(), tool_name: event.toolName, ok: !event.isError }});
      if (Buffer.byteLength(payload) > 1048576) return;
      const child = spawn({script}, [], {{ stdio: ["pipe", "ignore", "ignore"] }});
      child.on("error", () => {{}});
      child.stdin.on("error", () => {{}});
      child.stdin.end(payload);
      child.unref();
    }} catch {{ /* Observers must never alter tool decisions or interrupt pi. */ }}
  }});
}}
"#,
            script = serde_json::to_string(&self.script().to_string_lossy()).unwrap()
        )
    }
}

fn parse_json(bytes: Option<&[u8]>) -> Result<Value, String> {
    let value: Value = serde_json::from_slice(bytes.unwrap_or(b"{}"))
        .map_err(|_| "Invalid Claude settings JSON; no changes made".to_owned())?;
    if !value.is_object() {
        return Err("Claude settings must be a JSON object".into());
    }
    if value.get("hooks").is_some_and(|h| !h.is_object()) {
        return Err("Claude hooks must be an object".into());
    }
    if let Some(hooks) = value.get("hooks").and_then(Value::as_object) {
        for entries in hooks.values() {
            let entries = entries
                .as_array()
                .ok_or("Claude hook event must be an array")?;
            for entry in entries {
                if !entry.get("hooks").is_some_and(Value::is_array) {
                    return Err("Claude hook groups must contain a hooks array".into());
                }
            }
        }
    }
    Ok(value)
}
fn remove_claude_commands(entries: &mut Vec<Value>, command: &str) -> bool {
    let mut changed = false;
    entries.retain_mut(|entry| {
        let mut removed = false;
        let mut empty = false;
        if let Some(commands) = entry.get_mut("hooks").and_then(Value::as_array_mut) {
            let before = commands.len();
            commands.retain(|hook| !owned_claude_command(hook, command));
            removed = before != commands.len();
            empty = commands.is_empty();
            changed |= removed;
        }
        !(removed
            && empty
            && entry.get("matcher").and_then(Value::as_str) == Some("*")
            && entry.as_object().is_some_and(|o| o.len() == 2))
    });
    changed
}
fn owned_claude_command(hook: &Value, command: &str) -> bool {
    hook.get("type").and_then(Value::as_str) == Some("command")
        && hook.get("command").and_then(Value::as_str) == Some(command)
}
fn claude_entry_has(entry: &Value, command: &str) -> bool {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| hooks.iter().any(|hook| owned_claude_command(hook, command)))
}
fn claude_has(value: &Value, event: &str, command: &str) -> bool {
    value
        .get("hooks")
        .and_then(|v| v.get(event))
        .and_then(Value::as_array)
        .is_some_and(|entries| entries.iter().any(|entry| claude_entry_has(entry, command)))
}
fn owned_qin_rule(rule: &toml_edit::Table, command: &str) -> bool {
    rule.get("command").and_then(toml_edit::Item::as_str) == Some(command)
        && rule
            .get("event")
            .and_then(toml_edit::Item::as_str)
            .is_some_and(|event| QIN_EVENTS.contains(&event))
}
fn qin_has(doc: &toml_edit::DocumentMut, event: &str, command: &str) -> bool {
    doc.get("hooks")
        .and_then(|h| h.get("rules"))
        .and_then(toml_edit::Item::as_array_of_tables)
        .is_some_and(|rules| {
            rules.iter().any(|rule| {
                rule.get("event").and_then(toml_edit::Item::as_str) == Some(event)
                    && owned_qin_rule(rule, command)
            })
        })
}
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn toml_string(value: &str) -> String {
    toml_edit::Value::from(value).to_string()
}
fn utf8(bytes: &[u8]) -> Result<&str, String> {
    std::str::from_utf8(bytes).map_err(|_| "Configuration is not UTF-8; no changes made".into())
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => Err(format!(
            "Refusing non-regular hook file: {}",
            path.display()
        )),
        Ok(_) => fs::read(path)
            .map(Some)
            .map_err(|e| format!("Read {}: {e}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("Inspect {}: {err}", path.display())),
    }
}
fn unique() -> String {
    format!(
        "{}-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    )
}
fn backup(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".se-backup-{}", unique()));
    atomic_write(&PathBuf::from(name), bytes, 0o600)
}
fn write_if_changed(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    if read_optional(path)?.as_deref() != Some(bytes) {
        atomic_write(path, bytes, mode)?;
    }
    Ok(())
}
fn write_owned_script(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(existing) = read_optional(path)? {
        if existing != bytes {
            return Err(
                "Managed script path contains an unrecognized file; refusing to overwrite it"
                    .into(),
            );
        }
    }
    write_if_changed(path, bytes, 0o755)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn file_mode(path: &Path, fallback: u32) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(fallback)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        fallback
    }
}
fn executable(path: &Path) -> bool {
    path.is_file() && file_mode(path, 0o755) & 0o111 != 0
}
fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    // Reject symlink destinations rather than following a user config link.
    let _ = read_optional(path)?;
    let parent = path.parent().ok_or("Hook path has no parent")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temporary = parent.join(format!(".se-hook-{}.tmp", unique()));
    let result = (|| -> Result<(), String> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(mode);
        }
        let mut file = options.open(&temporary).map_err(|e| e.to_string())?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        fs::rename(&temporary, path).map_err(|e| e.to_string())?;
        if let Ok(directory) = fs::File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
fn remove_file(path: &Path) -> Result<(), String> {
    if read_optional(path)?.is_some() {
        fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn parse_version(raw: &str) -> Option<(u32, u32, u32)> {
    raw.split(|c: char| !c.is_ascii_digit() && c != '.')
        .find_map(|part| {
            let mut parts = part.split('.');
            Some((
                parts.next()?.parse().ok()?,
                parts.next()?.parse().ok()?,
                parts.next()?.parse().ok()?,
            ))
        })
}
fn discover(agent: &str) -> Cli {
    let binary = if agent == "claude-code" {
        "claude"
    } else {
        agent
    };
    let mut env = HashMap::new();
    crate::pty::env_refresh::apply_fresh_path(&mut env);
    let path = env
        .get("PATH")
        .cloned()
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    let found = std::env::split_paths(&path)
        .map(|dir| dir.join(binary))
        .find(|p| executable(p));
    let version = found
        .as_ref()
        .and_then(|p| bounded_output(p, &["--version"], Duration::from_secs(2)))
        .map(|bytes| {
            String::from_utf8_lossy(&bytes)
                .trim()
                .chars()
                .take(256)
                .collect()
        });
    Cli {
        path: found,
        version,
    }
}
fn bounded_output(path: &Path, args: &[&str], limit: Duration) -> Option<Vec<u8>> {
    let mut env = HashMap::new();
    crate::pty::env_refresh::apply_fresh_path(&mut env);
    let inherited = env
        .get("PATH")
        .cloned()
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    let path_env = std::env::join_paths(
        path.parent()
            .into_iter()
            .map(PathBuf::from)
            .chain(std::env::split_paths(&inherited)),
    )
    .ok()?;
    let mut child = Command::new(path)
        .args(args)
        .env("PATH", path_env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    #[cfg(unix)]
    {
        use std::io::Read;
        use std::os::fd::AsRawFd;
        let mut output = child.stdout.take()?;
        // Child stdout is private to this invocation. Nonblocking reads let the
        // deadline apply even when a grandchild keeps the write end open.
        let fd = output.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        let start = Instant::now();
        let mut bytes = Vec::new();
        let mut exited: Option<bool> = None;
        loop {
            let mut buffer = [0; 4096];
            loop {
                match output.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) if bytes.len() + count <= 65536 => {
                        bytes.extend_from_slice(&buffer[..count])
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return None;
                    }
                }
            }
            if let Some(success) = exited {
                return if success { Some(bytes) } else { None };
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    exited = Some(status.success());
                }
                Ok(None) if start.elapsed() < limit => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let start = Instant::now();
        while start.elapsed() < limit {
            if let Ok(Some(status)) = child.try_wait() {
                return status
                    .success()
                    .then(|| child.wait_with_output().ok())
                    .flatten()
                    .map(|o| o.stdout)
                    .filter(|o| o.len() <= 65536);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = child.kill();
        let _ = child.wait();
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // Fixed, deliberately quoted home: never discover a CLI, mutate HOME, or write here.
    const LEGACY_FIXTURE_HOME: &str = "/se-hooks-legacy-fixture/home 'with spaces";

    fn assert_legacy_fixture(name: &str, rendered: &[u8]) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/agent_hooks/tests/fixtures/legacy")
            .join(name);
        let expected = fs::read(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        // Ownership depends on exact bytes: no newline, whitespace or path normalization.
        assert_eq!(rendered, expected.as_slice(), "legacy fixture: {name}");
    }

    #[test]
    fn legacy_renderers_reproduce_exact_committed_bytes() {
        for agent in agents() {
            let adapter = Adapter::new(agent, PathBuf::from(LEGACY_FIXTURE_HOME), false);
            assert_legacy_fixture(
                &format!("{}-hook.v1.sh", adapter.name()),
                adapter.script_source().as_bytes(),
            );
        }

        let pi = Adapter::new(AgentKind::Pi, PathBuf::from(LEGACY_FIXTURE_HOME), false);
        let pi_v1 = pi.pi_v1_source();
        let pi_v2 = pi.pi_source();
        assert_legacy_fixture("pi-extension.v1.ts", pi_v1.as_bytes());
        assert_legacy_fixture("pi-extension.v2.ts", pi_v2.as_bytes());
        for original in [None, Some(pi_v1.as_bytes()), Some(pi_v2.as_bytes())] {
            assert_eq!(pi.merge(original, true).unwrap(), pi_v2.as_bytes());
        }
        for original in [pi_v1.as_bytes(), pi_v2.as_bytes()] {
            assert!(pi.merge(Some(original), false).unwrap().is_empty());
        }

        let qin = Adapter::new(
            AgentKind::QinCode,
            PathBuf::from(LEGACY_FIXTURE_HOME),
            false,
        );
        let qin_user = include_bytes!("tests/fixtures/legacy/qin-user-config.input.toml");
        for (name, original) in [("empty", None), ("user", Some(qin_user.as_slice()))] {
            let installed = qin.merge(original, true).unwrap();
            assert_legacy_fixture(&format!("qin-config.{name}.installed.toml"), &installed);
            assert_eq!(qin.merge(Some(&installed), true).unwrap(), installed);
            let removed = qin.merge(Some(&installed), false).unwrap();
            assert_legacy_fixture(&format!("qin-config.{name}.removed.toml"), &removed);
            assert_eq!(removed, original.unwrap_or_default());
        }
        assert_eq!(qin.merge(None, true).unwrap(), qin.qin_block().as_bytes());

        let mut claude = Adapter::new(
            AgentKind::ClaudeCode,
            PathBuf::from(LEGACY_FIXTURE_HOME),
            false,
        );
        // Freeze every distinct event-set output and both sides of each gate.
        // Unknown/unparseable versions share the conservative 1.0.64 event set.
        for (version, fixture_version) in [
            (None, "1.0.62"),
            (Some("99.0.0 (launcher, not Claude)"), "1.0.62"),
            (Some("launcher 99.0.0\n1.0.23 (Claude Code)"), "1.0.23"),
            (Some("1.0.23 (Claude Code)"), "1.0.23"),
            (Some("1.0.30 (Claude Code)"), "1.0.23"),
            (Some("1.0.31 (Claude Code)"), "1.0.31"),
            (Some("1.0.40 (Claude Code)"), "1.0.31"),
            (Some("1.0.41 (Claude Code)"), "1.0.41"),
            (Some("1.0.52 (Claude Code)"), "1.0.41"),
            (Some("1.0.53 (Claude Code)"), "1.0.53"),
            (Some("1.0.61 (Claude Code)"), "1.0.53"),
            (Some("1.0.62 (Claude Code)"), "1.0.62"),
            (Some("2.0.42 (Claude Code)"), "1.0.62"),
            (Some("2.0.43 (Claude Code)"), "2.0.43"),
            (Some("2.0.44 (Claude Code)"), "2.0.43"),
            (Some("2.0.45 (Claude Code)"), "2.0.45"),
            (Some("2.0.55 (Claude Code)"), "2.0.45"),
            (Some("2.0.56 (Claude Code)"), "2.0.56"),
            (Some("2.1.260 (Claude Code)"), "2.0.56"),
            (Some("2.1.261 (Claude Code)"), "2.1.261"),
        ] {
            claude.cli.version = version.map(str::to_owned);
            let installed = claude.merge(None, true).unwrap();
            assert_legacy_fixture(
                &format!("claude-settings.empty.{fixture_version}.json"),
                &installed,
            );
            assert_eq!(claude.merge(Some(&installed), true).unwrap(), installed);
        }
        claude.cli.version = Some("1.0.22 (Claude Code)".into());
        assert_eq!(
            claude.merge(None, true).unwrap_err(),
            "This Claude version predates supported hooks (requires at least 1.0.23)"
        );

        let claude_user = include_bytes!("tests/fixtures/legacy/claude-user-settings.input.json");
        for (label, version) in [
            ("unknown", None),
            ("2.1.261", Some("2.1.261 (Claude Code)")),
        ] {
            claude.cli.version = version.map(str::to_owned);
            let installed = claude.merge(Some(claude_user), true).unwrap();
            assert_legacy_fixture(
                &format!("claude-settings.user.{label}.installed.json"),
                &installed,
            );
            assert_eq!(claude.merge(Some(&installed), true).unwrap(), installed);
            assert_legacy_fixture(
                &format!("claude-settings.user.{label}.removed.json"),
                &claude.merge(Some(&installed), false).unwrap(),
            );
            // No owned commands means removal must retain original whitespace too.
            assert_eq!(claude.merge(Some(claude_user), false).unwrap(), claude_user);
        }
        // A downgrade removes only newly unsupported managed commands.
        claude.cli.version = Some("2.1.261 (Claude Code)".into());
        let modern = claude.merge(Some(claude_user), true).unwrap();
        claude.cli.version = None;
        assert_legacy_fixture(
            "claude-settings.user.downgraded.json",
            &claude.merge(Some(&modern), true).unwrap(),
        );
    }

    fn fixture(agent: AgentKind) -> (TempDir, Adapter) {
        let temp = tempfile::tempdir().unwrap();
        // Exercise spaces and shell apostrophes in all owned paths.
        let home = temp.path().join("home 'with spaces");
        fs::create_dir_all(&home).unwrap();
        (temp, Adapter::new(agent, home, false))
    }
    fn seed(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    fn backup_count(path: &Path) -> usize {
        fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".se-backup-"))
            .count()
    }

    #[test]
    fn claude_install_is_idempotent_and_uninstall_byte_identical() {
        let (_temp, adapter) = fixture(AgentKind::ClaudeCode);
        let original = b"{ \"permissions\": {\"allow\":[\"Bash(ls)\"]},\n\"hooks\": {\"Stop\":[{\"matcher\":\"*\",\"hooks\":[{\"type\":\"command\",\"command\":\"echo user se-manager-hook\"}]}]}, \"theme\":\"dark\" }";
        seed(&adapter.config(), original);
        assert_eq!(adapter.install().unwrap().state, "installed");
        let once = fs::read(adapter.config()).unwrap();
        let receipt = fs::read(adapter.receipt()).unwrap();
        let value: Value = serde_json::from_slice(&once).unwrap();
        assert_eq!(value["permissions"]["allow"][0], "Bash(ls)");
        assert_eq!(
            value["hooks"]["Stop"][0]["hooks"][0]["command"],
            "echo user se-manager-hook"
        );
        assert!(value["hooks"].get("PermissionRequest").is_none());
        adapter.install().unwrap();
        assert_eq!(fs::read(adapter.config()).unwrap(), once);
        assert_eq!(fs::read(adapter.receipt()).unwrap(), receipt);
        assert_eq!(backup_count(&adapter.config()), 1);
        adapter.uninstall().unwrap();
        assert_eq!(fs::read(adapter.config()).unwrap(), original);
        assert!(!adapter.script().exists());
        adapter.uninstall().unwrap();
        assert_eq!(fs::read(adapter.config()).unwrap(), original);
    }

    #[test]
    fn qin_preserves_legacy_fields_user_rules_and_exact_bytes() {
        let (_temp, adapter) = fixture(AgentKind::QinCode);
        let original = b"# user preferences\nmodel = 'keep-me'\n[hooks]\nlevel = 'standard' # do not promote\npost_tool = 'echo legacy'\n[[hooks.rules]]\nevent = 'pre_tool'\ncommand = 'security-check --required'\nmin_level = 'minimal'\n";
        seed(&adapter.config(), original);
        adapter.install().unwrap();
        let once = fs::read(adapter.config()).unwrap();
        assert!(once.starts_with(original));
        let doc = utf8(&once)
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        assert_eq!(doc["hooks"]["level"].as_str(), Some("standard"));
        assert_eq!(doc["hooks"]["post_tool"].as_str(), Some("echo legacy"));
        let rules = doc["hooks"]["rules"].as_array_of_tables().unwrap();
        assert_eq!(rules.len(), 6);
        assert_eq!(
            rules
                .iter()
                .filter(|r| owned_qin_rule(r, &adapter.command()))
                .count(),
            5
        );
        assert!(!rules
            .iter()
            .any(|r| owned_qin_rule(r, &adapter.command())
                && r["event"].as_str() == Some("pre_tool")));
        adapter.install().unwrap();
        assert_eq!(fs::read(adapter.config()).unwrap(), once);
        adapter.uninstall().unwrap();
        assert_eq!(fs::read(adapter.config()).unwrap(), original);
    }

    #[test]
    fn no_config_roundtrip_restores_absence_for_all_agents() {
        for agent in agents() {
            let (_temp, adapter) = fixture(agent);
            assert_eq!(adapter.status().state, "agent_not_found");
            assert!(!adapter.root().exists(), "status must be read-only");
            adapter.install().unwrap();
            let once = fs::read(adapter.config()).unwrap();
            adapter.install().unwrap();
            assert_eq!(fs::read(adapter.config()).unwrap(), once);
            adapter.uninstall().unwrap();
            assert!(!adapter.config().exists());
            assert!(!adapter.script().exists());
            assert!(!adapter.receipt().exists());
            adapter.uninstall().unwrap();
        }
    }

    #[test]
    fn status_preserves_cleanup_states_without_cli_for_all_agents() {
        for agent in agents() {
            let (_temp, mut adapter) = fixture(agent);
            assert_eq!(adapter.status().state, "agent_not_found");
            adapter.cli.path = Some(adapter.home.join("fixture-cli"));
            let absent = adapter.status();
            assert_eq!(absent.state, "not_installed");
            let json = serde_json::to_value(absent).unwrap();
            assert!(json["detail"].is_null());
            assert!(json["version"].is_null());
            adapter.install().unwrap();
            assert_eq!(adapter.status().state, "installed");
            adapter.cli.path = None;
            assert_eq!(adapter.status().state, "installed");
            // Config-only installs still need the Uninstall action.
            remove_file(&adapter.script()).unwrap();
            remove_file(&adapter.receipt()).unwrap();
            assert_eq!(adapter.status().state, "outdated");
            remove_file(&adapter.config()).unwrap();
            assert_eq!(adapter.status().state, "agent_not_found");
            // Both old script-only and receipt-only remnants are cleanable.
            seed(&adapter.script(), b"#!/bin/sh\n# old managed script\n");
            assert_eq!(adapter.status().state, "outdated");
            remove_file(&adapter.script()).unwrap();
            seed(&adapter.receipt(), b"{}");
            assert_eq!(adapter.status().state, "outdated");
            remove_file(&adapter.receipt()).unwrap();
            assert_eq!(adapter.status().state, "agent_not_found");
        }
    }

    #[test]
    fn status_marks_old_config_only_versions_outdated() {
        let (_temp, adapter) = fixture(AgentKind::Pi);
        seed(
            &adapter.config(),
            format!("{PI_EXTENSION}// previous observer version\n").as_bytes(),
        );
        assert_eq!(adapter.status().state, "outdated");

        let (_temp, mut adapter) = fixture(AgentKind::ClaudeCode);
        adapter.cli.version = Some("2.1.261 (Claude Code)".into());
        adapter.install().unwrap();
        adapter.cli.version = Some("2.1.260 (Claude Code)".into());
        assert_eq!(adapter.status().state, "outdated");
        // An unsupported event alone must not disappear from ownership detection.
        let mut value: Value =
            serde_json::from_slice(&fs::read(adapter.config()).unwrap()).unwrap();
        value["hooks"]
            .as_object_mut()
            .unwrap()
            .retain(|event, _| event == "SessionEnd");
        seed(&adapter.config(), &serde_json::to_vec(&value).unwrap());
        remove_file(&adapter.script()).unwrap();
        remove_file(&adapter.receipt()).unwrap();
        assert_eq!(adapter.status().state, "outdated");
    }

    #[test]
    fn pi_v1_upgrade_is_outdated_idempotent_and_reversible() {
        for with_receipt in [false, true] {
            let (_temp, adapter) = fixture(AgentKind::Pi);
            let legacy = adapter.pi_v1_source().into_bytes();
            seed(&adapter.config(), &legacy);
            seed(&adapter.script(), adapter.script_source().as_bytes());
            if with_receipt {
                seed(
                    &adapter.receipt(),
                    &serde_json::to_vec(&Receipt {
                        original: None,
                        installed: legacy.clone(),
                    })
                    .unwrap(),
                );
            }
            assert_eq!(adapter.status().state, "outdated");
            adapter.install().unwrap();
            let installed = fs::read(adapter.config()).unwrap();
            let receipt = fs::read(adapter.receipt()).unwrap();
            assert!(installed.starts_with(PI_EXTENSION.as_bytes()));
            assert_eq!(installed, adapter.pi_source().as_bytes());
            let backup = fs::read_dir(adapter.config().parent().unwrap())
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| path.to_string_lossy().contains(".se-backup-"))
                .unwrap();
            assert_eq!(fs::read(backup).unwrap(), legacy);
            adapter.install().unwrap();
            assert_eq!(fs::read(adapter.config()).unwrap(), installed);
            assert_eq!(fs::read(adapter.receipt()).unwrap(), receipt);
            adapter.uninstall().unwrap();
            assert!(!adapter.config().exists());
            assert!(!adapter.script().exists());
            assert!(!adapter.receipt().exists());
        }
    }

    #[test]
    fn pi_version_headers_do_not_authorize_overwriting_user_edits() {
        for legacy in [false, true] {
            let (_temp, adapter) = fixture(AgentKind::Pi);
            let mut source = if legacy {
                adapter.pi_v1_source()
            } else {
                adapter.pi_source()
            };
            source.push_str("// user edit must survive verbatim\n");
            seed(&adapter.config(), source.as_bytes());
            assert_eq!(adapter.status().state, "outdated");
            assert!(adapter.install().is_err());
            assert!(adapter.uninstall().is_err());
            assert_eq!(fs::read(adapter.config()).unwrap(), source.as_bytes());
            assert!(!adapter.root().exists());
        }
    }

    #[test]
    fn status_reports_invalid_configuration_as_error() {
        for (agent, invalid) in [
            (AgentKind::ClaudeCode, "{broken"),
            (AgentKind::QinCode, "[hooks\ninvalid"),
        ] {
            let (_temp, adapter) = fixture(agent);
            seed(&adapter.config(), invalid.as_bytes());
            let status = adapter.status();
            assert_eq!(status.state, "error");
            assert!(status.detail.is_some());
            assert!(!adapter.root().exists());
        }
    }

    #[test]
    fn invalid_configs_and_colliding_owned_paths_are_never_overwritten() {
        for (agent, bad) in [
            (AgentKind::ClaudeCode, "{broken json"),
            (AgentKind::ClaudeCode, "{\"hooks\": []}"),
            (AgentKind::ClaudeCode, "{\"hooks\": {\"Stop\": false}}"),
            (AgentKind::QinCode, "[hooks\ninvalid"),
            (AgentKind::Pi, "// user extension, not ours"),
        ] {
            let (_temp, adapter) = fixture(agent);
            seed(&adapter.config(), bad.as_bytes());
            assert!(adapter.install().is_err());
            assert_eq!(fs::read(adapter.config()).unwrap(), bad.as_bytes());
            assert!(!adapter.root().exists());
        }
        let (_temp, adapter) = fixture(AgentKind::ClaudeCode);
        seed(&adapter.script(), b"#!/bin/sh\necho user owned\n");
        assert!(adapter.install().is_err());
        assert!(!adapter.config().exists());
        assert!(!adapter.receipt().exists());
    }

    #[test]
    fn claude_uninstall_preserves_post_install_user_edits_and_similar_commands() {
        let (_temp, adapter) = fixture(AgentKind::ClaudeCode);
        seed(&adapter.config(), b"{}");
        adapter.install().unwrap();
        let mut edited: Value =
            serde_json::from_slice(&fs::read(adapter.config()).unwrap()).unwrap();
        let similar = format!("{} --user-extra", adapter.command());
        edited["theme"] = json!("new-user-value");
        edited["hooks"]["Stop"]
            .as_array_mut()
            .unwrap()
            .push(json!({"matcher":"*", "hooks":[]}));
        edited["hooks"]["Stop"][0]["hooks"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type":"command","command":similar}));
        seed(&adapter.config(), &serde_json::to_vec(&edited).unwrap());
        adapter.uninstall().unwrap();
        let clean: Value = serde_json::from_slice(&fs::read(adapter.config()).unwrap()).unwrap();
        assert_eq!(clean["theme"], "new-user-value");
        assert_eq!(clean["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(clean["hooks"]["Stop"][0]["hooks"][0]["command"], similar);
        for event in adapter.claude_events() {
            assert!(!claude_has(&clean, event, &adapter.command()));
        }
    }

    #[test]
    fn reinstall_after_user_edits_does_not_resurrect_managed_hooks_on_uninstall() {
        for agent in [AgentKind::ClaudeCode, AgentKind::QinCode] {
            let (_temp, adapter) = fixture(agent);
            adapter.install().unwrap();
            let mut text = fs::read_to_string(adapter.config()).unwrap();
            if agent == AgentKind::ClaudeCode {
                let mut value: Value = serde_json::from_str(&text).unwrap();
                value["model"] = json!("user-after-install");
                text = serde_json::to_string(&value).unwrap();
            } else {
                text.push_str("\n[user]\nmodel = 'user-after-install'\n");
            }
            seed(&adapter.config(), text.as_bytes());
            adapter.install().unwrap();
            adapter.uninstall().unwrap();
            let clean = fs::read_to_string(adapter.config()).unwrap();
            assert!(clean.contains("user-after-install"));
            assert!(!clean.contains("-hook.sh"));
        }
    }

    #[test]
    fn qin_removes_only_exact_owned_rules_after_users_edit_managed_block() {
        let (_temp, adapter) = fixture(AgentKind::QinCode);
        adapter.install().unwrap();
        let text = fs::read_to_string(adapter.config()).unwrap().replace(
            "# BEGIN Se managed agent hooks v1",
            "# user changed heading",
        );
        let similar = format!("{} --mine", adapter.command());
        let text = format!("{text}\n[[hooks.rules]]\nevent = 'pre_tool'\ncommand = {}\n\n[[hooks.rules]]\nevent = 'turn_end'\ncommand = {}\n", toml_string(&adapter.command()), toml_string(&similar));
        seed(&adapter.config(), text.as_bytes());
        adapter.uninstall().unwrap();
        let clean = fs::read_to_string(adapter.config())
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        let rules = clean["hooks"]["rules"].as_array_of_tables().unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules.get(0).unwrap()["event"].as_str(), Some("pre_tool"));
        assert_eq!(
            rules.get(1).unwrap()["command"].as_str(),
            Some(similar.as_str())
        );
    }

    #[test]
    fn pi_extension_is_dedicated_observer_and_preserves_other_extensions() {
        let (_temp, adapter) = fixture(AgentKind::Pi);
        let user = adapter.config().parent().unwrap().join("user.ts");
        seed(&user, b"export default () => {};");
        adapter.install().unwrap();
        let extension = fs::read_to_string(adapter.config()).unwrap();
        for event in [
            "session_start",
            "agent_start",
            "agent_end",
            "tool_execution_start",
            "tool_execution_end",
            "tool_approval_requested",
            "tool_approval_resolved",
            "ui_prompt_start",
            "ui_prompt_end",
            "session_shutdown",
        ] {
            assert!(extension.contains(&format!("\"{event}\"")));
        }
        for event in [
            "message_end",
            "session_before_compact",
            "tool_call",
            "subagent_start",
            "subagent_end",
        ] {
            assert!(!extension.contains(&format!("\"{event}\"")));
        }
        assert!(extension.contains("JSON.stringify({ event: name, session_id: ctx?.sessionManager?.getSessionId?.(), session_file: ctx?.sessionManager?.getSessionFile?.(), tool_name: event.toolName, ok: !event.isError })"));
        assert!(!extension.contains("...event"));
        assert!(!extension.contains("ctx?.cwd"));
        assert!(!extension.contains("return { block"));
        remove_file(&adapter.receipt()).unwrap();
        adapter.uninstall().unwrap();
        assert!(!adapter.config().exists());
        assert_eq!(fs::read(&user).unwrap(), b"export default () => {};");
    }

    #[test]
    fn claude_event_gates_are_safe_for_old_unknown_and_new_versions() {
        let (_temp, mut adapter) = fixture(AgentKind::ClaudeCode);
        assert!(adapter.claude_events().contains(&"SessionStart"));
        assert!(!adapter.claude_events().contains(&"SessionEnd"));
        adapter.cli.version = Some("1.0.23 (Claude Code)".into());
        assert_eq!(
            adapter.claude_events(),
            vec!["PreToolUse", "PostToolUse", "Notification"]
        );
        adapter.cli.version = Some("2.0.44 (Claude Code)".into());
        assert!(adapter.claude_events().contains(&"SubagentStart"));
        assert!(!adapter.claude_events().contains(&"PermissionRequest"));
        adapter.cli.version = Some("2.0.56 (Claude Code)".into());
        assert!(adapter.claude_events().contains(&"PermissionRequest"));
        assert!(adapter.claude_events().contains(&"PostToolUseFailure"));
        adapter.cli.version = Some("launcher 99.0.0\n1.0.23 (Claude Code)".into());
        assert_eq!(
            adapter.claude_events(),
            vec!["PreToolUse", "PostToolUse", "Notification"]
        );
        adapter.cli.version = Some("99.0.0 (launcher, not Claude)".into());
        assert!(!adapter.claude_events().contains(&"PermissionRequest"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_refused_without_mutating_their_target() {
        let (_temp, adapter) = fixture(AgentKind::ClaudeCode);
        let target = adapter.home.join("target.json");
        seed(&target, b"{\"user\":true}");
        fs::create_dir_all(adapter.config().parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, adapter.config()).unwrap();
        assert!(adapter.install().is_err());
        assert!(adapter.uninstall().is_err());
        assert_eq!(fs::read(target).unwrap(), b"{\"user\":true}");
        assert!(!adapter.root().exists());
    }

    #[cfg(unix)]
    #[test]
    fn pi_observer_startup_idle_is_ordered_prompt_safe_and_fail_open() {
        if !Command::new("node")
            .args(["--experimental-strip-types", "--eval", ""])
            .output()
            .is_ok_and(|output| output.status.success())
        {
            eprintln!("skipping pi runtime fixture: Node with TypeScript stripping is unavailable");
            return;
        }
        let (_temp, adapter) = fixture(AgentKind::Pi);
        adapter.install().unwrap();
        let module = adapter.home.join("observer.mts");
        seed(&module, adapter.pi_source().as_bytes());
        let capture = adapter.home.join("events.jsonl");
        let bin = adapter.home.join("bin");
        // Delay idle completion so an immediately following busy event would
        // overtake it if the observer regressed to parallel curl subprocesses.
        atomic_write(
            &bin.join("curl"),
            br#"#!/bin/sh
payload=$(cat)
case "$payload" in *'"event":"idle"'*) : > "$CAPTURE_IDLE_STARTED"; sleep 0.1;; esac
printf '%s\n' "$payload" >> "$CAPTURE_EVENTS"
"#,
            0o755,
        )
        .unwrap();
        seed(
            &adapter.root().join("endpoint.env"),
            b"SE_AGENT_HOOK_PORT=38455\nSE_AGENT_HOOK_TOKEN=fixture\n",
        );
        let harness = r#"
import observer from __MODULE__;
import assert from 'node:assert/strict';
import fs from 'node:fs';
const capture = process.env.CAPTURE_EVENTS;
const idleStarted = process.env.CAPTURE_IDLE_STARTED;
const pause = (ms) => new Promise(resolve => setTimeout(resolve, ms));
const read = () => fs.readFileSync(capture, 'utf8').trim().split('\n').filter(Boolean).map(JSON.parse);
const waitFor = async (n) => {
  for (let i = 0; i < 200; i++) { if (read().length >= n) return; await pause(10); }
  assert.fail('observer did not deliver expected events: ' + fs.readFileSync(capture, 'utf8'));
};
const setup = (args = [], override = {}) => {
  fs.writeFileSync(capture, '');
  if (fs.existsSync(idleStarted)) fs.unlinkSync(idleStarted);
  process.argv = ['node', 'pi', ...args];
  Object.defineProperty(process.stdin, 'isTTY', { value: true, configurable: true });
  const hooks = new Map();
  observer({ on: (name, handler) => hooks.set(name, handler) });
  const ctx = {
    hasUI: true, isIdle: () => true, hasPendingMessages: () => false,
    sessionManager: { getSessionId: () => 'session-fixture', getSessionFile: () => '/fixture/session.jsonl' },
    ...override
  };
  const fire = (name, event = {}) => {
    assert.equal(hooks.get(name)(event, ctx), undefined, 'observer must not block/return tool decisions');
  };
  return { fire, ctx };
};
// Actual observer TS and generated shell script: safe startup reports identity
// first, then one ready event with no message/tool output disclosure.
let current = setup(['--model', 'fixture', '-c']);
current.fire('session_start', { message: 'PRIVATE', toolResult: 'PRIVATE' });
await waitFor(2);
assert.deepEqual(read().map(x => x.event), ['session_start', 'idle']);
fs.writeFileSync(process.env.STARTUP_CAPTURE, JSON.stringify(read()));
for (const payload of read()) {
  assert.equal(payload.session_id, 'session-fixture');
  assert.equal(payload.session_file, '/fixture/session.jsonl');
  assert.equal(JSON.stringify(payload).includes('PRIVATE'), false);
}
// A busy event arriving while idle is in flight must stay after idle, never
// be overwritten by that delayed ready POST.
current = setup();
current.fire('session_start');
for (let i = 0; i < 200 && !fs.existsSync(idleStarted); i++) await pause(10);
assert.equal(fs.existsSync(idleStarted), true, 'idle delivery must have started');
assert.deepEqual(read().map(x => x.event), ['session_start']);
current.fire('agent_start');
await waitFor(3);
assert.deepEqual(read().map(x => x.event), ['session_start', 'idle', 'agent_start']);
// CLI prompts start AFTER session_start, so even a currently idle ctx is not
// sufficient evidence. Cover positional, @file, option+prompt and unknown args.
for (const args of [['hello'], ['@input.md'], ['--model', 'fixture', 'hello'], ['--custom-flag'], ['--model'], ['--print'], ['--mode', 'rpc']]) {
  current = setup(args);
  current.fire('session_start');
  await waitFor(1);
  await pause(80);
  assert.deepEqual(read().map(x => x.event), ['session_start'], JSON.stringify(args));
}
// No ready transition when actual startup work, pending input, missing API,
// noninteractive mode, or shutdown/agent_start has intervened.
for (const override of [{ isIdle: () => false }, { hasPendingMessages: () => true }, { hasUI: false }, { isIdle: undefined }, { hasPendingMessages: undefined }]) {
  current = setup([], override);
  current.fire('session_start');
  await waitFor(1);
  await pause(80);
  assert.deepEqual(read().map(x => x.event), ['session_start']);
}
current = setup();
Object.defineProperty(process.stdin, 'isTTY', { value: false, configurable: true });
current.fire('session_start');
await waitFor(1);
await pause(80);
assert.deepEqual(read().map(x => x.event), ['session_start']);
for (const name of ['agent_start', 'session_shutdown']) {
  current = setup();
  current.fire('session_start');
  current.fire(name);
  await waitFor(2);
  await pause(80);
  assert.deepEqual(read().map(x => x.event), ['session_start', name]);
}
// Simulate CLI initial-prompt ordering with a gap before agent_start.
current = setup(['initial prompt']);
current.fire('session_start');
await waitFor(1);
await pause(80);
current.fire('agent_start');
current.fire('agent_end');
await waitFor(3);
assert.deepEqual(read().map(x => x.event), ['session_start', 'agent_start', 'agent_end']);
// No terminal -> no delivery. Spawn failure -> no unhandled rejection and
// subsequent observers remain non-blocking/fail-open.
current = setup();
delete process.env.SE_TERMINAL_ID;
current.fire('session_start');
await pause(80);
assert.equal(read().length, 0);
process.env.SE_TERMINAL_ID = 'term-fixture';
fs.unlinkSync(__SCRIPT__);
current.fire('session_start');
current.fire('agent_start');
await pause(100);
assert.equal(read().length, 0);
console.log('pi startup observer runtime fixtures passed');
"#
        .replace("__MODULE__", &serde_json::to_string(&module).unwrap())
        .replace("__SCRIPT__", &serde_json::to_string(&adapter.script()).unwrap());
        let runner = adapter.home.join("run.mjs");
        seed(&runner, harness.as_bytes());
        let output = Command::new("node")
            .arg("--experimental-strip-types")
            .arg(&runner)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    bin.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("CAPTURE_EVENTS", &capture)
            .env("STARTUP_CAPTURE", adapter.home.join("startup-events.json"))
            .env("CAPTURE_IDLE_STARTED", adapter.home.join("idle-started"))
            .env("SE_TERMINAL_ID", "term-fixture")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "node observer fixture failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "pi startup observer runtime fixtures passed"
        );
        let events =
            serde_json::from_slice(&fs::read(adapter.home.join("startup-events.json")).unwrap())
                .unwrap();
        crate::agent_chat::tests::assert_real_pi_observer_startup_can_send(events);
    }

    #[cfg(unix)]
    #[test]
    fn scripts_forward_raw_payload_headers_and_source_rotated_endpoint() {
        for agent in [AgentKind::ClaudeCode, AgentKind::Pi, AgentKind::QinCode] {
            let (_temp, adapter) = fixture(agent);
            adapter.install().unwrap();
            let bin = adapter.home.join("bin");
            let capture = adapter.home.join("payload");
            let args = adapter.home.join("args");
            let mock = bin.join("curl");
            atomic_write(&mock, b"#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$CAPTURE_ARGS\"\ncat > \"$CAPTURE_PAYLOAD\"\n", 0o755).unwrap();
            let raw = "{\n  \"event\": \"turn_end\", \"session_id\": \"native-session-fixture\",\n  \"cwd\": \"/work\", \"message\": \"unmodified ' payload\"\n}\n\n";
            for port in [38455, 38456] {
                seed(&adapter.root().join("endpoint.env"), format!("SE_AGENT_HOOK_PORT={port}\nSE_AGENT_HOOK_TOKEN='fixture-not-a-real-secret'\n").as_bytes());
                let mut child = Command::new(adapter.script())
                    .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                    .env("CAPTURE_PAYLOAD", &capture)
                    .env("CAPTURE_ARGS", &args)
                    .env("SE_TERMINAL_ID", "term-fixture")
                    .env("SE_PROJECT_ID", "project-fixture")
                    .env("QIN_CODE_HOOK_PAYLOAD", raw)
                    .env("QIN_CODE_HOOK_EVENT", "turn_start")
                    .env("QIN_CODE_HOOK_SESSION_ID", "fallback-session-fixture")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap();
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(raw.as_bytes())
                    .unwrap();
                let output = child.wait_with_output().unwrap();
                assert!(output.status.success());
                assert_eq!(
                    output.stdout,
                    if agent == AgentKind::ClaudeCode {
                        b"{}\n".as_slice()
                    } else {
                        b"".as_slice()
                    }
                );
                assert!(output.stderr.is_empty());
                assert_eq!(fs::read_to_string(&capture).unwrap(), raw);
                let headers = fs::read_to_string(&args).unwrap();
                for expected in [
                    "Authorization: Bearer fixture-not-a-real-secret",
                    "Content-Type: application/json",
                    "x-se-terminal-id: term-fixture",
                    "x-se-project-id: project-fixture",
                    "x-se-hook-event: turn_start",
                    "x-se-session-id: fallback-session-fixture",
                ] {
                    assert_eq!(headers.lines().filter(|line| *line == expected).count(), 1);
                }
                assert!(headers.contains(&format!(
                    "http://127.0.0.1:{port}/agent-hooks/{}",
                    adapter.name()
                )));
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn scripts_fail_open_without_endpoint_or_terminal_and_when_curl_fails() {
        let (_temp, adapter) = fixture(AgentKind::ClaudeCode);
        adapter.install().unwrap();
        let output = Command::new(adapter.script())
            .env_remove("SE_AGENT_HOOK_PORT")
            .env_remove("SE_AGENT_HOOK_TOKEN")
            .env_remove("SE_TERMINAL_ID")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"{}\n");
        seed(
            &adapter.root().join("endpoint.env"),
            b"SE_AGENT_HOOK_PORT=38455\nSE_AGENT_HOOK_TOKEN=fixture\n",
        );
        let bin = adapter.home.join("bin");
        atomic_write(&bin.join("curl"), b"#!/bin/sh\nexit 7\n", 0o755).unwrap();
        let output = Command::new(adapter.script())
            .env("PATH", &bin)
            .env("SE_TERMINAL_ID", "term-fixture")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"{}\n");
    }

    #[test]
    fn lost_receipt_reinstall_adopts_only_exact_owned_entries() {
        for agent in agents() {
            let (_temp, adapter) = fixture(agent);
            adapter.install().unwrap();
            remove_file(&adapter.receipt()).unwrap();
            adapter.install().unwrap();
            adapter.uninstall().unwrap();
            assert!(!read_optional(&adapter.config())
                .unwrap()
                .is_some_and(|bytes| utf8(&bytes).unwrap().contains("-hook.sh")));
        }
    }

    #[test]
    fn claude_session_end_requires_2_1_261() {
        let (_temp, mut adapter) = fixture(AgentKind::ClaudeCode);
        for (version, supported) in [
            (None, false),
            (Some("2.1.260 (Claude Code)"), false),
            (Some("2.1.261 (Claude Code)"), true),
            (Some("2.1.262 (Claude Code)"), true),
        ] {
            adapter.cli.version = version.map(str::to_string);
            assert_eq!(adapter.claude_events().contains(&"SessionEnd"), supported);
            adapter.install().unwrap();
            let value: Value =
                serde_json::from_slice(&fs::read(adapter.config()).unwrap()).unwrap();
            assert_eq!(value["hooks"].get("SessionEnd").is_some(), supported);
            assert_eq!(adapter.status().state, "installed");
        }
        // Downgrade removes the managed SessionEnd registration again.
        adapter.cli.version = Some("2.1.260 (Claude Code)".into());
        adapter.install().unwrap();
        let value: Value = serde_json::from_slice(&fs::read(adapter.config()).unwrap()).unwrap();
        assert!(value["hooks"].get("SessionEnd").is_none());
    }

    #[test]
    fn claude_downgrade_removes_unsupported_managed_event_keys() {
        let (_temp, mut adapter) = fixture(AgentKind::ClaudeCode);
        let original = b"{\"hooks\":{\"Stop\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"echo user\"}]}]}}";
        seed(&adapter.config(), original);
        adapter.cli.version = Some("2.0.56 (Claude Code)".into());
        adapter.install().unwrap();
        adapter.cli.version = Some("1.0.64 (Claude Code)".into());
        adapter.install().unwrap();
        let value: Value = serde_json::from_slice(&fs::read(adapter.config()).unwrap()).unwrap();
        assert!(value["hooks"].get("PermissionRequest").is_none());
        assert!(value["hooks"].get("SessionEnd").is_none());
        assert_eq!(
            value["hooks"]["Stop"][0]["hooks"][0]["command"],
            "echo user"
        );
        adapter.uninstall().unwrap();
        assert_eq!(fs::read(adapter.config()).unwrap(), original);
    }

    #[test]
    fn preview_is_read_only_and_returns_only_managed_changes() {
        for agent in agents() {
            let (_temp, adapter) = fixture(agent);
            let preview = adapter.preview().unwrap();
            assert!(preview.contains(&adapter.config().to_string_lossy().to_string()));
            assert!(preview.contains("+ "));
            assert!(!adapter.root().exists());
            assert!(!adapter.config().exists());
        }
        let (_temp, adapter) = fixture(AgentKind::ClaudeCode);
        let original = b"{\"env\":{\"API_KEY\":\"fixture-user-secret\"},\"hooks\":{\"Stop\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"user-secret-command\"}]}]}}";
        seed(&adapter.config(), original);
        let preview = adapter.preview().unwrap();
        assert!(!preview.contains("fixture-user-secret"));
        assert!(!preview.contains("user-secret-command"));
        assert_eq!(fs::read(adapter.config()).unwrap(), original);
        assert!(!adapter.root().exists());
        adapter.install().unwrap();
        let installed = fs::read(adapter.config()).unwrap();
        assert!(adapter
            .preview()
            .unwrap()
            .contains("No configuration changes required"));
        assert_eq!(fs::read(adapter.config()).unwrap(), installed);
    }

    #[test]
    fn serialized_status_uses_the_public_snake_case_contract() {
        let (_temp, adapter) = fixture(AgentKind::QinCode);
        let value = serde_json::to_value(adapter.status()).unwrap();
        assert_eq!(value["agent"], "qin-code");
        let keys: Vec<_> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "agent",
                "state",
                "config_path",
                "detail",
                "cli_path",
                "version"
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn qin_install_reports_effective_level_without_changing_it() {
        for (level, detail) in [
            ("none", "disabled"),
            ("minimal", "post_tool requires"),
            ("standard", "observer tiers are enabled"),
        ] {
            let (_temp, mut adapter) = fixture(AgentKind::QinCode);
            let cli = adapter.home.join("qin-code-fixture");
            atomic_write(
                &cli,
                format!("#!/bin/sh\nprintf '%s' '{{\"level\":\"{level}\"}}'\n").as_bytes(),
                0o755,
            )
            .unwrap();
            adapter.cli.path = Some(cli);
            let original = format!("[hooks]\nlevel = '{level}'\n");
            seed(&adapter.config(), original.as_bytes());
            let status = adapter.install().unwrap();
            assert!(status.detail.unwrap().contains(detail));
            assert!(fs::read_to_string(adapter.config())
                .unwrap()
                .starts_with(&original));
            adapter.uninstall().unwrap();
            assert_eq!(fs::read_to_string(adapter.config()).unwrap(), original);
        }
    }

    #[cfg(unix)]
    #[test]
    fn diagnostic_commands_are_bounded_and_json_verification_is_non_destructive() {
        let (_temp, adapter) = fixture(AgentKind::QinCode);
        let cli = adapter.home.join("qin-code-fixture");
        // Login-shell PATH initialization is independently bounded and cached;
        // do not charge its first-use latency to the child timeout assertion.
        crate::pty::env_refresh::apply_fresh_path(&mut HashMap::new());
        let natural_runtime = Duration::from_secs(10);
        let scheduling_budget = natural_runtime / 2;
        atomic_write(
            &cli,
            format!("#!/bin/sh\nexec sleep {}\n", natural_runtime.as_secs()).as_bytes(),
            0o755,
        )
        .unwrap();
        let before = Instant::now();
        assert!(bounded_output(&cli, &[], Duration::from_millis(50)).is_none());
        assert!(
            before.elapsed() < scheduling_budget,
            "timed-out child took {:?}; must return well before its {:?} natural completion",
            before.elapsed(),
            natural_runtime,
        );
        // A successful parent can leave a grandchild holding stdout open.
        // The reader must return without waiting for that inherited pipe's EOF.
        atomic_write(
            &cli,
            format!(
                "#!/bin/sh\nsleep {} &\nprintf '%s' \"$!\"\n",
                natural_runtime.as_secs()
            )
            .as_bytes(),
            0o755,
        )
        .unwrap();
        let before = Instant::now();
        let output = bounded_output(&cli, &[], scheduling_budget).unwrap();
        let elapsed = before.elapsed();
        let grandchild: libc::pid_t = std::str::from_utf8(&output).unwrap().parse().unwrap();
        assert!(grandchild > 0);
        // SAFETY: this positive PID was emitted by the fixture's own shell.
        unsafe { libc::kill(grandchild, libc::SIGTERM) };
        assert!(
            elapsed < scheduling_budget,
            "inherited stdout delayed return by {elapsed:?}; must not wait for the grandchild's {natural_runtime:?} completion",
        );
        atomic_write(
            &cli,
            b"#!/bin/sh\nprintf '{\"level\":\"minimal\",\"rules\":[]}'\n",
            0o755,
        )
        .unwrap();
        // This is a successful JSON control, not a timeout assertion. Concurrent
        // subprocess load can delay even this printf-only child beyond one second;
        // keep its scheduling allowance separate from the 50 ms deadline above.
        let success_control_budget = Duration::from_secs(5);
        let before = Instant::now();
        let output = bounded_output(
            &cli,
            &["hooks", "status", "--json"],
            success_control_budget,
        )
        .unwrap_or_else(|| {
            panic!(
                "valid JSON success control failed after {:?} (scheduling budget {success_control_budget:?}, CLI {cli:?}); timeout behavior is asserted separately",
                before.elapsed(),
            )
        });
        assert_eq!(
            serde_json::from_slice::<Value>(&output).expect("success control must emit valid JSON"),
            json!({"level": "minimal", "rules": []}),
        );
        assert!(!adapter.config().exists());
    }
}
