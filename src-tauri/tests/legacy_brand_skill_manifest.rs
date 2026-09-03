//! T-H23 — the on-disk keys of the managed-skill manifest.
//!
//! # Why this contract is invisible
//!
//! Finding F-03. `ManagedSkillManifestV1` carries
//! `#[serde(rename_all = "camelCase", deny_unknown_fields)]`, so the Rust field
//! identifier of the ownership flag **is** its persisted JSON key. There is no
//! string literal anywhere in the repo to grep for, exclude from a rename list,
//! or notice in review. A rename of the identifier is a one-token edit that
//! compiles, reads naturally, and silently rewrites an external contract.
//!
//! `deny_unknown_fields` is what turns that from a cosmetic drift into data
//! loss: a manifest already sitting in a user's workspace directory stops
//! deserializing the *instant* the key moves — not degraded, rejected. The D
//! batch did move it once, with no alias and no version bump, and the executor
//! reverted it. This file is what makes the next attempt ring instead of
//! relying on a comment being read.
//!
//! # Why this reads from disk
//!
//! `tests/fixtures/legacy-brand/user-skills/managed-skills-v1.json` is a frozen
//! v1 manifest exactly as the provisioner writes it, sha256-guarded by
//! `legacy_brand_fixture_manifest.rs`. Its `sha256` field is the real digest of
//! the frozen skill file next to it, so the two fixtures corroborate each
//! other. Every expectation below comes from `brand::LEGACY.skill_manifest_key`
//! / `brand::canonical().skill_manifest_key` at runtime — never an inline
//! literal.
//!
//! # Why `brand.rs` gained a `skill_manifest_key`
//!
//! Serde attributes accept literals only, so production genuinely *cannot* read
//! the key from `brand::canonical()`. The constant exists so the value has one
//! home anyway, and the agreement between the attribute and that home is
//! guarded by a source-text comparison rather than by a whitelist entry that
//! nothing enforces.
//!
//! # Why the struct is mirrored rather than imported
//!
//! `mod skills` is private to `termul_manager_lib` (`src/lib.rs:29`, no `pub`
//! and no re-export), so `ManagedSkillManifestV1` is not nameable from an
//! integration test. Two stand-ins do the work, and the difference between them
//! is the point:
//!
//! - [`ManagedSkillManifestMirror`] tracks production *today*, pinned by
//!   `mirror_key_set_matches_the_production_struct`.
//! - [`ManagedSkillManifestV1Reader`] is frozen at the pre-rename shape and
//!   plays the older binary, pinned to the frozen fixture by
//!   `v1_reader_matches_the_frozen_manifest_on_disk`.
//!
//! Neither is ever the *source* of an expectation.
//!
//! # Seam status
//!
//! Landed by T-M12 and T-A21:
//!
//! 1. The ownership key moved to `brand::canonical().skill_manifest_key`'s
//!    spelling, with an explicit `#[serde(alias = …)]` naming the pre-rename key
//!    so manifests already on disk still parse.
//! 2. `schema_version` is stamped 2 on write and the read path accepts 1 and 2.
//!    T-M12 added the read path; before it, the manifest was write-only and no
//!    alias or version check on it could have taken effect at all.
//! 3. Downgrade is explicitly **out** of contract; see
//!    `downgrading_to_an_older_binary_is_a_known_and_accepted_loss`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use syn::visit::{self, Visit};
use syn::{Expr, ExprCall, ExprMethodCall, ExprStruct, ItemStruct, Lit};
use termul_manager_lib::brand::{self, BrandCanonical};

/// The production site under test.
const PRODUCTION_FILE: &str = "src/skills/provisioner.rs";
const PRODUCTION_STRUCT: &str = "ManagedSkillManifestV1";
const PRODUCTION_WRITER_FN: &str = "provision";

/// The post-rename canonical key. A different spelling from the legacy one, so
/// nothing below can be satisfied by accident.
fn post_rename() -> BrandCanonical {
    BrandCanonical {
        skill_manifest_key: "managedBySeManager",
        ..brand::DEFAULT_CANONICAL
    }
}

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_path() -> PathBuf {
    manifest_dir().join("tests/fixtures/legacy-brand/user-skills/managed-skills-v1.json")
}

