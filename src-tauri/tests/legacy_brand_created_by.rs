//! T-H22 — `ConversationRecordV2.createdBy`: the wire value must be decoupled
//! from the enum variant identifier.
//!
//! # The root cause is a data-structure problem, not a literal problem
//!
//! Finding F-01, and the reason it is CRITICAL. `ConversationCreator` carries
//! `#[serde(rename_all = "snake_case")]` and one variant, `Termul`
//! (`src/conversation/contracts.rs:302-306`). The identifier *is* the on-disk
//! string `"termul"`. There is no literal anywhere in the crate to grep for, to
//! put on a rename exclusion list, or to notice in a diff — so any execution
//! shaped as "rename the identifiers, keep the listed literals" rewrites an
//! external contract in complete silence. Every conversation index already on a
//! user's disk then fails validation, and the record is reported corrupt.
//!
//! `the_wire_value_has_no_greppable_literal_where_it_is_decided` below is that
//! claim, executed.
//!
//! # What the fix has to be
//!
//! Explicit per-variant `#[serde(rename = "…")]` **and** removal of
//! `rename_all` from the container. Adding the rename while leaving
//! `rename_all` in place looks equivalent and is not: `rename_all` keeps the
//! wire value hitched to whatever the identifier happens to be, so the *next*
//! rename silently rewrites disk again. That is the whole content of
//! `the_wire_value_is_decoupled_from_the_variant_identifier`.
//!
//! # Why nothing below names the variant identifier
//!
//! Every test reaches the legacy variant by *deserializing*
//! `brand::LEGACY.created_by`, never by writing `ConversationCreator::Termul`.
//! That is deliberate: a harness that names the identifier would stop compiling
//! under the very refactor it exists to certify, so it could never be used to
//! check that refactor. It also makes the assertions cross two independent
//! sources — the serde impl and `brand.rs` — instead of comparing a constant
//! with a copy of itself.
//!
//! The frozen record is `src/__fixtures__/legacy-brand/conversation-createdBy-termul.json`,
//! sha256-guarded by `src/__fixtures__/legacy-brand-manifest.test.ts`. It is the
//! same fixture the TypeScript half of this contract
//! (`src/shared/types/conversation.types.brand.test.ts`) reads, so both sides
//! are pinned to one artifact rather than to each other.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;
use syn::visit::{self, Visit};
use syn::{Attribute, ItemEnum, ItemMod, Lit};
use se_manager_lib::brand::{self, BrandCanonical};
use se_manager_lib::conversation::ConversationCreator;

/// Where the enum lives.
const CONTRACTS_FILE: &str = "src/conversation/contracts.rs";
const CREATOR_ENUM: &str = "ConversationCreator";

/// The production write points this repo has today, **discovered** by
/// `every_production_write_point_is_accounted_for` rather than transcribed from
/// a plan document. Listed here only so a *new* one is noticed.
///
/// The Wave-3 brief named `compatibility.rs:133` as the sole production write
/// point, with everything else "inside `#[cfg(test)]`". The scan disagrees, and
/// the scan is right — all three of these sit above their file's `#[cfg(test)]`
/// module:
///
/// - `compatibility.rs::LegacyConversationProjection` — the legacy projection.
/// - `creation.rs::prepare_new_locked` — the **primary** one. Every
///   conversation this app creates gets its `created_by` here. Missing it means
///   the flip decision (OD-04) would have been made for the projection and the
///   migration while the main creation path kept writing the other value.
/// - `migration/legacy.rs::stage_one` — reached from the public
///   `stage_legacy_conversations`; stamps every record the migration writes.
const KNOWN_PRODUCTION_WRITE_POINTS: &[&str] = &[
    "src/conversation/compatibility.rs",
    "src/conversation/creation.rs",
    "src/conversation/migration/legacy.rs",
];

