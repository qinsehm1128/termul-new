//! T-H15 — the managed scheduled-tasks skill file across the rename.
//!
//! # Why this reads the skill off disk instead of inlining a literal
//!
//! `src/skills/provisioner.rs` used to carry the managed marker twice — once as
//! a `MANAGED_MARKER` constant and once again, character for character, inside
//! a `SKILL_TEMPLATE` constant. An assertion written as
//! `assert_eq!(MANAGED_MARKER, "<!-- managed-by-termul:... -->")` is a copy of
//! the constant it checks: a repo-wide `sed 's/termul/se-manager/g'` rewrites
//! both operands together and the suite stays green, while every
//! `SKILL.md` a shipped build already wrote — carrying the *old* marker — stops
//! being recognised as ours. The provisioner then refuses to touch it
//! (`UnmanagedCollision`), and a user who has been running scheduled tasks for
//! months silently stops getting the skill.
//!
//! T-M12 replaced both constants with `skill_template()` and
//! `managed_markers()`, which read `brand::canonical()` and — for the marker —
//! also accept `brand::LEGACY.skill_marker` so a pre-rename file is still
//! claimed. All four ledger entries below are struck.
//!
//! So the subject here is `tests/fixtures/legacy-brand/user-skills/termul-scheduled-tasks.md`,
//! a sha256-frozen copy of what a pre-rename build wrote, and the expectation
//! comes from `brand::override_canonical`. Neither operand can be rewritten by
//! an edit to the other.
//!
//! # Reachability note (reported as a Wave-4 seam gap)
//!
//! `mod skills` is private to `termul_manager_lib`, so
//! `ConversationSkillProvisioner::provision` cannot be called from `tests/`.
//! The nearest public seam is the one production itself uses to reach it:
//! `ConversationApplicationService::open_conversation` →
//! `backfill_managed_skills` (`src/conversation/application.rs:563`). Every
//! test below builds a real Conversation through `ConversationBootstrap` and
//! opens it, so the provisioner runs exactly as it does in the app.
//!
//! `backfill_managed_skills` is a synchronous call inside the `open_conversation`
//! future, so on a current-thread runtime it executes on the test thread and the
//! thread-local brand override is in force throughout.
//!
//! Note that `backfill_managed_skills` swallows provisioning errors by design
//! (opening durable history must not fail because a workspace is missing).
//! An `UnmanagedCollision` is therefore invisible in the return value — the
//! filesystem is the only observable, which is what these tests assert on.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tempfile::TempDir;

use termul_manager_lib::brand::{self, BrandCanonical, BrandOverrideGuard, DEFAULT_CANONICAL};
use termul_manager_lib::conversation::{
    AgentBindingResult, BootstrapOutcome, ConversationBootstrap, ConversationCreationService,
    HostConversationRoots, MigrationHostMode, PrepareConversationRequest,
};
use termul_manager_lib::{ConversationId, ExecutionTarget};

/// Skill identity the app writes *after* the rename.
fn post_rename() -> BrandCanonical {
    BrandCanonical {
        workspace_dir: ".se-manager",
        skill_name: "se-manager-scheduled-tasks",
        skill_marker: "<!-- managed-by-se-manager:se-manager-scheduled-tasks -->",
        ..DEFAULT_CANONICAL
    }
}

fn legacy_skill_body() -> String {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/legacy-brand/user-skills/termul-scheduled-tasks.md");
    std::fs::read_to_string(&fixture)
        .unwrap_or_else(|e| panic!("read {} failed: {e}", fixture.display()))
}

fn skill_md(workspace: &Path, skill_name: &str) -> PathBuf {
    workspace
        .join(".agents")
        .join("skills")
        .join(skill_name)
        .join("SKILL.md")
}

fn write_skill(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().expect("skill path has a parent"))
        .expect("create skill directory");
    std::fs::write(path, body).expect("seed the legacy skill file");
}

/// A real Conversation whose visible workspace already carries a legacy skill.
struct OpenedWorkspace {
    _temp: TempDir,
    workspace_cwd: PathBuf,
}