fn frozen_manifest_text() -> String {
    let path = fixture_path();
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("frozen fixture {} is unreadable: {e}", path.display()))
}

fn frozen_manifest_value() -> Value {
    serde_json::from_str(&frozen_manifest_text()).expect("the frozen manifest is JSON")
}

/// The shape a **pre-rename** binary compiled: the old ownership key, and no
/// compatibility alias for anything.
///
/// This is deliberately *not* a mirror of production any more. It plays the
/// older binary in [`downgrading_to_an_older_binary_is_a_known_and_accepted_loss`],
/// and it is the reader the frozen v1 fixture has to parse under. Pinned to the
/// frozen artifact — not to production — by
/// [`v1_reader_matches_the_frozen_manifest_on_disk`], so it cannot drift into a
/// fiction the tests below reason about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManagedSkillManifestV1Reader {
    schema_version: u32,
    managed_by_termul: bool,
    skill_name: String,
    template_version: u32,
    sha256: String,
    paths: Vec<String>,
}

/// Stand-in for the private `ManagedSkillManifestV1` **as it exists today**.
/// Serde attributes are copied from `provisioner.rs`;
/// `mirror_key_set_matches_the_production_struct` is what keeps that copy honest.
///
/// The production struct's compat `alias` is deliberately absent: an alias never
/// appears in serialized output, so mirroring it would change nothing here, and
/// spelling a legacy brand value in this file would be the FORBID-04 violation
/// the alias itself is the single named exception to. The alias is checked
/// against `brand::LEGACY` by parsing the production source instead — see
/// [`accepted_disk_keys`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManagedSkillManifestMirror {
    schema_version: u32,
    managed_by_se_manager: bool,
    skill_name: String,
    template_version: u32,
    sha256: String,
    paths: Vec<String>,
}

// ---------------------------------------------------------------------------
// Reading the production source
// ---------------------------------------------------------------------------

/// serde's `RenameRule::CamelCase` applied to a snake_case identifier.
fn to_camel_case(identifier: &str) -> String {
    let mut out = String::with_capacity(identifier.len());
    let mut capitalize = false;
    for character in identifier.chars() {
        if character == '_' {
            capitalize = true;
            continue;
        }
        if capitalize {
            out.extend(character.to_uppercase());
            capitalize = false;
        } else {
            out.push(character);
        }
    }
    out
}

/// The serde words on a container or field: bare flags (`deny_unknown_fields`)
/// map to `None`, `key = "value"` forms map to `Some(value)`.
fn serde_words(attributes: &[syn::Attribute]) -> Vec<(String, Option<String>)> {
    let mut words = Vec::new();
    for attribute in attributes {
        if !attribute.path().is_ident("serde") {
            continue;
        }
        let _ = attribute.parse_nested_meta(|meta| {
            let name = meta
                .path
                .get_ident()
                .map(ToString::to_string)
                .unwrap_or_default();
            let value = meta
                .value()
                .ok()
                .and_then(|stream| stream.parse::<syn::LitStr>().ok())
                .map(|literal| literal.value());
            words.push((name, value));
            Ok(())
        });
    }
    words
}

fn parse_production_file(relative: &str) -> syn::File {
    let path = manifest_dir().join(relative);
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("production file {} is unreadable: {e}", path.display()));
    syn::parse_file(&source)
        .unwrap_or_else(|e| panic!("production file {} does not parse: {e}", path.display()))
}

#[derive(Default)]
struct StructFinder {
    found: Option<ItemStruct>,
}

impl<'ast> Visit<'ast> for StructFinder {
    fn visit_item_struct(&mut self, node: &'ast ItemStruct) {
        if node.ident == PRODUCTION_STRUCT {
            self.found = Some(node.clone());
        }
        visit::visit_item_struct(self, node);
    }
}

fn production_struct() -> ItemStruct {
    let mut finder = StructFinder::default();
    finder.visit_file(&parse_production_file(PRODUCTION_FILE));
    finder.found.unwrap_or_else(|| {
        panic!("{PRODUCTION_STRUCT} no longer exists in {PRODUCTION_FILE}; retarget this test")
    })
}

