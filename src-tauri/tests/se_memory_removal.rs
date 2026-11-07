//! Removal contract for the legacy se-memory process role.
//!
//! These are source/AST tests, deliberately NOT CARGO_BIN_EXE smoke tests:
//! launching a regressed desktop/server could open a GUI, migrate stores, or
//! start listeners before a timeout could kill it. Requiring the tombstone to
//! be main's first statement proves precedence over observer/core dispatch,
//! logging, argument parsing, updates, and every other setup action, safely.
//! Retained session-memory data/tool coverage lives in mcp_core_memory_merge.rs.

use std::fs;
use std::path::{Path, PathBuf};
use syn::visit::{self, Visit};
use syn::{BinOp, Expr, ExprCall, ExprMethodCall, Item, Lit, Pat, Stmt};

const FLAG: &str = "--memory-mcp-server";
const DIAGNOSTIC: &str = "se-memory has been removed; migrate to se-mcp (session-memory).";

fn source(relative: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)).unwrap()
}

fn is_path(expr: &Expr, expected: &[&str]) -> bool {
    let Expr::Path(path) = expr else { return false };
    path.attrs.is_empty()
        && path.qself.is_none()
        && path.path.segments.len() == expected.len()
        && path
            .path
            .segments
            .iter()
            .zip(expected)
            .all(|(segment, name)| segment.ident == *name && segment.arguments.is_empty())
}

fn call<'a>(expr: &'a Expr, expected: &[&str]) -> &'a ExprCall {
    let Expr::Call(call) = expr else {
        panic!("expected direct call")
    };
    assert!(call.attrs.is_empty());
    assert!(is_path(&call.func, expected), "unexpected callee");
    call
}

fn method<'a>(expr: &'a Expr, expected: &str) -> &'a ExprMethodCall {
    let Expr::MethodCall(call) = expr else {
        panic!("expected method call")
    };
    assert!(call.attrs.is_empty());
    assert_eq!(call.method, expected);
    assert!(call.turbofish.is_none());
    call
}

fn integer(expr: &Expr, expected: u8) {
    let Expr::Lit(literal) = expr else {
        panic!("expected integer literal")
    };
    assert!(literal.attrs.is_empty());
    let Lit::Int(value) = &literal.lit else {
        panic!("expected integer")
    };
    assert_eq!(value.base10_parse::<u8>().unwrap(), expected);
}

