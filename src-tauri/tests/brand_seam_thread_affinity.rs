//! T-H24 — the brand seam must never be read off the calling thread
//! (FORBID-07, finding F-05).
//!
//! # Why this failure mode is invisible
//!
//! `brand::canonical()` reads a `thread_local` override, and it has to: cargo
//! runs test fns on parallel threads inside one process, so a process-global
//! seam would leak one test's injected value into every sibling and every red
//! in the ledger would become non-deterministic. `brand.rs:155-160` says so, and
//! `brand::tests::override_does_not_leak_into_other_threads` proves it.
//!
//! The consequence is the trap. Anything moved into a
//! `tokio::task::spawn_blocking` or `std::thread::spawn` closure runs on a
//! *different* thread, so it reads `DEFAULT_CANONICAL` no matter what a test
//! injected. A Wave-4/5 implementation that resolves the brand inside such a
//! closure therefore compiles, reads correctly, and **tests green while doing
//! nothing at all** — the harness injects, production ignores the injection,
//! and the assertion passes against the shipped default it was already going to
//! see.
//!
//! `src/web/worktree_api.rs:194` (list) and `:269` (create) are the live
//! instances: both hand `WorktreeManager` to `spawn_blocking`, and the worktree
//! path is built from `brand::canonical().workspace_dir`. `mcp_servers_api`
//! resolves on the request thread and is unaffected.
//!
//! # This file registers zero ledger entries
//!
//! It is a **guard**, not a red. The scan finds zero offences today, so forcing
//! it into `#[should_panic]` would be a fabrication. All of its value comes from
//! the mutation proof recorded in the run report: a `brand::canonical()` call
//! temporarily inserted into `worktree_api.rs`'s `spawn_blocking` closure turns
//! `no_brand_seam_read_inside_a_closure_that_leaves_the_calling_thread` red, and
//! removing it turns it green again with a clean `git diff`.
//!
//! It changes no production code.
//!
//! # The rule, stated positively
//!
//! Resolve on the calling thread, move the resolved `BrandCanonical` (or the
//! single resolved field) into the closure as a captured value. The two
//! regression tests at the bottom execute both sides of that so the failure mode
//! is executable knowledge rather than a comment somebody has to find.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use syn::visit::{self, Visit};
use syn::{Expr, ExprCall, ExprMethodCall, Ident};
use termul_manager_lib::brand::{self, BrandCanonical, DEFAULT_CANONICAL};

/// The one file exempt from the scan, and why.
///
/// `src/brand.rs` deliberately spawns a thread and calls `canonical()` inside it
/// — that is `override_does_not_leak_into_other_threads`, the test that proves
/// the thread-local semantics this whole gate is derived from. Excluding it is
/// not a loophole: it is the single place where reading the seam off-thread is
/// the point.
const EXEMPT: &str = "src/brand.rs";

/// Call names whose argument runs somewhere other than the caller's thread.
///
/// `spawn_blocking` and `thread::spawn` are what FORBID-07 names.
/// `tokio::spawn` is included because an `async` block handed to it is polled by
/// whichever multi-thread worker picks it up, which has exactly the same
/// consequence for a `thread_local` read.
const OFF_THREAD_SPAWNS: &[&str] = &["spawn_blocking", "spawn"];

/// The seam function. Any call to it, however qualified.
const SEAM_FN: &str = "canonical";

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn source_files() -> Vec<String> {
    fn walk(dir: &Path, found: &mut Vec<String>) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("read_dir {} failed: {e}", dir.display()))
            .map(|entry| entry.expect("dir entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, found);
                continue;
            }
            if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(
                    path.strip_prefix(manifest_dir())
                        .expect("under the manifest dir")
                        .components()
                        .map(|component| component.as_os_str().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("/"),
                );
            }
        }
    }
    let mut found = Vec::new();
    walk(&manifest_dir().join("src"), &mut found);
    found.retain(|relative| relative != EXEMPT);
    found
}

