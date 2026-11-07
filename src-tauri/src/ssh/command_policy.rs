//! Pure, bounded command-text guardrail. This is deliberately not a shell sandbox.
use super::automation::{AutomationError, AutomationResult, SshPolicy};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const MAX_COMMAND_BYTES: usize = 32 * 1024;
pub const MAX_LAYER_RULES: usize = 32;
pub const MAX_POLICY_RULES: usize = 128;
pub const MAX_PATTERN_BYTES: usize = 512;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommandDenyKind {
    Exact,
    Contains,
    Regex,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandDenyRule {
    pub id: String,
    pub kind: CommandDenyKind,
    pub pattern: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommandRuleSource {
    Global,
    Profile,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CommandPolicyMatch {
    pub id: String,
    pub source: CommandRuleSource,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandPolicyPreview {
    pub outcome: String,
    pub matches: Vec<CommandPolicyMatch>,
    pub policy_revision: u64,
    pub draft: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<String>,
}

pub fn invalid_policy() -> AutomationError {
    AutomationError::new("POLICY_INVALID", "Invalid host command policy")
}
pub fn validate_command(command: &str) -> AutomationResult<()> {
    if command.is_empty() || command.len() > MAX_COMMAND_BYTES || command.contains('\0') {
        return Err(AutomationError::invalid(
            "Command is empty, oversized or contains NUL",
        ));
    }
    Ok(())
}
#[derive(Clone)]
struct CompiledRule {
    id: String,
    matcher: Matcher,
}
#[derive(Clone)]
enum Matcher {
    Exact(String),
    Contains(String),
    Regex(Regex),
}
impl CompiledRule {
    fn matches(&self, text: &str) -> bool {
        match &self.matcher {
            Matcher::Exact(p) => text == p,
            Matcher::Contains(p) => text.contains(p),
            Matcher::Regex(p) => p.is_match(text),
        }
    }
}
/// Immutable; the host publishes this together with the grants/revision after durable CAS.
#[derive(Clone)]
pub struct CompiledCommandPolicy {
    global: Vec<CompiledRule>,
    profiles: HashMap<String, Vec<CompiledRule>>,
}
impl CompiledCommandPolicy {
    pub fn compile(policy: &SshPolicy) -> AutomationResult<Self> {
        let total = policy.command_deny.len()
            + policy
                .profiles
                .values()
                .map(|p| p.command_deny.len())
                .sum::<usize>();
        if total > MAX_POLICY_RULES {
            return Err(invalid_policy());
        }
        fn layer(rules: &[CommandDenyRule]) -> AutomationResult<Vec<CompiledRule>> {
            if rules.len() > MAX_LAYER_RULES {
                return Err(invalid_policy());
            }
            let mut ids = HashSet::new();
            rules
                .iter()
                .map(|r| {
                    // IDs are returned in previews; keep them bounded, inert identifiers.
                    if r.id.is_empty()
                        || r.id.len() > 128
                        || !r
                            .id
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
                        || !ids.insert(&r.id)
                        || r.pattern.is_empty()
                        || r.pattern.len() > MAX_PATTERN_BYTES
                        || r.pattern.contains('\0')
                    {
                        return Err(invalid_policy());
                    }
                    let matcher = match r.kind {
                        CommandDenyKind::Exact => Matcher::Exact(r.pattern.clone()),
                        CommandDenyKind::Contains => Matcher::Contains(r.pattern.clone()),
                        CommandDenyKind::Regex => Matcher::Regex(
                            RegexBuilder::new(&r.pattern)
                                .size_limit(256 * 1024)
                                .dfa_size_limit(256 * 1024)
                                .nest_limit(32)
                                .build()
                                .map_err(|_| invalid_policy())?,
                        ),
                    };
                    Ok(CompiledRule {
                        id: r.id.clone(),
                        matcher,
                    })
                })
                .collect()
        }
        Ok(Self {
            global: layer(&policy.command_deny)?,
            profiles: policy
                .profiles
                .iter()
                .map(|(id, p)| Ok((id.clone(), layer(&p.command_deny)?)))
                .collect::<AutomationResult<_>>()?,
        })
    }
    pub fn evaluate(
        &self,
        profile_id: &str,
        command: &str,
    ) -> AutomationResult<Vec<CommandPolicyMatch>> {
        validate_command(command)?;
        Ok(self
            .global
            .iter()
            .filter(|r| r.matches(command))
            .map(|r| CommandPolicyMatch {
                id: r.id.clone(),
                source: CommandRuleSource::Global,
            })
            .chain(
                self.profiles
                    .get(profile_id)
                    .into_iter()
                    .flatten()
                    .filter(|r| r.matches(command))
                    .map(|r| CommandPolicyMatch {
                        id: r.id.clone(),
                        source: CommandRuleSource::Profile,
                    }),
            )
            .collect())
    }
    pub fn check(&self, profile_id: &str, command: &str) -> AutomationResult<()> {
        if !self.evaluate(profile_id, command)?.is_empty() {
            return Err(AutomationError::new(
                "command_denied",
                "Command denied by host policy",
            ));
        }
        Ok(())
    }
}
