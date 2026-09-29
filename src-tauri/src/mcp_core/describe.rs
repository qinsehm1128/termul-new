//! One-line "what is this MCP server for" descriptions.
//!
//! Agents pick a server by these lines (the grouped tool descriptions and
//! `list_mcp_servers`), so they are short and about when to use the server,
//! not a list of its tools. A model writes them from the server's tool list;
//! the user can edit or replace any of them. They are stored in
//! `~/<workspace dir>/mcp-descriptions.json` keyed by the server's config id,
//! which the gateway reloads on change.

use std::{collections::BTreeMap, path::Path};

use crate::ai_channels::{self, AiError, ChatRequest};

pub const MAX_DESCRIPTION_CHARS: usize = 240;
const MAX_TOOLS: usize = 60;
const MAX_TOOL_DESCRIPTION_CHARS: usize = 200;
const MAX_ANSWER_TOKENS: u32 = 400;

const SYSTEM_PROMPT: &str = "You write the one-line description an AI agent reads to decide \
whether to use an MCP server. Reply with the description only: one sentence, at most 25 words \
(or 40 characters in Chinese, Japanese or Korean), saying what the server is for and when to \
use it. No preamble, no quotes, no markdown, no list of tool names.";

pub fn prompt(server: &str, language: &str, tools: &[(String, String)]) -> String {
    let mut text = format!("Write it in {language}.\nServer: {server}\nTools:\n");
    for (name, description) in tools.iter().take(MAX_TOOLS) {
        let description = description
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(MAX_TOOL_DESCRIPTION_CHARS)
            .collect::<String>();
        text.push_str(&format!("- {name}: {description}\n"));
    }
    if tools.len() > MAX_TOOLS {
        text.push_str(&format!("- … and {} more\n", tools.len() - MAX_TOOLS));
    }
    text
}

/// First non-empty line, without wrapping quotes or markdown, capped.
pub fn clean(answer: &str) -> String {
    let line = answer
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let line = line
        .trim_start_matches(['#', '-', '*', ' '])
        .trim_matches(['"', '\'', '`', '“', '”', '「', '」'])
        .trim();
    if line.chars().count() <= MAX_DESCRIPTION_CHARS {
        line.to_owned()
    } else {
        let cut = line.chars().take(MAX_DESCRIPTION_CHARS).collect::<String>();
        format!("{}…", cut.trim_end())
    }
}

/// Ask the model configured for MCP summaries to describe one server.
pub async fn describe(
    resolved: &ai_channels::Resolved,
    server: &str,
    language: &str,
    tools: &[(String, String)],
) -> Result<String, AiError> {
    let user = prompt(server, language, tools);
    let answer = ai_channels::client::complete(
        &resolved.channel,
        &resolved.key,
        &resolved.model,
        ChatRequest {
            system: SYSTEM_PROMPT,
            user: &user,
            max_tokens: MAX_ANSWER_TOKENS,
        },
    )
    .await?;
    let description = clean(&answer);
    if description.is_empty() {
        return Err(AiError::Malformed("the description is empty".into()));
    }
    Ok(description)
}

/// Replace the stored descriptions. Empty texts are dropped.
pub fn save(config_root: &Path, descriptions: &BTreeMap<String, String>) -> Result<(), String> {
    let kept = descriptions
        .iter()
        .map(|(id, text)| (id.clone(), text.trim().to_owned()))
        .filter(|(_, text)| !text.is_empty())
        .map(|(id, text)| {
            let text = if text.chars().count() > MAX_DESCRIPTION_CHARS {
                text.chars().take(MAX_DESCRIPTION_CHARS).collect()
            } else {
                text
            };
            (id, text)
        })
        .collect::<BTreeMap<_, _>>();
    let bytes = serde_json::to_vec_pretty(&kept).map_err(|error| error.to_string())?;
    let path = super::service::descriptions_path(config_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    crate::acp::atomic_file::replace(&path, &bytes).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_keeps_one_plain_line() {
        assert_eq!(
            clean("\n\n\"Search library docs.\"\nMore"),
            "Search library docs."
        );
        assert_eq!(clean("# 查询最新库文档"), "查询最新库文档");
        let long = "x".repeat(300);
        assert_eq!(clean(&long).chars().count(), MAX_DESCRIPTION_CHARS + 1);
    }

    #[test]
    fn prompt_lists_tools_compactly_and_caps_them() {
        let tools = (0..65)
            .map(|index| (format!("t{index}"), "does\n  things".to_owned()))
            .collect::<Vec<_>>();
        let text = prompt("context7", "Chinese", &tools);
        assert!(text.starts_with("Write it in Chinese.\nServer: context7\n"));
        assert!(text.contains("- t0: does things\n"));
        assert!(!text.contains("- t60:"));
        assert!(text.ends_with("- … and 5 more\n"));
    }

    #[test]
    fn saving_drops_empty_texts_and_the_gateway_reads_the_rest() {
        let root = tempfile::tempdir().unwrap();
        let descriptions = BTreeMap::from([
            ("id-a".to_owned(), "  Docs lookup ".to_owned()),
            ("id-b".to_owned(), "   ".to_owned()),
        ]);
        save(root.path(), &descriptions).unwrap();
        assert_eq!(
            super::super::service::load_descriptions(root.path()),
            BTreeMap::from([("id-a".to_owned(), "Docs lookup".to_owned())])
        );
    }
}