/// The post-rename wire value.
fn post_rename() -> BrandCanonical {
    BrandCanonical {
        created_by: "se-manager",
        ..brand::DEFAULT_CANONICAL
    }
}

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The TypeScript-side frozen root, reached from `src-tauri/`. Both language
/// halves of this contract read the same bytes.
fn frozen_record() -> Value {
    let path = manifest_dir().join("../src/__fixtures__/legacy-brand/conversation-createdBy-termul.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("frozen fixture {} is unreadable: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("frozen fixture {} is not JSON: {e}", path.display()))
}

/// The legacy variant, obtained without ever spelling its identifier.
///
/// If serde's wire value and `brand::LEGACY.created_by` ever disagree this
/// fails to deserialize, which is exactly the coupling under test.
fn legacy_variant() -> ConversationCreator {
    serde_json::from_value(Value::String(brand::LEGACY.created_by.to_string())).unwrap_or_else(
        |e| {
            panic!(
                "brand::LEGACY.created_by ({:?}) is not a wire value any {CREATOR_ENUM} variant \
                 accepts: {e}. Either the enum's wire value moved without brand.rs moving, or \
                 the compatibility read for records already on disk is gone.",
                brand::LEGACY.created_by
            )
        },
    )
}

// ---------------------------------------------------------------------------
// Reading the production source
// ---------------------------------------------------------------------------

/// The serde words on a container or variant: bare flags map to `None`,
/// `key = "value"` forms map to `Some(value)`.
fn serde_words(attributes: &[Attribute]) -> Vec<(String, Option<String>)> {
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

fn has_cfg_test(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        attribute.path().is_ident("cfg")
            && attribute
                .meta
                .require_list()
                .is_ok_and(|list| list.tokens.to_string().contains("test"))
    })
}

fn parse_file(relative: &str) -> syn::File {
    let path = manifest_dir().join(relative);
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("production file {} is unreadable: {e}", path.display()));
    syn::parse_file(&source)
        .unwrap_or_else(|e| panic!("production file {} does not parse: {e}", path.display()))
}

#[derive(Default)]
struct EnumFinder {
    found: Option<ItemEnum>,
}

impl<'ast> Visit<'ast> for EnumFinder {
    fn visit_item_enum(&mut self, node: &'ast ItemEnum) {
        if node.ident == CREATOR_ENUM {
            self.found = Some(node.clone());
        }
        visit::visit_item_enum(self, node);
    }
}

fn creator_enum() -> ItemEnum {
    let mut finder = EnumFinder::default();
    finder.visit_file(&parse_file(CONTRACTS_FILE));
    finder
        .found
        .unwrap_or_else(|| panic!("{CREATOR_ENUM} no longer exists in {CONTRACTS_FILE}; retarget"))
}

/// Every `.rs` file under `src/` that is *not* compiled out of a release build.
///
/// Two exclusions, both necessary and both derived rather than listed:
/// files declared as `#[cfg(test)] mod <name>;` anywhere in the tree, and
/// `src/brand.rs`, which `brand.rs` itself designates as one of the two files
/// permitted to hold a legacy brand string.
fn production_rust_files() -> Vec<String> {
    let src = manifest_dir().join("src");
    let mut files = Vec::new();
    collect_rust_files(&src, &mut files);

    let mut test_only_stems: BTreeSet<String> = BTreeSet::new();
    for relative in &files {
        let ast = parse_file(relative);
        let mut scan = TestModScan::default();
        scan.visit_file(&ast);
        test_only_stems.extend(scan.stems);
    }

    files
        .into_iter()
        .filter(|relative| relative != "src/brand.rs")
        .filter(|relative| {
            let stem = Path::new(relative)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
            !test_only_stems.contains(&stem)
        })
        .collect()
}

fn collect_rust_files(dir: &Path, found: &mut Vec<String>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read_dir {} failed: {e}", dir.display()))
        .map(|entry| entry.expect("dir entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_rust_files(&path, found);
            continue;
        }
        if path.extension().is_some_and(|extension| extension == "rs") {
            let relative = path
                .strip_prefix(manifest_dir())
                .expect("under the manifest dir")
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            found.push(relative);
        }
    }
}