/// Build a Conversation through the real bootstrap, let `seed` populate its
/// workspace, then open it under the post-rename brand so the production
/// provisioner runs.
fn open_conversation_under_post_rename_brand(
    seed: impl FnOnce(&Path, &BrandOverrideGuard),
) -> OpenedWorkspace {
    open_conversation_under(post_rename(), seed)
}

/// The same, under an explicit set of canonical values.
///
/// The runtime is built explicitly (rather than via `#[tokio::test]`) because
/// `ConversationBootstrap::run` is synchronous and internally blocks on a
/// future — running it inside an active runtime is a deadlock waiting to
/// happen. It is therefore driven before `block_on` is entered. The runtime is
/// single-threaded so `open_conversation` resolves on this thread, where the
/// brand override lives.
///
/// The guard is installed even when `canonical` is the shipped set, so the
/// control test below exercises the identical code path as the others and
/// differs only in which values are in force.
fn open_conversation_under(
    canonical: BrandCanonical,
    seed: impl FnOnce(&Path, &BrandOverrideGuard),
) -> OpenedWorkspace {
    let temp = TempDir::new().unwrap();
    let outcome: BootstrapOutcome = ConversationBootstrap::run(
        HostConversationRoots::desktop(temp.path().join("state"), temp.path().join("workspaces")),
        MigrationHostMode::Desktop,
    )
    .expect("conversation bootstrap over an empty state root");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let creation: Arc<ConversationCreationService> = Arc::clone(&outcome.creation);
    let prepared = runtime
        .block_on(creation.prepare_conversation(PrepareConversationRequest::new(
            ExecutionTarget::Workspace,
        )))
        .expect("prepare a conversation");
    let workspace_cwd = PathBuf::from(&prepared.workspace_cwd);
    let conversation_id: ConversationId = prepared.conversation_id;

    // `backfill_managed_skills` returns early when the Conversation has no
    // agent binding, so the provisioner would never run without this.
    runtime
        .block_on(creation.complete_agent_binding(
            conversation_id,
            AgentBindingResult {
                agent_session_id: "harness-session".to_string(),
                runtime_agent_id: "codex-acp".to_string(),
                // `backfill_managed_skills` strips `config:` and passes the rest
                // to `provider_for_agent`; `codex-acp` maps to the cross-tool
                // root, so the skill has exactly one path and no native mirror.
                stable_agent_namespace: "config:codex-acp".to_string(),
            },
        ))
        .expect("bind an agent session");

    let guard = brand::override_canonical(canonical);
    seed(&workspace_cwd, &guard);
    runtime
        .block_on(outcome.application.open_conversation(conversation_id))
        .expect("open the conversation");
    drop(guard);

    OpenedWorkspace {
        _temp: temp,
        workspace_cwd,
    }
}

/// Control test — proves the harness reaches the real provisioner.
///
/// Deliberately runs under the *shipped* values rather than the injected ones,
/// so it stays an independent check: if the provisioner were bailing out early
/// (no binding, missing workspace) and writing nothing at all, every assertion
/// below would still be satisfiable by an accident of the seam, and this one
/// would not.
#[test]
fn opening_a_conversation_runs_the_real_skill_provisioner() {
    let opened = open_conversation_under(DEFAULT_CANONICAL, |_workspace, _guard| {});
    let provisioned = skill_md(&opened.workspace_cwd, DEFAULT_CANONICAL.skill_name);
    assert!(
        provisioned.is_file(),
        "open_conversation must provision the managed skill at {}",
        provisioned.display()
    );
    let body = std::fs::read_to_string(&provisioned).unwrap();
    assert!(
        body.contains(DEFAULT_CANONICAL.skill_marker),
        "the provisioned skill carries the managed marker"
    );
}

