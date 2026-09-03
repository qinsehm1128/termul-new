use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::conversation::{DirectoryPermissions, DurableFileSystem};

pub const SCHEDULED_TASK_SKILL_TEMPLATE_VERSION: u32 = 2;

/// File name of the provisioner's own record of what it owns.
const MANIFEST_FILE: &str = "managed-skills.json";

/// `schema_version` values this build knows how to read.
///
/// 2 is what this build writes since T-A21 moved the ownership key; 1 is what a
/// pre-rename build left on disk and must still be read. A manifest from a
/// version outside this set is skipped with a log line rather than silently
/// treated as absent.
const READABLE_MANIFEST_SCHEMA_VERSIONS: &[u32] = &[1, 2];

/// The managed skill's directory name.
///
/// A function rather than a `const` because the value comes from the brand
/// seam, which is thread-local by necessity. Every caller reaches it on the
/// thread that asked for the provisioning: `provision` is a plain synchronous
/// call from `conversation::application::backfill_managed_skills` and from
/// `acp::manager`, and neither hands it to `spawn_blocking` (FORBID-07).
#[must_use]
pub fn scheduled_task_skill_name() -> &'static str {
    crate::brand::canonical().skill_name
}

/// Markers that mean "this app wrote this file".
///
/// Both names count. A `SKILL.md` a shipped build already wrote carries the
/// pre-rename marker, and a provisioner that only recognised the current one
/// would treat its own file as user-owned, bail out with `UnmanagedCollision`,
/// and — because `backfill_managed_skills` logs and swallows that error —
/// silently stop maintaining the skill for anyone who had been using it. The
/// legacy marker is only ever *matched*, never written (FORBID-04).
fn managed_markers() -> [&'static str; 2] {
    [
        crate::brand::canonical().skill_marker,
        crate::brand::LEGACY.skill_marker,
    ]
}

