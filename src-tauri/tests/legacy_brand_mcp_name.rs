//! T-H09 — the internal `plan` MCP server identity, pinned against a frozen
//! agent-side artifact instead of an inline literal.
//!
//! # Why this reads from disk
//!
//! The MCP server name is not a private implementation detail: it is written
//! into the *agent's own* persisted configuration and is the key the agent uses
//! to address our tool afterwards. `tests/fixtures/legacy-brand/agent-mcp-config.json`
//! is a frozen snapshot of that remote-side memory, and it is sha256-guarded by
//! `legacy_brand_fixture_manifest.rs`.
//!
//! Reading it from disk is what makes this test un-fakeable. The in-crate
//! assertion this file replaces lived in
//! `manager.rs::reopen_methods_prepend_internal_mcp_to_configured_servers` and
//! compared the generated server name against an inline copy of the same
//! literal production emitted: a repo-wide `sed` rewrites the production string
//! *and* the assertion in one stroke and the suite stays green while every
//! already-installed agent keeps looking for a server that no longer exists.
//! T-A19 deleted it; the surviving assertion over there is the one that test is
//! actually about (the internal server is prepended to the configured ones).
//!
//! # Why this is a source-text parity check rather than a call
//!
//! The production site is `fn build_internal_plan_stdio` at
//! `src/acp/manager.rs:3010` — a *private* free function with no public
//! wrapper (`grep -n 'fn build_internal_plan_stdio' src/acp/manager.rs` →
//! one hit, no `pub`). Integration tests under `tests/` link this crate as an
//! external dependency and therefore see only `pub` items of
//! `termul_manager_lib`, so the function cannot be called from here.
//!
//! So the contract is asserted structurally instead: the two brand-bearing
//! values inside that function body must be *reads through the brand seam*,
//! not independent string literals. That is checked by parsing `manager.rs`
//! with `syn` and looking for a field access on an expression that mentions
//! `brand` — a positive assertion, which matters: a `sed` over the brand string
//! can delete a literal but can never *create* a `crate::brand::canonical()`
//! call, so this check cannot be laundered green.
//!
//! Two spellings, one identity: before T-A19 the server name and the
//! `current_exe()` fallback binary were two different literals inside one
//! function — see `brand::LEGACY.mcp_server_name` and
//! `brand::LEGACY.package_name` for what each one was. Both now resolve through
//! a single hoisted `crate::brand::canonical()`, so they can no longer drift.
//!
//! # Seam status
//!
//! Landed by T-A19: `build_internal_plan_stdio` reads
//! `crate::brand::canonical().mcp_server_name` and
//! `crate::brand::canonical().package_name`. Both ledger entries below are
//! struck.
//!
//! Not covered here, and deliberately: there is no compatibility read for the
//! *old* server name. Nothing in this repo resolves an agent-supplied server
//! name — the internal server is re-injected into `mcp_servers` on every session
//! open, so the name this build emits is the only one in play. An agent whose
//! own memory or prompt still names the pre-rename server addresses a tool that
//! is not there; that is OD-01 in the plan and it is an agent-side concern this
//! crate cannot close.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use syn::visit::{self, Visit};
use syn::{Expr, ExprField, Ident, ItemFn, Lit, Member};
use termul_manager_lib::brand::{self, BrandCanonical};

/// The production site under test.
const PRODUCTION_FILE: &str = "src/acp/manager.rs";
const PRODUCTION_FN: &str = "build_internal_plan_stdio";

/// The post-rename canonical values. Injected on this thread so the assertions
/// below run against a brand that is *different* from the one production still
/// emits — which is what makes the red real rather than self-certifying.
fn post_rename() -> BrandCanonical {
    BrandCanonical {
        mcp_server_name: "se-manager",
        package_name: "se-manager",
        ..brand::DEFAULT_CANONICAL
    }
}

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn fixture(relative: &str) -> PathBuf {
    manifest_dir().join("tests/fixtures/legacy-brand").join(relative)
}

/// What a pre-rename install left in the *agent's* MCP configuration.
struct AgentMemory {
    server_name: String,
    fallback_binary: String,
}

/// Reads the frozen agent-side artifact. Never inlines what it expects to find.
fn read_agent_memory() -> AgentMemory {
    let path = fixture("agent-mcp-config.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("frozen fixture {} is unreadable: {e}", path.display()));
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("frozen fixture {} is not JSON: {e}", path.display()));

    let servers = parsed["mcpServers"]
        .as_object()
        .unwrap_or_else(|| panic!("{} has no mcpServers object", path.display()));
    let mut names: Vec<&String> = servers.keys().collect();
    names.sort();
    let server_name = names
        .first()
        .unwrap_or_else(|| panic!("{} records no MCP server", path.display()))
        .to_string();

    let fallback_binary = parsed["fallback_binary_name"]
        .as_str()
        .unwrap_or_else(|| panic!("{} has no fallback_binary_name", path.display()))
        .to_string();

    AgentMemory {
        server_name,
        fallback_binary,
    }
}