/// `#[cfg(test)] mod <name>;` declarations — the files that only exist for tests.
#[derive(Default)]
struct TestModScan {
    stems: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for TestModScan {
    fn visit_item_mod(&mut self, node: &'ast ItemMod) {
        if node.content.is_none() && has_cfg_test(&node.attrs) {
            self.stems.insert(node.ident.to_string());
        }
        visit::visit_item_mod(self, node);
    }
}

/// String literals and `ConversationCreator::…` constructions, skipping
/// anything under a `#[cfg(test)]` item.
#[derive(Default)]
struct ProductionScan {
    string_literals: Vec<String>,
    /// Which variants this file stamps onto a record it writes. Recorded by
    /// identifier because that is all the source says; the identifier is turned
    /// back into a wire value through the enum's own serde attributes rather
    /// than through a second copy of the mapping.
    constructed_variants: BTreeSet<String>,
}

impl ProductionScan {
    fn constructs_creator(&self) -> bool {
        !self.constructed_variants.is_empty()
    }
}

impl<'ast> Visit<'ast> for ProductionScan {
    fn visit_item_mod(&mut self, node: &'ast ItemMod) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        visit::visit_item_mod(self, node);
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        visit::visit_item_fn(self, node);
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        visit::visit_impl_item_fn(self, node);
    }

    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        let segments: Vec<String> = node
            .path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        // `ConversationCreator::<Variant>` in expression position — a write.
        if segments.len() >= 2 && segments[segments.len() - 2] == CREATOR_ENUM {
            self.constructed_variants
                .insert(segments[segments.len() - 1].clone());
        }
        visit::visit_expr_path(self, node);
    }

    fn visit_lit(&mut self, node: &'ast Lit) {
        if let Lit::Str(value) = node {
            self.string_literals.push(value.value());
        }
        visit::visit_lit(self, node);
    }
}