/// Every identifier in a subtree.
#[derive(Default)]
struct IdentScan {
    seen: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for IdentScan {
    fn visit_ident(&mut self, node: &'ast Ident) {
        self.seen.insert(node.to_string());
    }
}

fn idents_of(expr: &Expr) -> BTreeSet<String> {
    let mut scan = IdentScan::default();
    scan.visit_expr(expr);
    scan.seen
}

/// A `canonical(...)` call anywhere in a subtree, at any qualification.
#[derive(Default)]
struct SeamReadScan {
    found: bool,
}

impl<'ast> Visit<'ast> for SeamReadScan {
    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        if let Expr::Path(path) = node.func.as_ref() {
            if path
                .path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == SEAM_FN)
            {
                self.found = true;
            }
        }
        visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        if node.method == SEAM_FN {
            self.found = true;
        }
        visit::visit_expr_method_call(self, node);
    }
}

fn reads_the_seam(expr: &Expr) -> bool {
    let mut scan = SeamReadScan::default();
    scan.visit_expr(expr);
    scan.found
}

/// The scan result for one file.
struct Scan {
    /// `<fn name>` for each spawn whose payload reads the seam.
    offences: Vec<String>,
    /// How many off-thread spawns were examined at all. Zero here would mean
    /// the scanner stopped matching and every green below is worthless.
    spawns_examined: usize,
}

#[derive(Default)]
struct SpawnVisitor {
    enclosing: Vec<String>,
    offences: Vec<String>,
    spawns_examined: usize,
}

impl SpawnVisitor {
    fn site(&self) -> String {
        if self.enclosing.is_empty() {
            "<file scope>".to_string()
        } else {
            self.enclosing.join("::")
        }
    }

    /// `spawn_blocking(..)` at any qualification; bare `spawn(..)` only when the
    /// path or receiver names a thread, so `tokio::spawn` and
    /// `thread::Builder::spawn` are caught while an unrelated `spawn` method is
    /// not silently swept in.
    fn is_off_thread(&self, callee_last: &str, qualifiers: &BTreeSet<String>) -> bool {
        if !OFF_THREAD_SPAWNS.contains(&callee_last) {
            return false;
        }
        if callee_last == "spawn_blocking" {
            return true;
        }
        qualifiers.contains("thread")
            || qualifiers.contains("Builder")
            || qualifiers.contains("tokio")
            || qualifiers.contains("async_runtime")
    }

    fn examine(&mut self, arguments: impl Iterator<Item = Expr>) {
        self.spawns_examined += 1;
        for argument in arguments {
            if reads_the_seam(&argument) {
                self.offences.push(self.site());
                break;
            }
        }
    }
}

impl<'ast> Visit<'ast> for SpawnVisitor {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.enclosing.push(node.sig.ident.to_string());
        visit::visit_item_fn(self, node);
        self.enclosing.pop();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.enclosing.push(node.sig.ident.to_string());
        visit::visit_impl_item_fn(self, node);
        self.enclosing.pop();
    }

    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        if let Expr::Path(path) = node.func.as_ref() {
            let segments: BTreeSet<String> = path
                .path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect();
            let last = path
                .path
                .segments
                .last()
                .map(|segment| segment.ident.to_string())
                .unwrap_or_default();
            if self.is_off_thread(&last, &segments) {
                self.examine(node.args.iter().cloned());
            }
        }
        visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        let method = node.method.to_string();
        let receiver_idents = idents_of(&node.receiver);
        if self.is_off_thread(&method, &receiver_idents) {
            self.examine(node.args.iter().cloned());
        }
        visit::visit_expr_method_call(self, node);
    }
}

fn scan_file(relative: &str) -> Scan {
    let path = manifest_dir().join(relative);
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("source file {} is unreadable: {e}", path.display()));
    let ast: syn::File = syn::parse_file(&source)
        .unwrap_or_else(|e| panic!("source file {} does not parse: {e}", path.display()));
    let mut visitor = SpawnVisitor::default();
    visitor.visit_file(&ast);
    Scan {
        offences: visitor.offences,
        spawns_examined: visitor.spawns_examined,
    }
}

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