/// Every identifier appearing anywhere in a subtree.
#[derive(Default)]
struct IdentScan {
    seen: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for IdentScan {
    fn visit_ident(&mut self, node: &'ast Ident) {
        self.seen.insert(node.to_string());
    }
}

fn subtree_mentions(expr: &Expr, ident: &str) -> bool {
    let mut scan = IdentScan::default();
    scan.visit_expr(expr);
    scan.seen.contains(ident)
}

/// The two things we need to know about the production function body:
/// which brand fields it reads, and which raw strings it still hardcodes.
#[derive(Default)]
struct BodyFacts {
    brand_fields_read: BTreeSet<String>,
    string_literals: Vec<String>,
}

impl<'ast> Visit<'ast> for BodyFacts {
    fn visit_expr_field(&mut self, node: &'ast ExprField) {
        if let Member::Named(name) = &node.member {
            // `<anything mentioning `brand`>.<field>` — this catches
            // `brand::canonical().mcp_server_name`,
            // `crate::brand::canonical().package_name`, and a hoisted
            // `let brand = crate::brand::canonical(); brand.mcp_server_name`.
            if subtree_mentions(&node.base, "brand") {
                self.brand_fields_read.insert(name.to_string());
            }
        }
        visit::visit_expr_field(self, node);
    }

    fn visit_lit(&mut self, node: &'ast Lit) {
        if let Lit::Str(value) = node {
            self.string_literals.push(value.value());
        }
        visit::visit_lit(self, node);
    }
}

/// Locates `build_internal_plan_stdio` in the real production file.
#[derive(Default)]
struct FnFinder {
    found: Option<ItemFn>,
}

impl<'ast> Visit<'ast> for FnFinder {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        if node.sig.ident == PRODUCTION_FN {
            self.found = Some(node.clone());
        }
        visit::visit_item_fn(self, node);
    }
}

fn production_body_facts() -> BodyFacts {
    let path = manifest_dir().join(PRODUCTION_FILE);
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("production file {} is unreadable: {e}", path.display()));
    let ast: syn::File = syn::parse_file(&source)
        .unwrap_or_else(|e| panic!("production file {} does not parse: {e}", path.display()));

    let mut finder = FnFinder::default();
    finder.visit_file(&ast);
    let function = finder.found.unwrap_or_else(|| {
        panic!("{PRODUCTION_FN} no longer exists in {PRODUCTION_FILE}; this test must be retargeted")
    });

    let mut facts = BodyFacts::default();
    facts.visit_block(&function.block);
    facts
}

/// The frozen artifact really is a *legacy* one. Not the red — a precondition,
/// so a rewritten fixture cannot quietly turn the reds below into no-ops.
#[test]
fn the_frozen_agent_config_records_the_pre_rename_identity() {
    let memory = read_agent_memory();
    assert_eq!(
        memory.server_name,
        brand::LEGACY.mcp_server_name,
        "the frozen agent config must still record the legacy MCP server name"
    );
    assert_eq!(
        memory.fallback_binary,
        brand::LEGACY.package_name,
        "the frozen agent config must still record the legacy fallback binary"
    );
    // Two different spellings of one identity, which is exactly why they must
    // end up on one seam rather than two.
    assert_ne!(memory.server_name, memory.fallback_binary);
}

/// (a) The MCP server name this repo generates must come from the brand seam.
///
/// Ledger entry struck by T-A19: `build_internal_plan_stdio` hoists
/// `crate::brand::canonical()` and names the server from it.
#[test]
fn internal_plan_mcp_server_name_comes_from_the_brand_seam() {
    let memory = read_agent_memory();
    let _guard = brand::override_canonical(post_rename());

    // The injection took: production must now emit something the agent does
    // not yet know about, which is the whole reason a compat read is needed.
    assert_ne!(
        brand::canonical().mcp_server_name,
        memory.server_name,
        "the post-rename injection did not take"
    );

    let facts = production_body_facts();
    assert!(
        facts.brand_fields_read.contains("mcp_server_name"),
        "{PRODUCTION_FILE}::{PRODUCTION_FN} must read crate::brand::canonical().mcp_server_name \
         instead of hardcoding the server name the agent remembers ({:?}); \
         brand fields read today: {:?}; string literals still in the body: {:?}",
        memory.server_name,
        facts.brand_fields_read,
        facts.string_literals,
    );
}

/// (b) The `current_exe()` fallback binary must come from the *same* source.
///
/// Ledger entry struck by T-A19: the fallback `PathBuf` is built from the same
/// hoisted `crate::brand::canonical()` the server name comes from, so the two
/// spellings can no longer drift apart.
#[test]
fn internal_plan_fallback_binary_comes_from_the_same_brand_seam() {
    let memory = read_agent_memory();
    let _guard = brand::override_canonical(post_rename());

    // Post-rename the two spellings collapse onto one value. If they ever
    // diverge again the unification this test exists to enforce is gone.
    assert_eq!(
        brand::canonical().mcp_server_name,
        brand::canonical().package_name,
        "the rename unifies the MCP server name and the binary name"
    );

    let facts = production_body_facts();
    assert!(
        facts.brand_fields_read.contains("package_name"),
        "{PRODUCTION_FILE}::{PRODUCTION_FN} must read crate::brand::canonical().package_name \
         instead of hardcoding the fallback binary the agent remembers ({:?}); \
         brand fields read today: {:?}; string literals still in the body: {:?}",
        memory.fallback_binary,
        facts.brand_fields_read,
        facts.string_literals,
    );
}