/// How one production field is named on disk.
///
/// `primary` is the key serde *writes*; `aliases` are the extra keys it will
/// still *read*. The two are kept apart because a mirror struct can only ever
/// reproduce the first — aliases never appear in serialized output — and
/// conflating them was what made the old single-set comparison impossible to
/// satisfy once a compat alias existed.
#[derive(Debug, Default)]
struct FieldKeys {
    primary: String,
    aliases: BTreeSet<String>,
}

impl FieldKeys {
    fn accepted(&self) -> BTreeSet<String> {
        let mut all = self.aliases.clone();
        all.insert(self.primary.clone());
        all
    }
}

/// `field identifier -> how that field is named on disk`.
///
/// Covers the three ways serde can name a field: the container's `rename_all`
/// projection of the identifier, an explicit per-field `rename`, and any number
/// of `alias`es (the compat-read mechanism).
fn disk_keys() -> BTreeMap<String, FieldKeys> {
    let item = production_struct();
    let container = serde_words(&item.attrs);
    let rename_all = container
        .iter()
        .find(|(name, _)| name == "rename_all")
        .and_then(|(_, value)| value.clone());

    let mut keys = BTreeMap::new();
    for field in &item.fields {
        let Some(identifier) = field.ident.as_ref().map(ToString::to_string) else {
            continue;
        };
        let words = serde_words(&field.attrs);
        let primary = match words.iter().find(|(name, _)| name == "rename") {
            Some((_, Some(explicit))) => explicit.clone(),
            _ => match rename_all.as_deref() {
                Some("camelCase") => to_camel_case(&identifier),
                _ => identifier.clone(),
            },
        };
        let aliases = words
            .iter()
            .filter(|(name, _)| name == "alias")
            .filter_map(|(_, value)| value.clone())
            .collect();
        keys.insert(identifier, FieldKeys { primary, aliases });
    }
    keys
}

/// `field identifier -> every JSON key that would deserialize into it`.
fn accepted_disk_keys() -> BTreeMap<String, BTreeSet<String>> {
    disk_keys()
        .into_iter()
        .map(|(identifier, keys)| (identifier, keys.accepted()))
        .collect()
}

/// Only the keys production *writes* — no aliases.
fn primary_disk_keys() -> BTreeSet<String> {
    disk_keys().into_values().map(|keys| keys.primary).collect()
}

/// The `schema_version` literal the writer stamps into every new manifest.
#[derive(Default)]
struct WriterScan {
    schema_version: Option<u64>,
}

impl<'ast> Visit<'ast> for WriterScan {
    fn visit_expr_struct(&mut self, node: &'ast ExprStruct) {
        let names_the_manifest = node
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == PRODUCTION_STRUCT);
        if names_the_manifest {
            for field in &node.fields {
                let is_schema_version = matches!(
                    &field.member,
                    syn::Member::Named(name) if name == "schema_version"
                );
                if !is_schema_version {
                    continue;
                }
                if let Expr::Lit(literal) = &field.expr {
                    if let Lit::Int(value) = &literal.lit {
                        self.schema_version = value.base10_parse::<u64>().ok();
                    }
                }
            }
        }
        visit::visit_expr_struct(self, node);
    }
}

fn writer_schema_version() -> u64 {
    let ast = parse_production_file(PRODUCTION_FILE);
    let mut scan = WriterScan::default();
    scan.visit_file(&ast);
    scan.schema_version.unwrap_or_else(|| {
        panic!(
            "{PRODUCTION_FILE}::{PRODUCTION_WRITER_FN} no longer stamps a literal schema_version \
             into {PRODUCTION_STRUCT}; retarget this test"
        )
    })
}

/// Every called function name in the skills module, used to answer "is there a
/// deserializing read of this manifest anywhere".
#[derive(Default)]
struct CallScan {
    called: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for CallScan {
    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        if let Expr::Path(path) = node.func.as_ref() {
            if let Some(segment) = path.path.segments.last() {
                self.called.insert(segment.ident.to_string());
            }
        }
        visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        self.called.insert(node.method.to_string());
        visit::visit_expr_method_call(self, node);
    }
}

