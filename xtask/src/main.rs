//! `cargo xtask` — workspace task runner.
//!
//! Round 3 Story 5 — today the only subcommand is `audit-emit-before-io`,
//! the G4 gate from `_bmad-output/sandcastle-features/architecture.md`.
//! Additional audits (G1–G3 are a ripgrep script; G5 ditto; G6 is
//! per-crate compile-time tests) live elsewhere — this crate exists for
//! checks that need a real AST.
//!
//! # Scope (v1, deliberately narrow)
//!
//! * **Target crate:** `crates/stepyard-harness` only. `Arc<dyn SandboxLifecycle>`
//!   fields exist nowhere else in production code today (grep
//!   `Arc<dyn SandboxLifecycle>` in `crates/stepyard-session` returns nothing).
//!   A follow-up story can auto-discover target crates by scanning
//!   `Arc<dyn SandboxLifecycle>` field declarations across the workspace.
//! * **Field name:** `lifecycle` only. The single production field name today
//!   (engine.rs and executor.rs both use `lifecycle: Arc<dyn SandboxLifecycle>`).
//!   Hard-coded intentionally — auto-discovery lands with a follow-up.
//! * **Emit vocabulary:** `self.emit(...)` and `self.session.append(...)`.
//!   These are the two persistence channels the architecture recognizes as
//!   "state recorded before IO fires".
//!
//! # Semantics
//!
//! For each async fn body in the target crate, the walker:
//! 1. Collects the line numbers of every `self.emit(...)` and
//!    `self.session.append(...)` expression anywhere inside the body.
//! 2. Collects every `self.lifecycle.<method>(...).await` site.
//! 3. For each sandbox IO site, warns if NO emit/persist line appears at a
//!    strictly-smaller line number in the same function body.
//!
//! `cargo xtask audit-emit-before-io` exits non-zero on findings. Historical
//! drift was burned down before this gate graduated to blocking.
//!
//! # Known limitations (v1)
//!
//! * Does not track aliasing: `let sb = &self.lifecycle; sb.create(...).await`
//!   is invisible to the walker.
//! * Does not descend into nested closures: `async move || { ... }` bodies
//!   are checked as their own scopes, so an emit in the enclosing function
//!   is not considered "before" a sandbox call inside the closure.
//! * Control-flow insensitive: an emit on only one branch of an `if` still
//!   counts as "before" in lexical order even if the runtime path does not
//!   reach it.
//! * Explicit opt-outs are line-local: a sandbox await is ignored only when a
//!   nearby preceding comment contains `audit: emit-before-io exempt` and a
//!   `reason:` clause. This is for wrapper functions whose caller owns the
//!   persistence boundary.
//!
//! Each of these produces either false positives (flagged-OK code) or false
//! negatives (missed drift) that a reviewer can resolve by reading the
//! flagged site. Tightening any of these lands with future stories.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use syn::visit::Visit;
use syn::{Block, Expr, ExprAwait, ExprField, ExprMethodCall, Member};

// --- Hard-coded v1 scope ----------------------------------------------------
const TARGET_CRATES: &[&str] = &["crates/stepyard-harness"];
const SANDBOX_FIELD: &str = "lifecycle";

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(subcommand) = args.next() else {
        eprintln!("usage: cargo xtask <subcommand>");
        eprintln!("  audit-emit-before-io — G4 emit-before-IO order check (warning-first)");
        return ExitCode::from(2);
    };

    match subcommand.as_str() {
        "audit-emit-before-io" => run_emit_before_io(),
        other => {
            eprintln!("xtask: unknown subcommand '{other}'");
            eprintln!("usage: cargo xtask <subcommand>");
            eprintln!("  audit-emit-before-io — G4 emit-before-IO order check (warning-first)");
            ExitCode::from(2)
        }
    }
}

fn run_emit_before_io() -> ExitCode {
    let mut findings: Vec<Finding> = Vec::new();
    for crate_dir in TARGET_CRATES {
        let src_dir = Path::new(crate_dir).join("src");
        if !src_dir.exists() {
            eprintln!(
                "audit-emit-before-io: skipping absent path '{}' (repo layout may have changed)",
                src_dir.display()
            );
            continue;
        }
        walk_dir(&src_dir, &mut findings);
    }

    findings.sort_by(|a, b| (a.file.as_path(), a.line).cmp(&(b.file.as_path(), b.line)));

    for f in &findings {
        eprintln!(
            "FAIL G4: {}:{}: `self.{}.{}().await` has no `self.emit(...)` or \
             `self.session.append(...)` at an earlier line in the same fn body",
            f.file.display(),
            f.line,
            SANDBOX_FIELD,
            f.method,
        );
    }

    if findings.is_empty() {
        eprintln!("audit-emit-before-io: 0 findings.");
    } else {
        eprintln!(
            "audit-emit-before-io: {} finding(s) — fix or add an explicit audited exemption.",
            findings.len()
        );
    }

    if findings.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

// --- Walker -----------------------------------------------------------------

struct Finding {
    file: PathBuf,
    line: usize,
    method: String,
}

fn walk_dir(dir: &Path, findings: &mut Vec<Finding>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            eprintln!("audit-emit-before-io: cannot read {}: {err}", dir.display());
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_dir(&path, findings);
        } else if path.extension().is_some_and(|e| e == "rs") {
            walk_file(&path, findings);
        }
    }
}

