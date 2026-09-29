//! Short "what is this MCP server for" descriptions.
//!
//! Agents pick a server by these lines (the grouped tool descriptions and
//! `list_mcp_servers`), so they are about when to use the server, not a list
//! of its tools. A model writes them from the server's whole tool list, under
//! settings the user can change (`mcp-describe.json`); the user can also edit
//! or replace any line. They are stored in
//! `~/<workspace dir>/mcp-descriptions.json` keyed by the server's config id,
//! which the gateway reloads on change.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::ai_channels::{self, AiError, ChatRequest};

/// Longest description stored, whoever wrote it.
pub const MAX_STORED_CHARS: usize = 2000;
const MAX_TOOL_DESCRIPTION_CHARS: usize = 10_000;
const MAX_PROMPT_CHARS: usize = 20_000;
const MIN_CHARS: usize = 20;

/// Replaced in the prompt by [`DescribeSettings::max_chars`].
pub const MAX_CHARS_PLACEHOLDER: &str = "{maxChars}";

const DEFAULT_PROMPT: &str = "You write the description an AI agent reads to decide whether \
to use an MCP server. Reply with the description only: one or two sentences, at most {maxChars} \
characters, saying what the server is for, when to use it and the main things it can do, in the \
words an agent would look for. No preamble, no quotes, no markdown, no list of tool names.";

/// How descriptions are written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DescribeSettings {
    /// System prompt; `{maxChars}` becomes `max_chars`.
    pub prompt: String,
    /// Longest description kept; a longer answer is cut.
    pub max_chars: usize,
    /// Each tool description in the prompt is cut to this; 0 keeps it whole.
    pub tool_description_chars: usize,
}

impl Default for DescribeSettings {
    fn default() -> Self {
        Self {
            prompt: DEFAULT_PROMPT.to_owned(),
            max_chars: 300,
            tool_description_chars: 0,
        }
    }
}

impl DescribeSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.prompt.trim().is_empty() {
            return Err("the prompt is empty".into());
        }
        if self.prompt.chars().count() > MAX_PROMPT_CHARS {
            return Err(format!(
                "the prompt is longer than {MAX_PROMPT_CHARS} characters"
            ));
        }
        if !(MIN_CHARS..=MAX_STORED_CHARS).contains(&self.max_chars) {
            return Err(format!(
                "the description length must be {MIN_CHARS}–{MAX_STORED_CHARS} characters"
            ));
        }
        if self.tool_description_chars > MAX_TOOL_DESCRIPTION_CHARS {
            return Err(format!(
                "the tool description length must be 0–{MAX_TOOL_DESCRIPTION_CHARS} characters"
            ));
        }
        Ok(())
    }

    pub fn system_prompt(&self) -> String {
        self.prompt
            .trim()
            .replace(MAX_CHARS_PLACEHOLDER, &self.max_chars.to_string())
    }
}

pub fn settings_path(config_root: &Path) -> PathBuf {
    config_root
        .join(crate::brand::canonical().workspace_dir)
        .join("mcp-describe.json")
}

/// The stored settings, or the defaults when there are none.
pub fn load_settings(config_root: &Path) -> Result<DescribeSettings, String> {
    let path = settings_path(config_root);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DescribeSettings::default());
        }
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let settings: DescribeSettings =
        serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    settings
        .validate()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(settings)
}

pub fn save_settings(config_root: &Path, settings: &DescribeSettings) -> Result<(), String> {
    settings.validate()?;
    let bytes = serde_json::to_vec_pretty(settings).map_err(|error| error.to_string())?;
    write(&settings_path(config_root), &bytes)
}

/// The user message: the answer language, the server and every tool.
pub fn prompt(
    server: &str,
    language: &str,
    tools: &[(String, String)],
    tool_description_chars: usize,
) -> String {
    let mut text = format!("Write it in {language}.\nServer: {server}\nTools:\n");
    for (name, description) in tools {
        let description = description.split_whitespace().collect::<Vec<_>>().join(" ");
        let description = if tool_description_chars == 0 {
            description
        } else {
            description.chars().take(tool_description_chars).collect()
        };
        text.push_str(&format!("- {name}: {description}\n"));
    }
    text
}