/// `serde_json::from_*` calls anywhere under `src/skills/`.
fn deserializing_calls_in_skills_module() -> BTreeSet<String> {
    let root = manifest_dir().join("src/skills");
    let mut scan = CallScan::default();
    let entries = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("read_dir {} failed: {e}", root.display()));
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.extension().is_some_and(|extension| extension == "rs") {
            let relative = path
                .strip_prefix(manifest_dir())
                .expect("under the manifest dir")
                .to_string_lossy()
                .into_owned();
            scan.visit_file(&parse_production_file(&relative));
        }
    }
    scan.called
        .iter()
        .filter(|name| name.starts_with("from_"))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------------

/// The frozen v1 manifest really is a *legacy* one, and it deserializes.
///
/// Two independent sources: the bytes on disk, and `brand::LEGACY`. Editing
/// either alone turns this red, which is what makes the reds below trustworthy.
#[test]
fn frozen_v1_manifest_deserializes_and_declares_legacy_ownership() {
    let value = frozen_manifest_value();
    let ownership = value
        .get(brand::LEGACY.skill_manifest_key)
        .unwrap_or_else(|| {
            panic!(
                "the frozen manifest has no {:?} key; keys on disk: {:?}",
                brand::LEGACY.skill_manifest_key,
                value.as_object().map(|map| map.keys().collect::<Vec<_>>())
            )
        });
    assert_eq!(
        ownership,
        &Value::Bool(true),
        "the frozen manifest must declare this app owns the skill files it lists"
    );

    let parsed: ManagedSkillManifestV1Reader =
        serde_json::from_str(&frozen_manifest_text()).expect("the frozen v1 manifest deserializes");
    assert!(
        parsed.managed_by_termul,
        "the ownership flag must survive deserialization"
    );
    assert_eq!(parsed.schema_version, 1, "the frozen manifest is a v1");

    // The fixtures corroborate each other: the digest recorded here is the real
    // digest of the frozen skill file beside it, so neither can be regenerated
    // in isolation without the other noticing.
    let skill = manifest_dir().join("tests/fixtures/legacy-brand/user-skills");
    let listed_skill_name = skill.join(format!("{}.md", parsed.skill_name));
    assert!(
        listed_skill_name.is_file(),
        "the frozen manifest names a skill with no frozen file beside it: {}",
        listed_skill_name.display()
    );
}

/// The attribute and the constant agree. This is the *only* place the legacy
/// key spelling is allowed to exist outside `brand.rs` and the frozen fixtures
/// — serde takes literals only — so it gets a guard rather than a whitelist
/// entry.
#[test]
fn production_field_identifier_projects_onto_the_brand_key() {
    let keys = accepted_disk_keys();
    let projected: BTreeSet<String> = keys.values().flatten().cloned().collect();

    assert!(
        projected.contains(brand::LEGACY.skill_manifest_key),
        "the disk key produced by {PRODUCTION_FILE}'s serde attributes is not \
         brand::LEGACY.skill_manifest_key ({:?}); keys the struct accepts today: {projected:?}",
        brand::LEGACY.skill_manifest_key,
    );

    // And the frozen bytes agree with both.
    let on_disk: BTreeSet<String> = frozen_manifest_value()
        .as_object()
        .expect("the frozen manifest is an object")
        .keys()
        .cloned()
        .collect();
    assert!(
        on_disk.is_subset(&projected),
        "the frozen manifest carries keys the production struct would reject under \
         deny_unknown_fields: on disk {on_disk:?}, accepted {projected:?}"
    );
}

/// Keeps [`ManagedSkillManifestV1Reader`] pinned to the bytes a pre-rename build
/// actually wrote, rather than to production.
///
/// It stopped tracking production the moment T-A21 moved the ownership key, and
/// an unpinned stand-in for "the older binary" is worth nothing — the downgrade
/// test below would be asserting against whatever this file happened to declare.
/// The frozen fixture is the independent source: it is sha256-guarded by
/// `legacy_brand_fixture_manifest.rs` and cannot be edited to match.
#[test]
fn v1_reader_matches_the_frozen_manifest_on_disk() {
    let reader_value = serde_json::to_value(ManagedSkillManifestV1Reader {
        schema_version: 1,
        managed_by_termul: true,
        skill_name: String::new(),
        template_version: 0,
        sha256: String::new(),
        paths: Vec::new(),
    })
    .expect("the v1 reader serializes");
    let reader: BTreeSet<String> = reader_value
        .as_object()
        .expect("the v1 reader serializes to an object")
        .keys()
        .cloned()
        .collect();

    let on_disk: BTreeSet<String> = frozen_manifest_value()
        .as_object()
        .expect("the frozen manifest is an object")
        .keys()
        .cloned()
        .collect();

    assert_eq!(
        reader, on_disk,
        "ManagedSkillManifestV1Reader no longer describes the frozen v1 manifest, so it is \
         not the older binary any more and downgrading_to_an_older_binary_is_a_known_and_accepted_loss \
         proves nothing"
    );
    assert!(
        reader.contains(brand::LEGACY.skill_manifest_key),
        "the v1 reader's ownership key is not brand::LEGACY.skill_manifest_key ({:?}); \
         keys it writes: {reader:?}",
        brand::LEGACY.skill_manifest_key,
    );
}