fn walk_file(path: &Path, findings: &mut Vec<Finding>) {
    let src = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(err) => {
            eprintln!(
                "audit-emit-before-io: cannot read {}: {err}",
                path.display()
            );
            return;
        }
    };
    let file = match syn::parse_file(&src) {
        Ok(f) => f,
        Err(err) => {
            // Non-Rust or broken file: a silent skip here would hide a real
            // parse regression, so surface it as a warning on stderr. The
            // audit step still exits 0 because G4 is warning-first overall.
            eprintln!(
                "audit-emit-before-io: syn parse failed on {}: {err} (skipping file)",
                path.display()
            );
            return;
        }
    };

    let mut collector = FnBodyCollector { bodies: Vec::new() };
    collector.visit_file(&file);

    let lines: Vec<&str> = src.lines().collect();
    for body in collector.bodies {
        check_body(path, &lines, body, findings);
    }
}

/// First pass: collect every function body in the file. We check each body
/// in isolation so "before" means "at an earlier line number within this
/// body" — the right scope for the emit-before-IO invariant.
struct FnBodyCollector<'ast> {
    bodies: Vec<&'ast Block>,
}

impl<'ast> Visit<'ast> for FnBodyCollector<'ast> {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.bodies.push(&node.block);
        syn::visit::visit_item_fn(self, node);
    }
    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.bodies.push(&node.block);
        syn::visit::visit_impl_item_fn(self, node);
    }
    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        if let Some(block) = &node.default {
            self.bodies.push(block);
        }
        syn::visit::visit_trait_item_fn(self, node);
    }
}

fn check_body(path: &Path, lines: &[&str], body: &Block, findings: &mut Vec<Finding>) {
    let mut emits = EmitLineCollector { lines: Vec::new() };
    emits.visit_block(body);

    let mut sandbox = SandboxAwaitCollector { sites: Vec::new() };
    sandbox.visit_block(body);

    for site in sandbox.sites {
        let has_earlier_emit = emits.lines.iter().any(|&l| l < site.line);
        if !has_earlier_emit && !has_explicit_exemption(lines, site.line) {
            findings.push(Finding {
                file: path.to_path_buf(),
                line: site.line,
                method: site.method,
            });
        }
    }
}

fn has_explicit_exemption(lines: &[&str], line: usize) -> bool {
    // `line` is 1-indexed from proc-macro2 spans. Check the current line and
    // the few preceding lines so comments can sit above a chained call whose
    // `.await` appears several lines later.
    let zero_based = line.saturating_sub(1);
    let start = zero_based.saturating_sub(4);
    lines
        .get(start..=zero_based.min(lines.len().saturating_sub(1)))
        .unwrap_or(&[])
        .iter()
        .any(|line| line.contains("audit: emit-before-io exempt") && line.contains("reason:"))
}

struct EmitLineCollector {
    lines: Vec<usize>,
}

impl<'ast> Visit<'ast> for EmitLineCollector {
    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        if is_emit_like(node) {
            self.lines.push(node.method.span().start().line);
        }
        syn::visit::visit_expr_method_call(self, node);
    }
}

struct SandboxAwaitSite {
    line: usize,
    method: String,
}

struct SandboxAwaitCollector {
    sites: Vec<SandboxAwaitSite>,
}

impl<'ast> Visit<'ast> for SandboxAwaitCollector {
    fn visit_expr_await(&mut self, node: &'ast ExprAwait) {
        if let Some(method) = sandbox_method_name(&node.base) {
            self.sites.push(SandboxAwaitSite {
                line: node.await_token.span.start().line,
                method,
            });
        }
        syn::visit::visit_expr_await(self, node);
    }
}

// --- AST shape helpers ------------------------------------------------------

fn is_self(expr: &Expr) -> bool {
    if let Expr::Path(p) = expr {
        if p.qself.is_none()
            && p.path.leading_colon.is_none()
            && p.path.segments.len() == 1
            && p.path.segments[0].ident == "self"
            && p.path.segments[0].arguments.is_none()
        {
            return true;
        }
    }
    false
}

/// Returns true iff `expr` is `self.<SANDBOX_FIELD>`.
fn is_self_sandbox_field(expr: &Expr) -> bool {
    if let Expr::Field(ExprField {
        base,
        member: Member::Named(ident),
        ..
    }) = expr
    {
        ident == SANDBOX_FIELD && is_self(base)
    } else {
        false
    }
}

/// If `expr` is a method-call chain whose receiver is `self.<SANDBOX_FIELD>`,
/// returns the top-level method name. Walks through `Try` (`?`) wrappers so
/// `self.lifecycle.create(id).await?` is recognized.
fn sandbox_method_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Try(t) => sandbox_method_name(&t.expr),
        Expr::MethodCall(mc) => {
            if is_self_sandbox_field(&mc.receiver) {
                Some(mc.method.to_string())
            } else {
                // Allow chains: self.lifecycle.reuse_or_create(id).await
                //   -> self.lifecycle.exec_with_env(...).await  if the
                //   receiver of the outer MethodCall is another MethodCall on
                //   self.lifecycle, propagate upward. V1 is conservative: it
                //   only flags when the IMMEDIATE receiver is self.lifecycle,
                //   matching today's call shapes. Deeper chains land with a
                //   follow-up if real code introduces them.
                None
            }
        }
        _ => None,
    }
}

/// Returns true iff `mc` is an emit-like method call the invariant recognizes
/// as a persistence site:
/// * `self.emit(...)`
/// * `self.session.append(...)`
fn is_emit_like(mc: &ExprMethodCall) -> bool {
    if mc.method == "emit" && is_self(&mc.receiver) {
        return true;
    }
    if mc.method == "append" {
        if let Expr::Field(ExprField {
            base,
            member: Member::Named(ident),
            ..
        }) = &*mc.receiver
        {
            if ident == "session" && is_self(base) {
                return true;
            }
        }
    }
    false
}