fn assert_tombstone_first(text: &str, standalone: bool) {
    let file = syn::parse_file(text).expect("binary must parse");
    let mains: Vec<_> = file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Fn(function) if function.sig.ident == "main" => Some(function),
            _ => None,
        })
        .collect();
    assert_eq!(
        mains.len(),
        1,
        "exactly one main, not cfg-specific alternatives"
    );
    let main = mains[0];
    assert!(
        main.attrs.is_empty(),
        "main must not be wrapped by a startup macro"
    );
    assert!(
        main.sig.asyncness.is_none(),
        "no async runtime before the tombstone"
    );
    let Some(Stmt::Expr(Expr::If(guard), _)) = main.block.stmts.first() else {
        panic!("tombstone must be the FIRST action, before observer/setup/core/update");
    };
    assert!(guard.attrs.is_empty(), "tombstone must not be cfg gated");
    assert!(guard.else_branch.is_none());

    // Inspect the full condition, not a source substring that comments or a
    // later branch could satisfy: args[_os]().skip(1).any(|arg| arg == FLAG).
    let any = method(&guard.cond, "any");
    assert_eq!(any.args.len(), 1);
    let skip = method(&any.receiver, "skip");
    assert_eq!(skip.args.len(), 1);
    integer(&skip.args[0], 1);
    let args = match &*skip.receiver {
        expr if is_path_call(expr, &["std", "env", "args_os"]) => {
            call(expr, &["std", "env", "args_os"])
        }
        expr => call(expr, &["std", "env", "args"]),
    };
    assert!(args.args.is_empty());
    let Expr::Closure(predicate) = &any.args[0] else {
        panic!("expected flag predicate")
    };
    assert!(predicate.attrs.is_empty());
    assert_eq!(predicate.inputs.len(), 1);
    let Pat::Ident(argument) = &predicate.inputs[0] else {
        panic!("expected argument binding")
    };
    assert!(argument.attrs.is_empty());
    let Expr::Binary(equals) = &*predicate.body else {
        panic!("expected exact flag equality")
    };
    assert!(equals.attrs.is_empty());
    assert!(matches!(equals.op, BinOp::Eq(_)));
    assert!(is_path(&equals.left, &[&argument.ident.to_string()]));
    let Expr::Lit(flag) = &*equals.right else {
        panic!("expected literal flag")
    };
    assert!(flag.attrs.is_empty());
    let Lit::Str(flag) = &flag.lit else {
        panic!("expected flag string")
    };
    assert_eq!(flag.value(), FLAG);

    assert_eq!(
        guard.then_branch.stmts.len(),
        2,
        "only diagnostic and exit are allowed"
    );
    let Stmt::Macro(message) = &guard.then_branch.stmts[0] else {
        panic!("expected eprintln!")
    };
    assert!(message.attrs.is_empty());
    assert!(message.mac.path.is_ident("eprintln"));
    let message: syn::LitStr = syn::parse2(message.mac.tokens.clone())
        .expect("one fixed string, no formatting or argument disclosure");
    assert_eq!(message.value(), DIAGNOSTIC);
    let Stmt::Expr(exit, _) = &guard.then_branch.stmts[1] else {
        panic!("expected immediate exit")
    };
    let exit = if standalone {
        let Expr::Return(ret) = exit else {
            panic!("standalone must return ExitCode")
        };
        assert!(ret.attrs.is_empty());
        call(
            ret.expr.as_ref().expect("return exit code"),
            &["ExitCode", "from"],
        )
    } else {
        call(exit, &["std", "process", "exit"])
    };
    assert_eq!(exit.args.len(), 1);
    integer(&exit.args[0], 2);
}

fn is_path_call(expr: &Expr, expected: &[&str]) -> bool {
    matches!(expr, Expr::Call(call) if is_path(&call.func, expected))
}

#[test]
fn desktop_rejects_legacy_flag_before_any_other_action() {
    assert_tombstone_first(&source("src/main.rs"), false);
}

#[test]
fn standalone_rejects_legacy_flag_before_any_other_action() {
    assert_tombstone_first(&source("src/server_main.rs"), true);
}

fn rust_sources(path: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_sources(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

struct NoLegacySurface<'a>(&'a Path);

impl<'ast> Visit<'ast> for NoLegacySurface<'_> {
    fn visit_ident(&mut self, ident: &'ast syn::Ident) {
        let name = ident.to_string();
        assert!(
            ![
                "stdio_mcp",
                "memory_index_mcp_invocation",
                "memory_index_universal_mcp_invocation",
                "memory_index_mcp_invocation_cmd",
                "memory_index_universal_mcp_invocation_cmd",
                "METHOD_MEMORY_MCP_INVOCATION",
                "METHOD_MEMORY_UNIVERSAL_MCP_INVOCATION",
            ]
            .contains(&name.as_str()),
            "legacy module/command/ACP dispatch {name} survived in {}",
            self.0.display()
        );
        visit::visit_ident(self, ident);
    }

    fn visit_lit_str(&mut self, literal: &'ast syn::LitStr) {
        assert!(
            !["memoryMcpInvocation", "memoryUniversalMcpInvocation"]
                .contains(&literal.value().as_str()),
            "legacy ACP invocation method survived in {}",
            self.0.display()
        );
        visit::visit_lit_str(self, literal);
    }

    // syn treats macro bodies as opaque. Walk every token tree too so a
    // generate_handler![memory_index_mcp_invocation_cmd] registration is covered.
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        for name in mac
            .tokens
            .to_string()
            .split(|c: char| !c.is_alphanumeric() && c != '_')
        {
            if let Ok(ident) = syn::parse_str::<syn::Ident>(name) {
                self.visit_ident(&ident);
            }
        }
        visit::visit_macro(self, mac);
    }
}