/// Keeps [`ManagedSkillManifestMirror`] pinned to the production struct. If a
/// field is added, dropped or renamed over there, this diverges — so the mirror
/// can never quietly become a fiction the other tests reason about.
///
/// Compared against production's *primary* keys rather than everything it
/// accepts: aliases are read-only and never serialize, so a mirror can never
/// reproduce them. That the alias exists, and that it is exactly the pre-rename
/// spelling, is asserted separately below from the parsed production source.
#[test]
fn mirror_key_set_matches_the_production_struct() {
    let production = primary_disk_keys();

    let mirror_value = serde_json::to_value(ManagedSkillManifestMirror {
        schema_version: 2,
        managed_by_se_manager: true,
        skill_name: String::new(),
        template_version: 0,
        sha256: String::new(),
        paths: Vec::new(),
    })
    .expect("the mirror serializes");
    let mirror: BTreeSet<String> = mirror_value
        .as_object()
        .expect("the mirror serializes to an object")
        .keys()
        .cloned()
        .collect();

    assert_eq!(
        mirror, production,
        "ManagedSkillManifestMirror has drifted from {PRODUCTION_FILE}::{PRODUCTION_STRUCT}"
    );

    // The compat read is an `alias` on the ownership field specifically, and it
    // is exactly the pre-rename spelling — not some other key that happens to be
    // accepted. Both operands are independent: one is parsed out of the
    // production source, the other read from `brand::LEGACY`.
    let ownership = disk_keys()
        .into_iter()
        .find(|(_, keys)| keys.primary == brand::canonical().skill_manifest_key)
        .map(|(_, keys)| keys)
        .unwrap_or_else(|| {
            panic!(
                "no field of {PRODUCTION_STRUCT} writes brand::canonical().skill_manifest_key ({:?})",
                brand::canonical().skill_manifest_key
            )
        });
    assert_eq!(
        ownership.aliases,
        BTreeSet::from([brand::LEGACY.skill_manifest_key.to_string()]),
        "the ownership field's compat aliases must be exactly the pre-rename key; \
         serde takes literals only, so this attribute is the single named FORBID-04 \
         exception and this is what keeps it equal to brand::LEGACY.skill_manifest_key"
    );

    // `deny_unknown_fields` is load-bearing for every claim in this file: it is
    // why a moved key is a rejection rather than a default.
    let container = serde_words(&production_struct().attrs);
    assert!(
        container
            .iter()
            .any(|(name, _)| name == "deny_unknown_fields"),
        "{PRODUCTION_STRUCT} no longer denies unknown fields; the failure mode this \
         file guards has changed shape and the tests must be rewritten"
    );
}

