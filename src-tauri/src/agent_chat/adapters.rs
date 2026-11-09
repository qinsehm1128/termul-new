//! Stateless native transcript -> ACP adapters. No file IO or indexed-text truncation.
//!
//! `user_text` is deliberately raw and is ONLY for local prompt-echo matching.
//! Never publish it: `blocks` and `tool_calls` are the redacted wire payloads.
use crate::agent_hooks::AgentKind;
use crate::memory_index::redact::{is_sensitive_name, redact};
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedEntry {
    pub id: String,
    pub role: String,
    pub blocks: Vec<Value>,
    pub tool_calls: Vec<Value>,
    pub user_text: Option<String>,
}

/// Parse exactly one decoded JSONL record; offsets provide deterministic fallback IDs.
/// Unknown bookkeeping records are ignored; recognized malformed messages fail closed.
pub fn parse_record(
    agent: AgentKind,
    value: &Value,
    offset: u64,
) -> Result<Vec<ParsedEntry>, String> {
    let parsed = match agent {
        AgentKind::ClaudeCode => parse_claude(value, offset),
        AgentKind::Pi => parse_pi(value, offset),
        AgentKind::QinCode => parse_qin(value, offset),
    };
    // Do not expose serde errors, offending fields, or secret-bearing source data.
    parsed.map_err(|()| format!("Malformed {} message record", agent.as_str()))
}

fn parse_claude(value: &Value, offset: u64) -> Result<Vec<ParsedEntry>, ()> {
    let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
    if !matches!(kind, "user" | "assistant") {
        return Ok(Vec::new());
    }
    let message = value.get("message").filter(|v| v.is_object()).ok_or(())?;
    if message.get("role").and_then(Value::as_str) != Some(kind) {
        return Err(());
    }
    split_message(message, record_id(value, "uuid", "claude", offset), false)
}

fn parse_pi(value: &Value, offset: u64) -> Result<Vec<ParsedEntry>, ()> {
    if value.get("type").and_then(Value::as_str) != Some("message") {
        return Ok(Vec::new());
    }
    let message = value.get("message").filter(|v| v.is_object()).ok_or(())?;
    split_message(message, record_id(value, "id", "pi", offset), false)
}

fn parse_qin(value: &Value, offset: u64) -> Result<Vec<ParsedEntry>, ()> {
    // Actual native schema: SessionJournalEntry { meta, append_messages, ... }.
    // cea-jcode/crates/jcode-base/src/session/journal.rs and
    // cea-jcode/crates/jcode-session-types/src/lib.rs (StoredMessage).
    // There is no `type: "append_messages"` event wrapper in the journal.
    let Some(messages) = value.get("append_messages") else {
        return Ok(Vec::new());
    };
    let messages = messages.as_array().ok_or(())?;
    let mut entries = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        let id = if message
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty())
        {
            record_id(message, "id", "qin", offset)
        } else {
            format!("qin:{offset}:{index}")
        };
        entries.extend(split_message(message, id, true)?);
    }
    // A journal append can contain both the call and its result. Retain call
    // metadata on those result updates; across records the consumer merges IDs.
    let mut calls: HashMap<String, Value> = HashMap::new();
    for entry in &mut entries {
        for call in &mut entry.tool_calls {
            let id = call["toolCallId"].as_str().ok_or(())?.to_owned();
            if call.get("rawInput").is_none() {
                if let Some(previous) = calls.get(&id) {
                    for key in ["title", "kind", "rawInput"] {
                        if let Some(value) = previous.get(key) {
                            call[key] = value.clone();
                        }
                    }
                }
            }
            calls.insert(id, call.clone());
        }
    }
    Ok(entries)
}

fn record_id(value: &Value, key: &str, vendor: &str, offset: u64) -> String {
    let id = value
        .get(key)
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty());
    match id {
        Some(id) => format!("{vendor}:{}", redact(id).text),
        None => format!("{vendor}:{offset}"),
    }
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str, ()> {
    value.get(key).and_then(Value::as_str).ok_or(())
}