/// The skill body this build writes.
///
/// Built at call time rather than held as a `const` for the same reason as
/// [`scheduled_task_skill_name`]: three of its lines — the front-matter `name`,
/// the ownership key, and the HTML marker — are brand contracts and must come
/// from the seam, not from a literal a repo-wide rename would rewrite in step
/// with every assertion about it.
fn skill_template() -> String {
    let canonical = crate::brand::canonical();
    format!(
        r#"---
name: {skill_name}
description: Draft safe Se-level AI schedules for review from any Conversation.
metadata:
  {ownership_key}: true
  templateVersion: {template_version}
---
{marker}

# Se Scheduled Tasks

Use Se's `scheduled_task_*` tools when a user asks for recurring, delayed,
or one-time autonomous work.

1. Clarify the timezone, schedule, ACP, execution directory, expected result,
   and minimum permissions. A project association is optional; never require
   one before drafting a task.
2. Call `scheduled_task_preview` and show the next execution times. Never
   calculate timezone or DST behavior yourself.
3. Create or update a draft. A draft does not run until the user confirms the
   exact revision and hash in Se.
4. Never place secrets, credentials, environment variables, or hidden model
   reasoning in a task prompt. Explain observable decisions through normal
   messages, plans, and tool calls.
5. Prefer narrow permissions and an explicit failure result. Do not add
   automatic retries for work that can have side effects.
6. `scheduled_task_pause` affects future occurrences only. It does not cancel
   a Conversation that has already started.

Se stores full execution traces in the isolated child Conversation. The task
run ledger contains only status, hashes, timestamps, usage, and Conversation
links.
"#,
        skill_name = canonical.skill_name,
        ownership_key = canonical.skill_manifest_key,
        template_version = SCHEDULED_TASK_SKILL_TEMPLATE_VERSION,
        marker = canonical.skill_marker,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillProvider {
    Claude,
    Qwen,
    Cline,
    CrossTool,
}

impl SkillProvider {
    fn relative_root(self) -> &'static str {
        match self {
            Self::Claude => ".claude/skills",
            Self::Qwen => ".qwen/skills",
            Self::Cline => ".cline/skills",
            Self::CrossTool => ".agents/skills",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedSkillManifestV1 {
    pub schema_version: u32,
    // `rename_all = "camelCase"` makes this identifier the persisted JSON key,
    // so renaming it *is* an on-disk schema change — which is why T-A21 moved it
    // together with the `schema_version` bump below rather than with the
    // repo-internal renames in the D batch.
    //
    // The alias is a permanent compatibility read, not a transitional one:
    // `deny_unknown_fields` means every manifest a pre-rename build left in a
    // user's workspace is rejected *outright* the moment it disappears, and the
    // provisioner then stops recognising the skill files it owns. Serde
    // attributes take literals only, so this line cannot read
    // `brand::LEGACY.skill_manifest_key`; it is the single named FORBID-04
    // exception, and `tests/legacy_brand_skill_manifest.rs` compares the two.
    #[serde(alias = "managedByTermul")]
    pub managed_by_se_manager: bool,
    pub skill_name: String,
    pub template_version: u32,
    pub sha256: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionedSkill {
    pub primary_path: PathBuf,
    pub mirrored_paths: Vec<PathBuf>,
    pub sha256: String,
}

#[derive(Debug)]
pub enum SkillProvisionError {
    InvalidWorkspace(String),
    UnmanagedCollision(PathBuf),
    Io(std::io::Error),
    Durable(crate::conversation::DurableFsError),
    Json(serde_json::Error),
}

impl std::fmt::Display for SkillProvisionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidWorkspace(detail) => {
                write!(formatter, "invalid skill workspace: {detail}")
            }
            Self::UnmanagedCollision(path) => write!(
                formatter,
                "refusing to overwrite an unmanaged skill at '{}'",
                path.display()
            ),
            Self::Io(error) => write!(formatter, "skill provisioning io error: {error}"),
            Self::Durable(error) => write!(
                formatter,
                "skill provisioning durable write failed: {error}"
            ),
            Self::Json(error) => write!(formatter, "skill manifest serialization failed: {error}"),
        }
    }
}

impl std::error::Error for SkillProvisionError {}

impl From<std::io::Error> for SkillProvisionError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<crate::conversation::DurableFsError> for SkillProvisionError {
    fn from(value: crate::conversation::DurableFsError) -> Self {
        Self::Durable(value)
    }
}

impl From<serde_json::Error> for SkillProvisionError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Debug, Default)]
pub struct ConversationSkillProvisioner {
    durable_fs: DurableFileSystem,
}