/// The negative gate. Zero brand-seam reads inside anything that leaves the
/// calling thread.
///
/// Mutation-proved: inserting one `brand::canonical()` into the
/// `spawn_blocking` closure at `src/web/worktree_api.rs:194` turns this red.
#[test]
fn no_brand_seam_read_inside_a_closure_that_leaves_the_calling_thread() {
    let mut offences = Vec::new();
    let mut spawns_examined = 0usize;
    for relative in source_files() {
        let scan = scan_file(&relative);
        spawns_examined += scan.spawns_examined;
        for site in scan.offences {
            offences.push(format!("{relative}::{site}"));
        }
    }

    // A gate that matched nothing would pass for the wrong reason.
    assert!(
        spawns_examined >= 20,
        "the scanner found only {spawns_examined} off-thread spawns across src/, which is far \
         fewer than this repo has. It has stopped matching and its green means nothing."
    );

    assert!(
        offences.is_empty(),
        "brand::canonical() is read inside a closure that runs off the calling thread, at: \
         {offences:?}. The seam is thread_local by necessity (brand.rs:155-160), so that read \
         returns DEFAULT_CANONICAL regardless of any injected override — the code compiles, \
         looks right, and its tests pass while doing nothing. Resolve on the calling thread and \
         move the resolved value in. See FORBID-07."
    );
}

/// The gate is exempting exactly one file, and for the stated reason.
///
/// Without this, a future edit could quietly widen the exemption list and the
/// gate above would go on reporting zero.
#[test]
fn the_exemption_is_exactly_the_seam_module_and_it_really_does_read_off_thread() {
    let scan = scan_file(EXEMPT);
    assert!(
        !scan.offences.is_empty(),
        "{EXEMPT} no longer reads the seam off-thread, so the exemption has no subject and \
         should be deleted rather than left standing"
    );
    assert!(
        !source_files().iter().any(|relative| relative == EXEMPT),
        "{EXEMPT} must be the only exemption and must actually be exempt"
    );
}

// ---------------------------------------------------------------------------
// The two halves of F-05, executed
// ---------------------------------------------------------------------------

fn post_rename() -> BrandCanonical {
    BrandCanonical {
        workspace_dir: ".se-manager",
        ..DEFAULT_CANONICAL
    }
}

/// The correct shape: resolve on the calling thread, move the value in.
#[tokio::test]
async fn a_value_resolved_on_the_calling_thread_survives_into_a_blocking_closure() {
    let _guard = brand::override_canonical(post_rename());
    // Resolved here, on the thread that holds the override.
    let workspace_dir = brand::canonical().workspace_dir;

    let observed = tokio::task::spawn_blocking(move || workspace_dir)
        .await
        .expect("blocking task joins");

    assert_eq!(
        observed,
        post_rename().workspace_dir,
        "a value resolved on the calling thread must be what the closure sees"
    );
    assert_ne!(
        observed, DEFAULT_CANONICAL.workspace_dir,
        "the injection did not take, so this proves nothing"
    );
}

/// The broken shape, side by side with it. This is what FORBID-07 forbids, and
/// the reason the gate above exists: nothing about this code looks wrong.
#[tokio::test]
async fn a_seam_read_inside_a_blocking_closure_silently_ignores_the_override() {
    let _guard = brand::override_canonical(post_rename());
    assert_eq!(
        brand::canonical().workspace_dir,
        post_rename().workspace_dir,
        "the override is in force on this thread"
    );

    let observed = tokio::task::spawn_blocking(|| brand::canonical().workspace_dir)
        .await
        .expect("blocking task joins");

    assert_eq!(
        observed, DEFAULT_CANONICAL.workspace_dir,
        "a seam read inside a blocking closure returns the shipped default — this is the \
         failure mode, and it is silent"
    );
    assert_ne!(
        observed,
        post_rename().workspace_dir,
        "if this ever changes the seam is no longer thread_local and the whole ledger's \
         isolation guarantee has to be re-derived"
    );
}

/// Same thing for `std::thread::spawn`, so the gate's second pattern has a
/// behavioural counterpart rather than only a source-scan one.
#[test]
fn a_seam_read_inside_a_spawned_thread_silently_ignores_the_override() {
    let _guard = brand::override_canonical(post_rename());
    let observed = std::thread::spawn(|| brand::canonical().workspace_dir)
        .join()
        .expect("thread joins");
    assert_eq!(observed, DEFAULT_CANONICAL.workspace_dir);
}