fn split_message(message: &Value, id: String, native_qin: bool) -> Result<Vec<ParsedEntry>, ()> {
    let role = required_string(message, "role")?;
    if role == "toolResult" && !native_qin {
        let call = tool_result(message, "toolCallId", "isError", message.get("toolName"))?;
        return Ok(vec![ParsedEntry {
            id,
            role: "agent".into(),
            blocks: Vec::new(),
            tool_calls: vec![call],
            user_text: None,
        }]);
    }
    let base_role = match role {
        "user" => "user",
        "assistant" => "agent",
        _ => return Ok(Vec::new()),
    };
    let content = message.get("content").ok_or(())?;
    let blocks = match content {
        Value::Array(blocks) => blocks.clone(),
        Value::String(text) if !native_qin => vec![json!({"type":"text", "text":text})],
        _ => return Err(()),
    };
    let mut entries = Vec::new();
    for block in &blocks {
        let kind = required_string(block, "type")?;
        match kind {
            "text" => {
                let text = required_string(block, "text")?;
                let entry = entry_for(&mut entries, &id, base_role);
                entry
                    .blocks
                    .push(json!({"type":"text", "text":redact(text).text}));
                if base_role == "user" {
                    let raw = entry.user_text.get_or_insert_with(String::new);
                    if !raw.is_empty() {
                        raw.push('\n');
                    }
                    raw.push_str(text);
                }
            }
            "thinking" | "anthropic_thinking" | "reasoning" | "reasoning_trace" => {
                let key = if matches!(kind, "thinking" | "anthropic_thinking") {
                    "thinking"
                } else {
                    "text"
                };
                let text = required_string(block, key)?;
                entry_for(&mut entries, &id, "thought")
                    .blocks
                    .push(json!({"type":"text", "text":redact(text).text}));
            }
            // serde's snake_case spelling of OpenAIReasoning in native qin.
            "open_a_i_reasoning" => {
                for key in ["summary", "reasoning_text"] {
                    if let Some(lines) = block.get(key) {
                        for line in lines.as_array().ok_or(())? {
                            let text = line.as_str().ok_or(())?;
                            entry_for(&mut entries, &id, "thought")
                                .blocks
                                .push(json!({"type":"text", "text":redact(text).text}));
                        }
                    }
                }
            }
            "tool_use" | "toolCall" => {
                let call_id = required_string(block, "id")?;
                let name = required_string(block, "name")?;
                if call_id.is_empty() || name.is_empty() {
                    return Err(());
                }
                let input_key = if kind == "toolCall" {
                    "arguments"
                } else {
                    "input"
                };
                let input = block.get(input_key).ok_or(())?;
                let call = redact_value(&json!({
                    "toolCallId":call_id, "title":name, "kind":tool_kind(name),
                    "status":"in_progress", "rawInput":input, "content":[]
                }));
                entry_for(&mut entries, &id, "agent").tool_calls.push(call);
            }
            "tool_result" => {
                let call = tool_result(block, "tool_use_id", "is_error", None)?;
                entry_for(&mut entries, &id, "agent").tool_calls.push(call);
            }
            // Retain supported non-text payloads, but never expose signed or
            // encrypted reasoning metadata as visible prose.
            "image" | "audio" | "document" | "resource" | "resource_link" => {
                entry_for(&mut entries, &id, base_role)
                    .blocks
                    .push(redact_value(block));
            }
            _ => {} // Future/opaque bookkeeping blocks are not chat prose.
        }
    }
    Ok(entries)
}

fn entry_for<'a>(entries: &'a mut Vec<ParsedEntry>, id: &str, role: &str) -> &'a mut ParsedEntry {
    if entries.last().is_none_or(|entry| entry.role != role) {
        let index = entries.len();
        entries.push(ParsedEntry {
            id: if index == 0 {
                id.to_owned()
            } else {
                format!("{id}:{index}")
            },
            role: role.into(),
            blocks: Vec::new(),
            tool_calls: Vec::new(),
            user_text: None,
        });
    }
    entries.last_mut().expect("entry was just ensured")
}