impl ConversationSkillProvisioner {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn provision(
        &self,
        workspace_cwd: &Path,
        agent_config_id: &str,
    ) -> Result<ProvisionedSkill, SkillProvisionError> {
        if !workspace_cwd.is_absolute() {
            return Err(SkillProvisionError::InvalidWorkspace(
                "workspace path must be absolute".to_string(),
            ));
        }
        if !workspace_cwd.is_dir() {
            return Err(SkillProvisionError::InvalidWorkspace(
                "workspace directory does not exist".to_string(),
            ));
        }

        let provider = provider_for_agent(agent_config_id);
        let primary = skill_path(workspace_cwd, SkillProvider::CrossTool);
        let mut paths = vec![primary.clone()];
        let native = skill_path(workspace_cwd, provider);
        if native != primary {
            paths.push(native);
        }

        // What a previous run recorded as ours, read before this run overwrites
        // it. After the rename that record is the pre-rename manifest, sitting
        // in the pre-rename workspace directory.
        let previously_claimed = read_manifest(workspace_cwd)
            .map(|manifest| manifest.paths)
            .unwrap_or_default();

        let template = skill_template();
        let sha256 = sha256(template.as_bytes());
        for path in &paths {
            self.write_managed_skill(path, &template, &previously_claimed)?;
        }

        // Deactivation, which is the one thing M-12 needs beyond a copy: a
        // skill file this app wrote under the previous name is still on disk
        // and still loadable, so the manifest must stop naming it as the live
        // one. The file itself is never deleted or rewritten (FORBID-05) — it
        // simply drops out of the claim below.
        let claimed_now: Vec<String> = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let deactivated: Vec<&String> = previously_claimed
            .iter()
            .filter(|previous| !claimed_now.contains(previous))
            .collect();
        if !deactivated.is_empty() {
            log::info!(
                "[scheduled-task-skill] boundary=deactivated_previous_claim count={} paths={:?}",
                deactivated.len(),
                deactivated
            );
        }

        let manifest = ManagedSkillManifestV1 {
            // 2 since T-A21 moved the ownership key. `READABLE_MANIFEST_SCHEMA_VERSIONS`
            // still accepts 1, so a manifest a pre-rename build wrote is read;
            // the reverse is out of contract — see
            // `legacy_brand_skill_manifest::downgrading_to_an_older_binary_is_a_known_and_accepted_loss`.
            schema_version: 2,
            managed_by_se_manager: true,
            skill_name: scheduled_task_skill_name().to_string(),
            template_version: SCHEDULED_TASK_SKILL_TEMPLATE_VERSION,
            sha256: sha256.clone(),
            paths: claimed_now,
        };
        // M-08: the manifest lives in this app's per-repository workspace
        // directory, so it moves with that directory's name. Nothing is
        // relocated — a manifest a pre-rename build left behind stays where it
        // is and is still read (see `read_manifest`).
        let manifest_dir = workspace_cwd.join(crate::brand::canonical().workspace_dir);
        self.durable_fs
            .create_dir_durable(&manifest_dir, DirectoryPermissions::PrivateOwnerOnly)?;
        self.durable_fs.replace_bytes(
            &manifest_dir.join(MANIFEST_FILE),
            &serde_json::to_vec_pretty(&manifest)?,
        )?;
        log::info!(
            "[scheduled-task-skill] boundary=provisioned provider={:?} version={} mirror_count={}",
            provider,
            SCHEDULED_TASK_SKILL_TEMPLATE_VERSION,
            paths.len()
        );
        Ok(ProvisionedSkill {
            primary_path: primary,
            mirrored_paths: paths,
            sha256,
        })
    }

    /// Write the skill, refusing to touch a file this app did not write.
    ///
    /// A file counts as ours on two independent pieces of evidence: it carries
    /// one of [`managed_markers`], or a manifest we wrote earlier named it.
    /// The second is what makes the manifest read path do something: a user who
    /// edited the marker out of a file we own would otherwise turn it into a
    /// permanent `UnmanagedCollision`.
    fn write_managed_skill(
        &self,
        path: &Path,
        template: &str,
        previously_claimed: &[String],
    ) -> Result<(), SkillProvisionError> {
        if let Ok(existing) = fs::read_to_string(path) {
            let ours = managed_markers()
                .iter()
                .any(|marker| existing.contains(marker))
                || previously_claimed
                    .iter()
                    .any(|claimed| Path::new(claimed) == path);
            if !ours {
                return Err(SkillProvisionError::UnmanagedCollision(path.to_path_buf()));
            }
            if existing == template {
                return Ok(());
            }
        }
        let parent = path.parent().ok_or_else(|| {
            SkillProvisionError::InvalidWorkspace("skill path has no parent".to_string())
        })?;
        self.durable_fs
            .create_dir_durable(parent, DirectoryPermissions::PrivateOwnerOnly)?;
        self.durable_fs.replace_bytes(path, template.as_bytes())?;
        Ok(())
    }
}