/// The answer as one plain line, without wrapping quotes or markdown, at
/// most `max_chars` characters.
pub fn clean(answer: &str, max_chars: usize) -> String {
    let line = answer
        .lines()
        .map(|line| line.trim().trim_start_matches(['#', '-', '*', ' ']).trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let line = line
        .trim_matches(['"', '\'', '`', '“', '”', '「', '」'])
        .trim();
    if line.chars().count() <= max_chars {
        line.to_owned()
    } else {
        let cut = line
            .chars()
            .take(max_chars.saturating_sub(1))
            .collect::<String>();
        format!("{}…", cut.trim_end())
    }
}

/// Ask the model configured for MCP summaries to describe one server.
pub async fn describe(
    resolved: &ai_channels::Resolved,
    settings: &DescribeSettings,
    server: &str,
    language: &str,
    tools: &[(String, String)],
) -> Result<String, AiError> {
    let system = settings.system_prompt();
    let user = prompt(server, language, tools, settings.tool_description_chars);
    let answer = ai_channels::client::complete(
        &resolved.channel,
        &resolved.key,
        &resolved.model,
        ChatRequest {
            system: &system,
            user: &user,
            max_tokens: None,
        },
    )
    .await?;
    let description = clean(&answer, settings.max_chars);
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
            let text = if text.chars().count() > MAX_STORED_CHARS {
                text.chars().take(MAX_STORED_CHARS).collect()
            } else {
                text
            };
            (id, text)
        })
        .collect::<BTreeMap<_, _>>();
    let bytes = serde_json::to_vec_pretty(&kept).map_err(|error| error.to_string())?;
    write(&super::service::descriptions_path(config_root), &bytes)
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    crate::acp::atomic_file::replace(path, bytes).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_joins_lines_and_cuts_at_the_configured_length() {
        assert_eq!(
            clean(
                "\n\n\"Search library docs.\n\nUse it for API questions.\"\n",
                300
            ),
            "Search library docs. Use it for API questions."
        );
        assert_eq!(clean("# 查询最新库文档", 300), "查询最新库文档");
        let long = "x".repeat(500);
        let cut = clean(&long, 400);
        assert_eq!(cut.chars().count(), 400);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn prompt_lists_every_tool_and_cuts_descriptions_only_when_asked() {
        let tools = (0..150)
            .map(|index| (format!("t{index}"), "does\n  many things".to_owned()))
            .collect::<Vec<_>>();
        let text = prompt("context7", "Chinese", &tools, 0);
        assert!(text.starts_with("Write it in Chinese.\nServer: context7\n"));
        assert!(text.contains("- t0: does many things\n"));
        assert!(text.ends_with("- t149: does many things\n"));
        assert!(prompt("context7", "Chinese", &tools, 4).contains("- t0: does\n"));
    }

    #[test]
    fn settings_default_until_saved_and_fill_the_length_into_the_prompt() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            load_settings(root.path()).unwrap(),
            DescribeSettings::default()
        );
        assert!(DescribeSettings::default()
            .system_prompt()
            .contains("at most 300 characters"));

        let settings = DescribeSettings {
            prompt: "Describe it in {maxChars} characters.".into(),
            max_chars: 120,
            tool_description_chars: 80,
        };
        save_settings(root.path(), &settings).unwrap();
        let loaded = load_settings(root.path()).unwrap();
        assert_eq!(loaded, settings);
        assert_eq!(loaded.system_prompt(), "Describe it in 120 characters.");
    }

    #[tokio::test]
    async fn describing_sends_the_settings_and_leaves_the_answer_uncapped() {
        use crate::ai_channels::{client::tests, AiApi};
        let (base, seen) = tests::provider(vec![(
            "/v1/chat/completions",
            serde_json::json!({"choices": [{"message": {"content": "查询库文档。\n适合 API 问题。"}}]}),
        )])
        .await;
        let resolved = ai_channels::Resolved {
            channel: tests::channel(AiApi::OpenaiCompletions, format!("{base}/v1")),
            key: "k".into(),
            model: "m".into(),
        };
        let settings = DescribeSettings {
            prompt: "At most {maxChars}.".into(),
            max_chars: 20,
            tool_description_chars: 0,
        };
        let tools = [("query".to_owned(), "Query docs".to_owned())];
        let text = describe(&resolved, &settings, "context7", "Chinese", &tools)
            .await
            .unwrap();
        assert_eq!(text, "查询库文档。 适合 API 问题。");
        let (_, _, body) = seen.lock().unwrap()[0].clone();
        assert!(body.get("max_tokens").is_none(), "{body}");
        assert_eq!(body["messages"][0]["content"], "At most 20.");
        assert!(body["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains("- query: Query docs"));
    }

    #[test]
    fn settings_out_of_range_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let base = DescribeSettings::default();
        for bad in [
            DescribeSettings {
                prompt: "  ".into(),
                ..base.clone()
            },
            DescribeSettings {
                max_chars: MAX_STORED_CHARS + 1,
                ..base.clone()
            },
            DescribeSettings {
                max_chars: MIN_CHARS - 1,
                ..base.clone()
            },
            DescribeSettings {
                tool_description_chars: MAX_TOOL_DESCRIPTION_CHARS + 1,
                ..base.clone()
            },
        ] {
            assert!(save_settings(root.path(), &bad).is_err(), "{bad:?}");
        }
        assert!(!settings_path(root.path()).exists());

        std::fs::create_dir_all(settings_path(root.path()).parent().unwrap()).unwrap();
        std::fs::write(settings_path(root.path()), br#"{"maxChars":5}"#).unwrap();
        assert!(load_settings(root.path()).is_err());
    }

    #[test]
    fn saving_drops_empty_texts_and_the_gateway_reads_the_rest() {
        let root = tempfile::tempdir().unwrap();
        let descriptions = BTreeMap::from([
            ("id-a".to_owned(), "  Docs lookup ".to_owned()),
            ("id-b".to_owned(), "   ".to_owned()),
            ("id-c".to_owned(), "y".repeat(MAX_STORED_CHARS + 10)),
        ]);
        save(root.path(), &descriptions).unwrap();
        assert_eq!(
            super::super::service::load_descriptions(root.path()),
            BTreeMap::from([
                ("id-a".to_owned(), "Docs lookup".to_owned()),
                ("id-c".to_owned(), "y".repeat(MAX_STORED_CHARS)),
            ])
        );
    }
}