/// The claim the two tests below make through an injected brand, made once more
/// under the values this build actually **ships**.
///
/// T-A21 flipped `DEFAULT_CANONICAL`'s skill name and marker. Every other test
/// in this file injects its own post-rename brand, so all of them would keep
/// passing if that flip had never landed or were reverted — they prove the seam,
/// not the shipped values. This one is the difference, and it is the state a
/// user's workspace is in the first time they open the renamed app: a `SKILL.md`
/// this app wrote is sitting at what is now the *canonical* path, still carrying
/// the pre-rename marker, and a directory under the pre-rename name is beside it.
///
/// The `assert_ne!` pair is the precondition, not decoration: with the flip
/// reverted, `canonical() == LEGACY` for both fields, "recognised the legacy
/// marker" and "recognised its own marker" become the same statement, and
/// nothing below distinguishes them.
#[test]
fn the_shipped_skill_identity_claims_a_pre_rename_file_without_rewriting_it() {
    assert_ne!(
        DEFAULT_CANONICAL.skill_name,
        brand::LEGACY.skill_name,
        "the managed skill name has not been flipped, so nothing below \
         distinguishes the pre-rename identity from the current one"
    );
    assert_ne!(
        DEFAULT_CANONICAL.skill_marker,
        brand::LEGACY.skill_marker,
        "the managed marker has not been flipped, so 'claimed a file carrying the \
         legacy marker' below is vacuous"
    );

    let body = legacy_skill_body();
    let opened = open_conversation_under(DEFAULT_CANONICAL, |workspace, _guard| {
        // A file this app wrote before the rename, now sitting at the path the
        // shipped build considers its own.
        write_skill(&skill_md(workspace, DEFAULT_CANONICAL.skill_name), &body);
        // And the directory the pre-rename build actually used.
        write_skill(&skill_md(workspace, brand::LEGACY.skill_name), &body);
    });

    // Claimed rather than refused: the provisioner recognised the pre-rename
    // marker as its own writing and re-templated the file. Had it not, the body
    // would still be the frozen fixture — `backfill_managed_skills` logs and
    // swallows `UnmanagedCollision`, so the filesystem is the only observable.
    let canonical = skill_md(&opened.workspace_cwd, DEFAULT_CANONICAL.skill_name);
    let after = std::fs::read_to_string(&canonical)
        .unwrap_or_else(|e| panic!("read {} failed: {e}", canonical.display()));
    assert!(
        after.contains(DEFAULT_CANONICAL.skill_marker),
        "the shipped build did not claim a file carrying the pre-rename marker: {} \
         still reads as it did before provisioning, so its own earlier writing was \
         treated as a user-owned file and the skill silently stopped being maintained",
        canonical.display()
    );

    // And the pre-rename directory is left exactly as it was (FORBID-05): it is
    // a user-visible file an agent may still be reading.
    let legacy = skill_md(&opened.workspace_cwd, brand::LEGACY.skill_name);
    assert_eq!(
        std::fs::read_to_string(&legacy).expect("the pre-rename skill must never be deleted"),
        body,
        "{} was rewritten; LEGACY values may only ever be read",
        legacy.display()
    );
}

/// After the rename the managed skill must be written under the canonical name.
#[test]
fn managed_skill_is_provisioned_under_the_canonical_name() {
    let opened = open_conversation_under_post_rename_brand(|workspace, guard| {
        let _ = guard;
        write_skill(
            &skill_md(workspace, brand::LEGACY.skill_name),
            &legacy_skill_body(),
        );
    });

    let canonical_name = post_rename().skill_name;
    let canonical = skill_md(&opened.workspace_cwd, canonical_name);
    assert!(
        canonical.is_file(),
        "skill was not provisioned under the canonical name: canonical() \
         reports the skill name is {canonical_name:?}, so the provisioner must \
         write {}, but nothing is there. `skill_path` must join \
         `provisioner::scheduled_task_skill_name()`, not a literal.",
        canonical.display()
    );
}