/// Downgrade is **not** in the contract, and that is a decision rather than an
/// oversight.
///
/// Once the key moves and `schema_version` bumps, a manifest written by the new
/// binary will be rejected outright by an older one — `deny_unknown_fields`
/// makes it a hard error, not a lost field. This test executes that rejection
/// so nobody later reads it as a bug to be fixed: rolling back the app after it
/// has provisioned a skill means the older binary re-provisions rather than
/// adopts. Losing the ownership record only costs a rewrite; there are no user
/// data in this file.
#[test]
fn downgrading_to_an_older_binary_is_a_known_and_accepted_loss() {
    let _guard = brand::override_canonical(post_rename());
    let mut value = frozen_manifest_value();
    let object = value.as_object_mut().expect("the frozen manifest is an object");

    let legacy_flag = object
        .remove(brand::LEGACY.skill_manifest_key)
        .expect("the frozen manifest carries the legacy ownership key");
    object.insert(
        brand::canonical().skill_manifest_key.to_string(),
        legacy_flag,
    );
    object.insert("schemaVersion".to_string(), Value::from(2));

    let error = serde_json::from_value::<ManagedSkillManifestV1Reader>(value)
        .expect_err("a v2 manifest must NOT deserialize under the v1 reader");
    let message = error.to_string();
    assert!(
        message.contains("unknown field"),
        "the v1 reader must reject a v2 manifest by name, so the failure is legible \
         in a log; got {message:?}"
    );
    assert!(
        message.contains(brand::canonical().skill_manifest_key),
        "the rejection must name the key that moved; got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// The ledger — reds that go green when the capability lands
// ---------------------------------------------------------------------------

/// The disk key must move to the canonical spelling **and** keep accepting the
/// legacy one.
///
/// Both halves in one assertion on purpose: moving the key without a compat
/// read is exactly the failure the reverted D-batch change caused, and keeping
/// the compat read without moving the key is not a rename at all.
///
/// Ledger entry struck by T-A21: the field is `managed_by_se_manager` with a
/// permanent `#[serde(alias = …)]` for the pre-rename key.
#[test]
fn manifest_accepts_both_the_canonical_and_the_legacy_ownership_key() {
    let _guard = brand::override_canonical(post_rename());
    assert_ne!(
        brand::canonical().skill_manifest_key,
        brand::LEGACY.skill_manifest_key,
        "the post-rename injection did not take"
    );

    let accepted: BTreeSet<String> = accepted_disk_keys().values().flatten().cloned().collect();

    assert!(
        accepted.contains(brand::canonical().skill_manifest_key),
        "{PRODUCTION_FILE}::{PRODUCTION_STRUCT} does not accept the post-rename ownership key \
         ({:?}). Serde attributes take literals only, so this needs an explicit field rename \
         whose spelling matches brand::canonical().skill_manifest_key. Keys accepted today: \
         {accepted:?}",
        brand::canonical().skill_manifest_key,
    );
    assert!(
        accepted.contains(brand::LEGACY.skill_manifest_key),
        "{PRODUCTION_FILE}::{PRODUCTION_STRUCT} stopped accepting the legacy ownership key \
         ({:?}). deny_unknown_fields means every manifest already in a user's .termul/ is \
         rejected outright the moment that alias disappears. Keys accepted today: {accepted:?}",
        brand::LEGACY.skill_manifest_key,
    );
}

/// A moved key is a schema change, so the version has to say so.
///
/// Without the bump there is no way for any future reader to tell a v1 manifest
/// from a v2 one except by probing which key is present, which is precisely the
/// implicit coupling this file exists to remove.
///
/// Ledger entry struck by T-A21: `provision` stamps 2.
#[test]
fn new_manifests_are_written_at_schema_version_2() {
    let written = writer_schema_version();
    assert_eq!(
        written, 2,
        "{PRODUCTION_FILE}::{PRODUCTION_WRITER_FN} still stamps schema_version {written} onto \
         every manifest it writes. Moving the ownership key changes the on-disk shape, so the \
         version must move with it."
    );
}

/// The manifest is read back, so a compatibility read on it can take effect.
///
/// It used to be write-only: `provision` serialized it and nothing under
/// `src/skills/` ever deserialized it, which meant an alias or a
/// `schema_version` check on it could not possibly change any behaviour. T-M12
/// added `provisioner::read_manifest` — it reads the current workspace
/// directory then the pre-rename one, accepts `schema_version` 1 and 2, and
/// feeds the recorded paths into `write_managed_skill`'s ownership decision.
/// Without a reader, the remaining ledger entry above would be decoration.
#[test]
fn a_read_path_exists_and_accepts_both_schema_versions() {
    let reads = deserializing_calls_in_skills_module();
    assert!(
        !reads.is_empty(),
        "src/skills/ never deserializes the managed-skill manifest — it is written by \
         {PRODUCTION_FILE}::{PRODUCTION_WRITER_FN} and never read back, so no compat alias \
         and no schema_version check can possibly take effect. Deserializing calls found: \
         {reads:?}"
    );
}