fn tool_result(
    value: &Value,
    id_key: &str,
    error_key: &str,
    name: Option<&Value>,
) -> Result<Value, ()> {
    let id = required_string(value, id_key)?;
    if id.is_empty() {
        return Err(());
    }
    let is_error = match value.get(error_key) {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        _ => return Err(()),
    };
    let output = value.get("content").ok_or(())?;
    let blocks = match output {
        Value::String(text) => vec![json!({"type":"text", "text":text})],
        Value::Array(blocks) => {
            for block in blocks {
                let kind = required_string(block, "type")?;
                if kind == "text" {
                    required_string(block, "text")?;
                }
            }
            blocks.clone()
        }
        _ => return Err(()),
    };
    let title = match name {
        Some(name) => name.as_str().ok_or(())?,
        None => "Tool result",
    };
    let content: Vec<Value> = blocks
        .into_iter()
        .map(|block| json!({"type":"content", "content":block}))
        .collect();
    Ok(redact_value(&json!({
        "toolCallId":id, "title":title, "kind":tool_kind(title),
        "status":if is_error { "failed" } else { "completed" },
        "content":content, "rawOutput":output
    })))
}

fn tool_kind(name: &str) -> &'static str {
    match name.to_ascii_lowercase().as_str() {
        "read" | "read_file" => "read",
        "edit" | "write" | "write_file" | "apply_patch" | "multiedit" | "notebookedit" => "edit",
        "bash" | "shell" | "execute" | "exec" => "execute",
        "grep" | "glob" | "find" | "search" | "agentgrep" | "code_symbols" => "search",
        "webfetch" | "websearch" | "fetch" => "fetch",
        _ => "other",
    }
}