/// The legacy `SKILL.md` a shipped build already wrote must survive untouched.
///
/// It is a user-visible file an agent may still be reading; the rename copies
/// forward, it does not rewrite in place (brand.rs FORBID-04).
#[test]
fn legacy_skill_file_survives_provisioning_byte_for_byte() {
    let body = legacy_skill_body();
    let opened = open_conversation_under_post_rename_brand(|workspace, guard| {
        let _ = guard;
        write_skill(&skill_md(workspace, brand::LEGACY.skill_name), &body);
    });

    let legacy = skill_md(&opened.workspace_cwd, brand::LEGACY.skill_name);
    let after = std::fs::read_to_string(&legacy).expect("legacy skill must never be deleted");
    assert_eq!(
        after,
        body,
        "legacy skill file was rewritten after the rename: {} no longer matches \
         the frozen fixture, but LEGACY values may only ever be read",
        legacy.display()
    );
}

/// The provisioner must still recognise the *legacy* marker as its own.
///
/// This is the migration shape: the skill directory has been moved to the
/// canonical name but its body still carries the marker the previous build
/// wrote. `write_managed_skill` refuses to overwrite any file that is not
/// evidently ours, so a provisioner that only knows the canonical marker treats
/// its own file as user-owned and bails out with `UnmanagedCollision` —
/// silently, because `backfill_managed_skills` logs and swallows the error.
///
/// Re-templating that file is therefore the observable proof the claim was
/// recognised.
#[test]
fn provisioner_claims_a_skill_carrying_the_legacy_marker() {
    let canonical_name = post_rename().skill_name;
    let opened = open_conversation_under_post_rename_brand(|workspace, guard| {
        write_skill(
            &skill_md(workspace, guard_skill_name(guard)),
            &legacy_skill_body(),
        );
        let _ = workspace;
    });

    let canonical = skill_md(&opened.workspace_cwd, canonical_name);
    let body = std::fs::read_to_string(&canonical)
        .unwrap_or_else(|e| panic!("read {} failed: {e}", canonical.display()));
    let canonical_marker = post_rename().skill_marker;
    assert!(
        body.contains(canonical_marker),
        "provisioner did not claim a file carrying the legacy marker: {} still \
         reads as it did before provisioning, so the legacy marker was treated \
         as an unmanaged collision instead of as this app's own writing. It \
         must carry {canonical_marker:?}.",
        canonical.display()
    );
}

/// Reads the canonical skill name back out of the seam, so the seeding closure
/// and the assertion cannot drift apart.
fn guard_skill_name(_guard: &BrandOverrideGuard) -> &'static str {
    brand::canonical().skill_name
}

/// The legacy skill must stop being the *active* one.
///
/// `managed-skills.json` is the provisioner's own record of which files it
/// currently owns. After the rename it must name the canonical skill and point
/// only at canonical paths; the legacy file stays on disk but is no longer
/// claimed.
#[test]
fn managed_skill_manifest_stops_claiming_the_legacy_skill() {
    let opened = open_conversation_under_post_rename_brand(|workspace, guard| {
        let _ = guard;
        write_skill(
            &skill_md(workspace, brand::LEGACY.skill_name),
            &legacy_skill_body(),
        );
    });

    let canonical = post_rename();
    // The manifest moves with the workspace directory; accept it wherever it
    // landed so this test reports on the *claim*, not on the directory name
    // (which `legacy_brand_worktree.rs` already owns).
    let manifest_path = [canonical.workspace_dir, brand::LEGACY.workspace_dir]
        .into_iter()
        .map(|dir| opened.workspace_cwd.join(dir).join("managed-skills.json"))
        .find(|path| path.is_file())
        .expect("the provisioner writes a managed-skills manifest");
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap())
            .expect("managed-skills.json parses");

    let legacy_segment = format!("/{}/", brand::LEGACY.skill_name);
    let claimed: Vec<String> = manifest["paths"]
        .as_array()
        .expect("manifest carries a paths array")
        .iter()
        .map(|entry| entry.as_str().expect("manifest path is a string").to_string())
        .collect();
    assert!(
        !claimed.iter().any(|path| path.contains(&legacy_segment)),
        "managed-skills manifest still points at the legacy skill: {} claims \
         {claimed:?}, but canonical() reports the skill name is {:?} — the \
         legacy file must remain on disk unclaimed, not stay the active one.",
        manifest_path.display(),
        canonical.skill_name
    );
}