/// Read the manifest a previous run wrote, from the current workspace directory
/// or from the one a pre-rename build used.
///
/// Until this existed the manifest was write-only: `provision` serialized it and
/// nothing under `src/skills/` ever read it back, which meant any compatibility
/// alias or `schema_version` check on it could not possibly take effect. The
/// read is what gives those a code path.
///
/// Never fatal. A manifest that is missing, unreadable, malformed or written by
/// a build this one does not understand degrades to "no previous claim" — the
/// markers in the files themselves are the other, independent evidence — but it
/// is logged rather than swallowed, because a rejected manifest means the app
/// is about to stop recognising files it owns.
fn read_manifest(workspace_cwd: &Path) -> Option<ManagedSkillManifestV1> {
    let mut roots = vec![crate::brand::canonical().workspace_dir];
    if crate::brand::LEGACY.workspace_dir != roots[0] {
        roots.push(crate::brand::LEGACY.workspace_dir);
    }
    for root in roots {
        let path = workspace_cwd.join(root).join(MANIFEST_FILE);
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        match serde_json::from_slice::<ManagedSkillManifestV1>(&bytes) {
            Ok(manifest)
                if READABLE_MANIFEST_SCHEMA_VERSIONS.contains(&manifest.schema_version) =>
            {
                return Some(manifest);
            }
            Ok(manifest) => log::warn!(
                "[scheduled-task-skill] boundary=manifest_schema_unsupported path={} schema_version={}",
                path.display(),
                manifest.schema_version
            ),
            Err(error) => log::warn!(
                "[scheduled-task-skill] boundary=manifest_unreadable path={} error={error}",
                path.display()
            ),
        }
    }
    None
}

#[must_use]
pub fn provider_for_agent(agent_config_id: &str) -> SkillProvider {
    let id = agent_config_id.to_ascii_lowercase();
    if id.contains("claude") {
        SkillProvider::Claude
    } else if id.contains("qwen") {
        SkillProvider::Qwen
    } else if id.contains("cline") {
        SkillProvider::Cline
    } else {
        SkillProvider::CrossTool
    }
}