/// Preserve JSON structure and length; mask both text secrets and credential
/// fields (including nested/non-string credential values). Never serialize then
/// redact/reparse: quotes/newlines in arbitrary tool payloads must stay valid JSON.
fn redact_value(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(redact(text).text),
        Value::Array(items) => Value::Array(items.iter().map(redact_value).collect()),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| {
                    let mut normalized = String::new();
                    for ch in key.chars() {
                        if ch.is_ascii_uppercase() && !normalized.is_empty() {
                            normalized.push('_');
                        }
                        normalized.push(ch.to_ascii_lowercase());
                    }
                    let safe_value = if is_sensitive_name(key)
                        || is_sensitive_name(&normalized)
                        || matches!(
                            key.to_ascii_lowercase().as_str(),
                            "authorization" | "proxy-authorization" | "proxy_authorization"
                        ) {
                        Value::String("[redacted:credential]".into())
                    } else {
                        redact_value(value)
                    };
                    (redact(key).text, safe_value)
                })
                .collect(),
        ),
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(agent: AgentKind, source: &str) -> Vec<ParsedEntry> {
        let mut offset = 0;
        let mut entries = Vec::new();
        for line in source.lines() {
            let value = serde_json::from_str(line).expect("fixture JSON");
            entries.extend(parse_record(agent, &value, offset).expect("valid fixture record"));
            offset += line.len() as u64 + 1;
        }
        entries
    }

    fn wire(entries: &[ParsedEntry]) -> String {
        serde_json::to_string(&entries.iter().map(|entry| {
            json!({"id":entry.id, "role":entry.role, "blocks":entry.blocks, "toolCalls":entry.tool_calls})
        }).collect::<Vec<_>>()).unwrap()
    }

    fn assert_safe(entries: &[ParsedEntry]) {
        let published = wire(entries);
        for secret in [
            "fixture-secret",
            "thinking-secret",
            "input-secret",
            "result-secret",
            "opaque-not-visible",
            "opaque-header-secret",
        ] {
            assert!(!published.contains(secret), "wire leaked {secret}");
        }
        assert!(entries[0]
            .user_text
            .as_ref()
            .unwrap()
            .contains("fixture-secret"));
        assert!(published.contains("redacted"));
        assert!(entries.iter().any(|entry| entry.role == "thought"));
        assert!(entries
            .iter()
            .filter(|entry| entry.role != "user")
            .all(|entry| entry.user_text.is_none()));
    }

    #[test]
    fn claude_fixture_maps_thought_and_tool_result() {
        let entries = fixture(AgentKind::ClaudeCode, include_str!("fixtures/claude.jsonl"));
        assert_safe(&entries);
        assert_eq!(
            entries.iter().map(|e| e.role.as_str()).collect::<Vec<_>>(),
            ["user", "thought", "agent", "agent"]
        );
        assert_eq!(entries[2].tool_calls[0]["toolCallId"], "claude-call-1");
        assert_eq!(entries[2].tool_calls[0]["kind"], "read");
        assert_eq!(entries[3].tool_calls[0]["status"], "completed");
        assert_eq!(entries[3].tool_calls[0]["content"][0]["type"], "content");
        assert_eq!(
            entries[3].tool_calls[0]["content"][0]["content"]["type"],
            "text"
        );
    }

    #[test]
    fn pi_fixture_maps_native_tool_call_and_failed_result() {
        let entries = fixture(AgentKind::Pi, include_str!("fixtures/pi.jsonl"));
        assert_safe(&entries);
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[2].tool_calls[0]["kind"], "execute");
        assert_eq!(entries[2].tool_calls[0]["status"], "in_progress");
        assert_eq!(entries[3].tool_calls[0]["toolCallId"], "pi-call-1");
        assert_eq!(entries[3].tool_calls[0]["title"], "bash");
        assert_eq!(entries[3].tool_calls[0]["status"], "failed");
    }

    #[test]
    fn qin_native_journal_maps_multiple_messages_and_enriches_result() {
        let entries = fixture(AgentKind::QinCode, include_str!("fixtures/qin.jsonl"));
        assert_safe(&entries);
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].id, "qin:qin-user-1");
        assert_eq!(entries[1].role, "thought");
        assert_eq!(entries[1].blocks.len(), 4);
        assert_eq!(entries[3].tool_calls[0]["toolCallId"], "qin-call-1");
        assert_eq!(entries[3].tool_calls[0]["title"], "read");
        assert_eq!(entries[3].tool_calls[0]["kind"], "read");
        assert_eq!(
            entries[3].tool_calls[0]["rawInput"],
            entries[2].tool_calls[0]["rawInput"]
        );
        assert_eq!(entries[3].tool_calls[0]["status"], "completed");
    }

    #[test]
    fn malformed_known_messages_fail_generically_without_secret_details() {
        let cases = [
            (
                AgentKind::ClaudeCode,
                json!({"type":"user", "message":{"role":"user", "content":null}}),
            ),
            (
                AgentKind::ClaudeCode,
                json!({"type":"assistant", "message":{"role":"assistant", "content":[{"type":"text","text":{"password":"private-secret"}}]}}),
            ),
            (
                AgentKind::ClaudeCode,
                json!({"type":"assistant", "message":{"role":"user", "content":[]}}),
            ),
            (AgentKind::Pi, json!({"type":"message"})),
            (
                AgentKind::Pi,
                json!({"type":"message", "message":{"role":"assistant", "content":[{"type":"toolCall","id":"x","name":"bash"}]}}),
            ),
            (
                AgentKind::Pi,
                json!({"type":"message", "message":{"role":"toolResult","content":[],"toolCallId":"x","isError":"private-secret"}}),
            ),
            (AgentKind::QinCode, json!({"append_messages":null})),
            (
                AgentKind::QinCode,
                json!({"append_messages":[{"role":"user","content":[]}, {"role":"assistant","content":[{"type":"tool_result","content":"private-secret"}]}]}),
            ),
        ];
        for (agent, value) in cases {
            let error = parse_record(agent, &value, 0).unwrap_err();
            assert_eq!(
                error,
                format!("Malformed {} message record", agent.as_str())
            );
            assert!(!error.contains("private-secret"));
        }
    }

    #[test]
    fn unknown_bookkeeping_and_future_blocks_are_skipped() {
        for agent in [AgentKind::ClaudeCode, AgentKind::Pi, AgentKind::QinCode] {
            for value in [
                json!(null),
                json!({"type":"summary","summary":"password=private-secret"}),
                json!({"type":"session"}),
            ] {
                assert!(parse_record(agent, &value, 0).unwrap().is_empty());
            }
        }
        let future = json!({"type":"message","message":{"role":"assistant","content":[{"type":"future_metadata","value":"password=private-secret"}]}});
        assert!(parse_record(AgentKind::Pi, &future, 0).unwrap().is_empty());
    }

    #[test]
    fn fallback_ids_are_deterministic_unique_and_offsets_are_not_truncated() {
        let value = json!({"append_messages":[{"role":"user","content":[{"type":"text","text":"one"}]},{"role":"assistant","content":[{"type":"text","text":"two"}]}]});
        let entries = parse_record(AgentKind::QinCode, &value, u64::MAX).unwrap();
        assert_eq!(entries[0].id, format!("qin:{}:0", u64::MAX));
        assert_ne!(entries[0].id, entries[1].id);
        assert_eq!(
            entries,
            parse_record(AgentKind::QinCode, &value, u64::MAX).unwrap()
        );
    }

    #[test]
    fn nested_tool_fields_and_auth_headers_are_redacted_without_breaking_json() {
        let input = json!({"safe":"quotes: \"hello\"\nline", "apiKey":"opaque-secret-1", "Authorization":"opaque-secret-2", "nested":[{"password":{"value":"opaque-secret-3"}},{"access_token":["opaque-secret-4"]}], "command":"curl -H 'Authorization: Bearer abcdefghijklmnop'"});
        let record = json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","id":"call","name":"bash","arguments":input}]}});
        let entries = parse_record(AgentKind::Pi, &record, 0).unwrap();
        let wire = wire(&entries);
        for secret in [
            "opaque-secret-1",
            "opaque-secret-2",
            "opaque-secret-3",
            "opaque-secret-4",
            "abcdefghijklmnop",
        ] {
            assert!(!wire.contains(secret));
        }
        assert_eq!(
            entries[0].tool_calls[0]["rawInput"]["safe"],
            "quotes: \"hello\"\nline"
        );
    }

    #[test]
    fn all_vendors_keep_full_prose_thought_tool_input_and_output() {
        let long = format!(
            "{}TAIL survives intact",
            "ordinary prose with spaces. ".repeat(6000)
        );
        for agent in [AgentKind::ClaudeCode, AgentKind::Pi, AgentKind::QinCode] {
            let tool = match agent {
                AgentKind::Pi => {
                    json!({"type":"toolCall","id":"large-call","name":"bash","arguments":{"command":long}})
                }
                _ => {
                    json!({"type":"tool_use","id":"large-call","name":"bash","input":{"command":long}})
                }
            };
            let message = json!({"role":"assistant","content":[{"type":"thinking","thinking":long},{"type":"text","text":long},tool]});
            let record = match agent {
                AgentKind::ClaudeCode => json!({"type":"assistant","message":message}),
                AgentKind::Pi => json!({"type":"message","message":message}),
                AgentKind::QinCode => json!({"append_messages":[message]}),
            };
            let entries = parse_record(agent, &record, 42).unwrap();
            assert_eq!(entries[0].blocks[0]["text"], long);
            assert_eq!(entries[1].blocks[0]["text"], long);
            assert_eq!(entries[1].tool_calls[0]["rawInput"]["command"], long);
            let message = match agent {
                AgentKind::Pi => {
                    json!({"role":"toolResult","toolCallId":"large-call","content":[{"type":"text","text":long}]})
                }
                _ => {
                    json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"large-call","content":long}]})
                }
            };
            let record = match agent {
                AgentKind::ClaudeCode => json!({"type":"user","message":message}),
                AgentKind::Pi => json!({"type":"message","message":message}),
                AgentKind::QinCode => json!({"append_messages":[message]}),
            };
            let entries = parse_record(agent, &record, 43).unwrap();
            assert_eq!(
                entries[0].tool_calls[0]["content"][0]["content"]["text"],
                long
            );
        }
    }
}