/// `relative path -> facts`, for every production file.
fn scan_production() -> BTreeMap<String, ProductionScan> {
    production_rust_files()
        .into_iter()
        .map(|relative| {
            let ast = parse_file(&relative);
            let mut scan = ProductionScan::default();
            scan.visit_file(&ast);
            (relative, scan)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------------

/// The serde impl and `brand.rs` agree, and neither is a copy of the other.
///
/// This is the assertion the two mutation proofs target: renaming the variant
/// identifier while keeping an explicit `#[serde(rename = "termul")]` must
/// leave it green (the wire value is decoupled), and deleting that rename must
/// turn it red (the guard is really guarding).
#[test]
fn the_legacy_wire_value_is_the_one_serde_emits() {
    let variant = legacy_variant();
    assert_eq!(
        serde_json::to_value(variant).expect("the creator serializes"),
        Value::String(brand::LEGACY.created_by.to_string()),
        "the wire value serde emits for the legacy creator must be \
         brand::LEGACY.created_by; every conversation index on a user's disk \
         carries that exact string"
    );
}

/// Round-trip closure over the frozen record: the bytes on disk deserialize
/// into the legacy variant, and that variant re-serializes to the same bytes.
#[test]
fn the_frozen_conversation_record_round_trips_through_the_creator_enum() {
    let record = frozen_record();
    let on_disk = record
        .get("createdBy")
        .unwrap_or_else(|| panic!("the frozen record has no createdBy: {record}"))
        .clone();

    assert_eq!(
        on_disk,
        Value::String(brand::LEGACY.created_by.to_string()),
        "the frozen record must still carry the legacy wire value"
    );

    let parsed: ConversationCreator = serde_json::from_value(on_disk.clone())
        .expect("the frozen record's createdBy deserializes into a creator variant");
    assert_eq!(
        serde_json::to_value(parsed).expect("the creator serializes"),
        on_disk,
        "re-serializing what was read off disk must reproduce it byte for byte"
    );
}

/// The finding, executed: nothing that decides `createdBy` spells its value.
///
/// Scoped to the enum's own file and to the three sites that stamp a creator
/// onto a record, because the *string* `"termul"` is not unique to this
/// contract — `src/acp/manager.rs`, `src/logging.rs` and `src/web/config.rs`
/// each hold it as a literal, for the MCP server name, the log target and the
/// standalone state dir respectively. That is precisely what makes F-01 so
/// dangerous: a rename driven by grepping for the brand string finds three
/// files, edits them, reports itself complete, and never touches the one
/// contract that has no literal at all.
///
/// So this asserts zero at the sites that matter, and the doc above records why
/// a repo-wide zero would be the wrong question.
#[test]
fn the_wire_value_has_no_greppable_literal_where_it_is_decided() {
    let wire_value = brand::LEGACY.created_by;
    let deciding_files: BTreeSet<&str> = KNOWN_PRODUCTION_WRITE_POINTS
        .iter()
        .copied()
        .chain(std::iter::once(CONTRACTS_FILE))
        .collect();

    let offenders: Vec<String> = scan_production()
        .into_iter()
        .filter(|(relative, _)| deciding_files.contains(relative.as_str()))
        .filter(|(_, scan)| scan.string_literals.iter().any(|value| value == wire_value))
        .map(|(relative, _)| relative)
        .collect();

    assert!(
        offenders.is_empty(),
        "the createdBy wire value {wire_value:?} now appears as a literal in {offenders:?}. \
         That is not the failure this test guards — it guards the opposite. If a literal has \
         appeared, the contract has a second source and this file's reasoning must be redone."
    );
}

/// Blast radius. Every place production stamps a creator onto a record it
/// writes, discovered from the source rather than transcribed from a plan.
///
/// The Wave-3 brief named `compatibility.rs:133` as the only production write
/// point. It is not: `migration/legacy.rs::stage_one` is a second, and it runs
/// on every legacy migration. If a third appears, this goes red and the
/// decision about what `createdBy` should say has to be made for it too.
#[test]
fn every_production_write_point_is_accounted_for() {
    let found: Vec<String> = scan_production()
        .into_iter()
        .filter(|(_, scan)| scan.constructs_creator())
        .map(|(relative, _)| relative)
        .filter(|relative| relative != CONTRACTS_FILE)
        .collect();

    let expected: Vec<String> = KNOWN_PRODUCTION_WRITE_POINTS
        .iter()
        .map(|value| (*value).to_string())
        .collect();

    assert_eq!(
        found, expected,
        "the set of production sites that construct a {CREATOR_ENUM} has changed. Each one \
         decides what a record this app writes today claims about its own origin, and none of \
         them contains a literal that a rename pass could see."
    );
}

/// Every variant's wire value, read from the enum's own `#[serde(rename = "…")]`
/// attributes. The mapping is never written down a second time here — if the
/// rename moves, this moves with it, which is what keeps the guard below from
/// comparing a constant with a copy of itself.
fn variant_wire_values() -> BTreeMap<String, String> {
    creator_enum()
        .variants
        .iter()
        .map(|variant| {
            let wire = serde_words(&variant.attrs)
                .into_iter()
                .find_map(|(name, value)| (name == "rename").then_some(value).flatten())
                .unwrap_or_else(|| {
                    panic!(
                        "{CREATOR_ENUM}::{} has no explicit #[serde(rename = \"…\")]; \
                         the_wire_value_is_decoupled_from_the_variant_identifier covers why \
                         that is not allowed",
                        variant.ident
                    )
                });
            (variant.ident.to_string(), wire)
        })
        .collect()
}

/// The other half of the blast radius: not *where* production stamps a creator,
/// but *what it stamps*.
///
/// `every_production_write_point_is_accounted_for` pins the file set and would
/// stay green with every one of those files writing the pre-rename value — which
/// is not hypothetical. Reverting `creation.rs`'s single token during T-A04 left
/// the entire suite green: the primary write point, the one every conversation
/// this app creates passes through, had no assertion on its value anywhere.
/// That is F-01's exact shape one level down — a half-flip that compiles, reads
/// correctly in review, and silently stamps a mixed corpus.
///
/// Compared through the serde rename map rather than the identifier, so a later
/// identifier refactor cannot make this pass by accident.
#[test]
fn every_production_write_point_stamps_the_canonical_creator() {
    let wire_values = variant_wire_values();
    let canonical = brand::canonical().created_by;

    let offenders: Vec<String> = scan_production()
        .into_iter()
        .filter(|(relative, _)| relative != CONTRACTS_FILE)
        .flat_map(|(relative, scan)| {
            scan.constructed_variants
                .into_iter()
                .map(move |variant| (relative.clone(), variant))
        })
        .filter_map(|(relative, variant)| {
            let emitted = wire_values.get(&variant).cloned().unwrap_or_else(|| {
                panic!("{CREATOR_ENUM}::{variant} is constructed in {relative} but is not a variant")
            });
            (emitted != canonical).then(|| format!("{relative} writes {emitted:?}"))
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "a production write point stamps a createdBy other than the canonical \
         {canonical:?}: {offenders:?}. FORBID-04 allows reading the legacy value forever \
         and re-emitting it never — a record written today must claim today's creator, or \
         the corpus splits into two batches that disagree about their own origin for no \
         reason a reader can recover."
    );
}

// ---------------------------------------------------------------------------
// The ledger — cleared in Wave 4 (T-A01); these are live guards now
// ---------------------------------------------------------------------------

/// The core of this file. Was a `#[should_panic]` ledger entry until T-A01
/// landed the decoupling; now a live guard against it being undone.
///
/// The wire value must be decided by an explicit `#[serde(rename = "…")]` on
/// each variant, and `rename_all` must be gone from the container. Both halves
/// are required. With `rename_all` still present the wire value stays hitched
/// to the identifier, so the next person who renames the identifier — for
/// perfectly good reasons, in a refactor that has nothing to do with branding —
/// rewrites the disk contract again, exactly as this rename would have.
#[test]
fn the_wire_value_is_decoupled_from_the_variant_identifier() {
    let item = creator_enum();

    let container_rename_all = serde_words(&item.attrs)
        .into_iter()
        .find(|(name, _)| name == "rename_all")
        .and_then(|(_, value)| value);

    let variants_without_explicit_rename: Vec<String> = item
        .variants
        .iter()
        .filter(|variant| {
            !serde_words(&variant.attrs)
                .iter()
                .any(|(name, value)| name == "rename" && value.is_some())
        })
        .map(|variant| variant.ident.to_string())
        .collect();

    assert!(
        container_rename_all.is_none() && variants_without_explicit_rename.is_empty(),
        "the createdBy wire value is still decided by the variant identifier: \
         {CONTRACTS_FILE}::{CREATOR_ENUM} carries rename_all={container_rename_all:?} and \
         these variants have no explicit #[serde(rename = \"…\")]: \
         {variants_without_explicit_rename:?}. Both must change together — an explicit rename \
         under a surviving rename_all still leaves the next identifier edit free to rewrite \
         every conversation index on disk, with no literal anywhere for a review to catch."
    );
}

/// A record written *after* the flip must be readable. Cleared by T-A01.
///
/// Reached by deserialization so it never names an identifier: once a variant
/// exists whose wire value is `brand::canonical().created_by`, this passes.
#[test]
fn a_variant_carries_the_post_rename_wire_value() {
    let _guard = brand::override_canonical(post_rename());
    assert_ne!(
        brand::canonical().created_by,
        brand::LEGACY.created_by,
        "the post-rename injection did not take"
    );

    let canonical = brand::canonical().created_by;
    let parsed = serde_json::from_value::<ConversationCreator>(Value::String(canonical.to_string()));
    assert!(
        parsed.is_ok(),
        "no variant accepts the post-rename createdBy wire value ({canonical:?}), so a record \
         this app writes after the flip cannot be read back by its own deserializer. Variants \
         today: {:?}",
        creator_enum()
            .variants
            .iter()
            .map(|variant| variant.ident.to_string())
            .collect::<Vec<_>>(),
    );

    // And the legacy value must keep working: FORBID-04 forbids re-writing it,
    // not reading it.
    assert!(
        serde_json::from_value::<ConversationCreator>(Value::String(
            brand::LEGACY.created_by.to_string()
        ))
        .is_ok(),
        "the legacy wire value stopped deserializing; every record already on disk is now \
         reported corrupt"
    );
}