fn skill_path(workspace_cwd: &Path, provider: SkillProvider) -> PathBuf {
    workspace_cwd
        .join(provider.relative_root())
        .join(scheduled_task_skill_name())
        .join("SKILL.md")
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn workspace(label: &str) -> PathBuf {
        let root = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("se-manager-skill-{label}-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn provider_mapping_matches_supported_native_roots() {
        assert_eq!(provider_for_agent("claude-acp"), SkillProvider::Claude);
        assert_eq!(provider_for_agent("qwen-code"), SkillProvider::Qwen);
        assert_eq!(provider_for_agent("cline"), SkillProvider::Cline);
        assert_eq!(provider_for_agent("codex-acp"), SkillProvider::CrossTool);
        assert_eq!(provider_for_agent("gemini"), SkillProvider::CrossTool);
    }

    #[test]
    fn provisions_cross_tool_and_native_mirror_inside_managed_workspace() {
        let root = workspace("mirror");
        let receipt = ConversationSkillProvisioner::new()
            .provision(&root, "claude-acp")
            .unwrap();
        assert!(receipt.primary_path.is_file());
        assert!(root
            .join(".claude/skills")
            .join(scheduled_task_skill_name())
            .join("SKILL.md")
            .is_file());
        assert!(root
            .join(crate::brand::canonical().workspace_dir)
            .join("managed-skills.json")
            .is_file());
        assert!(receipt
            .mirrored_paths
            .iter()
            .all(|path| path.starts_with(&root)));
        let _ = fs::remove_dir_all(root);
    }

    /// Seed a manifest on disk as raw JSON.
    ///
    /// Deliberately not built from `ManagedSkillManifestV1` — the point is to
    /// read bytes a *different* build wrote, and a struct literal here would
    /// only ever produce shapes this build already agrees with.
    fn seed_manifest(workspace_root: &Path, dir: &str, schema_version: u32, claimed: &str) {
        let manifest = serde_json::json!({
            "schemaVersion": schema_version,
            crate::brand::LEGACY.skill_manifest_key: true,
            "skillName": crate::brand::LEGACY.skill_name,
            "templateVersion": 2,
            "sha256": "0".repeat(64),
            "paths": [claimed],
        });
        let manifest_dir = workspace_root.join(dir);
        fs::create_dir_all(&manifest_dir).unwrap();
        fs::write(
            manifest_dir.join(MANIFEST_FILE),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
    }

    /// M-12 — the manifest read accepts both workspace directories and both
    /// schema versions, and rejects anything else out loud.
    ///
    /// The compatibility read only matters for a manifest a *pre-rename* build
    /// wrote, which sits in the pre-rename workspace directory. Reading only
    /// the current directory would find nothing and silently drop the record of
    /// which files this app owns.
    #[test]
    fn manifest_read_accepts_both_workspace_dirs_and_both_schema_versions() {
        let root = workspace("manifest-read");
        let _guard = crate::brand::override_canonical(crate::brand::BrandCanonical {
            workspace_dir: ".se-manager",
            ..crate::brand::DEFAULT_CANONICAL
        });
        let legacy_dir = crate::brand::LEGACY.workspace_dir;
        assert_ne!(
            crate::brand::canonical().workspace_dir,
            legacy_dir,
            "the injection did not take, so this proves nothing"
        );

        // Only the pre-rename directory has one: it must still be found.
        seed_manifest(&root, legacy_dir, 1, "/claimed/v1/SKILL.md");
        let found = read_manifest(&root).expect("a v1 manifest in the pre-rename directory");
        assert_eq!(found.paths, vec!["/claimed/v1/SKILL.md".to_string()]);

        // The current directory wins once it has one, and v2 is readable too.
        seed_manifest(
            &root,
            crate::brand::canonical().workspace_dir,
            2,
            "/claimed/v2/SKILL.md",
        );
        let found = read_manifest(&root).expect("a v2 manifest in the current directory");
        assert_eq!(found.schema_version, 2);
        assert_eq!(found.paths, vec!["/claimed/v2/SKILL.md".to_string()]);

        // A version this build does not know is not silently treated as v1.
        seed_manifest(
            &root,
            crate::brand::canonical().workspace_dir,
            99,
            "/claimed/v99/SKILL.md",
        );
        let found = read_manifest(&root).expect("falls back to the pre-rename manifest");
        assert_eq!(
            found.paths,
            vec!["/claimed/v1/SKILL.md".to_string()],
            "an unreadable schema_version must be skipped, not accepted"
        );

        let _ = fs::remove_dir_all(root);
    }

    /// A file this app recorded as its own is still its own after the marker
    /// has been edited out of it.
    ///
    /// This is what makes the read path load-bearing rather than decorative:
    /// without it the manifest could be read and thrown away and every test
    /// above would still pass.
    #[test]
    fn a_file_named_by_the_previous_manifest_is_claimed_even_without_a_marker() {
        let root = workspace("manifest-claim");
        let skill = root
            .join(".agents/skills")
            .join(scheduled_task_skill_name())
            .join("SKILL.md");
        fs::create_dir_all(skill.parent().unwrap()).unwrap();
        fs::write(&skill, "ours, but the marker was edited out").unwrap();

        // Without a claim it is an unmanaged collision …
        assert!(matches!(
            ConversationSkillProvisioner::new().provision(&root, "codex"),
            Err(SkillProvisionError::UnmanagedCollision(found)) if found == skill
        ));

        // … and with one it is re-templated.
        seed_manifest(
            &root,
            crate::brand::canonical().workspace_dir,
            1,
            &skill.to_string_lossy(),
        );
        ConversationSkillProvisioner::new()
            .provision(&root, "codex")
            .expect("a file the previous manifest claimed is ours");
        assert_eq!(fs::read_to_string(&skill).unwrap(), skill_template());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn refuses_to_replace_an_unmanaged_skill() {
        let root = workspace("collision");
        let path = root
            .join(".agents/skills")
            .join(scheduled_task_skill_name())
            .join("SKILL.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "user owned").unwrap();
        assert!(matches!(
            ConversationSkillProvisioner::new().provision(&root, "codex"),
            Err(SkillProvisionError::UnmanagedCollision(found)) if found == path
        ));
        let _ = fs::remove_dir_all(root);
    }
}