#[test]
fn legacy_surface_guard_rejects_command_definitions_and_macro_registrations() {
    let fixture_path = Path::new("in-memory-regression.rs");
    let supported = syn::parse_file(
        "fn memory_index_stats_cmd() {} fn main() { tauri::generate_handler![memory_index_stats_cmd]; }",
    ).unwrap();
    NoLegacySurface(fixture_path).visit_file(&supported);

    for name in [
        "memory_index_mcp_invocation_cmd",
        "memory_index_universal_mcp_invocation_cmd",
    ] {
        for fixture in [
            format!("#[tauri::command] pub async fn {name}() {{}}"),
            format!(
                "fn main() {{ tauri::generate_handler![crate::memory_index::commands::{name}]; }}"
            ),
        ] {
            let ast = syn::parse_file(&fixture).unwrap();
            assert!(
                std::panic::catch_unwind(|| NoLegacySurface(fixture_path).visit_file(&ast))
                    .is_err(),
                "legacy command definition or macro registration escaped the guard: {fixture}"
            );
        }
    }
}

#[test]
fn no_legacy_stdio_module_tauri_commands_or_acp_dispatch_remain() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    assert!(
        !root.join("memory_index/stdio_mcp.rs").exists(),
        "delete the legacy implementation"
    );
    let mut files = Vec::new();
    rust_sources(&root, &mut files);
    assert!(!files.is_empty());
    for path in files {
        let text = fs::read_to_string(&path).unwrap();
        let ast =
            syn::parse_file(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        NoLegacySurface(&path).visit_file(&ast);
    }
    assert!(
        root.join("mcp_core/mod.rs").is_file(),
        "the supported MCP Core must remain"
    );
    let lib = syn::parse_file(&source("src/lib.rs")).unwrap();
    assert!(
        lib.items.iter().any(|item| matches!(item,
            Item::Mod(module) if module.ident == "mcp_core"
                && matches!(module.vis, syn::Visibility::Public(_))
        )),
        "the public MCP Core module must remain"
    );
}

#[test]
fn structural_guard_rejects_unsafe_or_incomplete_tombstones() {
    // Mutation checks protect this test's own safety/order assertions without
    // ever launching either application binary or opening user-owned state.
    for standalone in [false, true] {
        let exit = if standalone {
            "return ExitCode::from(2);"
        } else {
            "std::process::exit(2);"
        };
        let signature = if standalone {
            "fn main() -> ExitCode"
        } else {
            "fn main()"
        };
        let fixture = format!(
            "{signature} {{ if std::env::args_os().skip(1).any(|a| a == \"{FLAG}\") {{ eprintln!(\"{DIAGNOSTIC}\"); {exit} }} observer_setup(); }}"
        );
        assert_tombstone_first(&fixture, standalone);
        for bad in [
            fixture.replacen("{ if", "{ observer_setup(); if", 1),
            fixture.replace("skip(1)", "skip(2)"),
            fixture.replace(".any(", ".all("),
            fixture.replace("==", "!="),
            fixture
                .replace("from(2)", "from(0)")
                .replace("exit(2)", "exit(0)"),
            fixture.replace("{ eprintln!", "{ setup(); eprintln!"),
            fixture.replace("eprintln!", "println!"),
            fixture.replace(DIAGNOSTIC, "wrong migration diagnostic"),
            fixture.replacen("{ if", "{ #[cfg(any())] if", 1),
        ] {
            assert!(
                std::panic::catch_unwind(|| assert_tombstone_first(&bad, standalone)).is_err(),
                "unsafe tombstone mutation escaped the guard: {bad}"
            );
        }
    }
}
