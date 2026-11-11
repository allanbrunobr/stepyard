//! `cargo xtask` — workspace task runner.
//!
//! Round 3 Story 5 — today the only subcommand is `audit-emit-before-io`,
//! the G4 (emit-before-IO) gate described in CONTRIBUTING.md.
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

/// The workspace root, independent of the directory `cargo xtask` was
/// invoked from (BUG-CLI-15: a CWD-relative path made the audit "skip the
/// absent path" and pass with 0 findings when run from a subdirectory).
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level below the workspace root")
        .to_path_buf()
}

fn run_emit_before_io() -> ExitCode {
    let mut findings: Vec<Finding> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let root = workspace_root();
    for crate_dir in TARGET_CRATES {
        let src_dir = root.join(crate_dir).join("src");
        if !src_dir.exists() {
            // Fail closed: an audit that cannot see its target must not pass.
            errors.push(format!(
                "target path '{}' does not exist",
                src_dir.display()
            ));
            continue;
        }
        walk_dir(&src_dir, &mut findings, &mut errors);
    }

    if !errors.is_empty() {
        for e in &errors {
            eprintln!("audit-emit-before-io: ERROR: {e}");
        }
        eprintln!(
            "audit-emit-before-io: {} file(s)/path(s) could not be audited — failing closed",
            errors.len()
        );
        return ExitCode::from(2);
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

fn walk_dir(dir: &Path, findings: &mut Vec<Finding>, errors: &mut Vec<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            errors.push(format!("cannot read {}: {err}", dir.display()));
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_dir(&path, findings, errors);
        } else if path.extension().is_some_and(|e| e == "rs") {
            walk_file(&path, findings, errors);
        }
    }
}

fn walk_file(path: &Path, findings: &mut Vec<Finding>, errors: &mut Vec<String>) {
    let src = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(err) => {
            errors.push(format!("cannot read {}: {err}", path.display()));
            return;
        }
    };
    audit_source_into(path, &src, findings, errors);
}

/// Audit `src` and record the outcome. A file the walker cannot parse is a
/// file it did not audit: it is counted as an error (which fails the run)
/// instead of being skipped with exit 0 (TEST-08).
fn audit_source_into(
    path: &Path,
    src: &str,
    findings: &mut Vec<Finding>,
    errors: &mut Vec<String>,
) {
    match check_source(path, src) {
        Ok(found) => findings.extend(found),
        Err(err) => errors.push(format!("syn parse failed on {}: {err}", path.display())),
    }
}

/// Audit one source file. `Err` when it does not parse.
fn check_source(path: &Path, src: &str) -> Result<Vec<Finding>, syn::Error> {
    let file = syn::parse_file(src)?;
    let mut collector = FnBodyCollector { bodies: Vec::new() };
    collector.visit_file(&file);

    let lines: Vec<&str> = src.lines().collect();
    let mut findings = Vec::new();
    for body in collector.bodies {
        check_body(path, &lines, body, &mut findings);
    }
    Ok(findings)
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

/// Returns true iff `expr` is `self.<SANDBOX_FIELD>` or the accessor form
/// `self.<SANDBOX_FIELD>()` (TEST-08: scope.rs reaches the lifecycle through
/// the accessor, which the walker used to miss).
fn is_self_sandbox_field(expr: &Expr) -> bool {
    if let Expr::MethodCall(mc) = expr {
        return mc.method == SANDBOX_FIELD && mc.args.is_empty() && is_self(&mc.receiver);
    }
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

#[cfg(test)]
mod tests {
    //! The G4 gate had no tests (TEST-08); each case below is a shape the
    //! gate must flag or accept.
    use super::*;

    fn audit(src: &str) -> Vec<Finding> {
        check_source(Path::new("t.rs"), src).expect("parses")
    }

    #[test]
    fn flags_sandbox_io_without_an_earlier_emit() {
        let found = audit(
            "impl E { async fn f(&mut self) { self.lifecycle.destroy_by_session(id).await; } }",
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].method, "destroy_by_session");
    }

    #[test]
    fn accepts_sandbox_io_after_an_emit_or_a_session_append() {
        // The gate compares line numbers ("an earlier line"), so each call
        // sits on its own line, as in real code.
        for src in [
            "impl E { async fn f(&mut self) {\n self.emit(ev).await;\n self.lifecycle.destroy(id).await;\n } }",
            "impl E { async fn f(&mut self) {\n self.session.append(p).await;\n self.lifecycle.exec(id).await;\n } }",
        ] {
            assert!(audit(src).is_empty(), "{src}");
        }
    }

    #[test]
    fn sees_the_accessor_form_used_by_scope_rs() {
        let found = audit(
            "impl E { async fn f(&mut self) {\n let _ = self\n .lifecycle()\n .destroy_by_session(id)\n .await;\n } }",
        );
        assert_eq!(found.len(), 1, "self.lifecycle().<m>() must be audited too");
    }

    #[test]
    fn honours_an_explicit_audited_exemption() {
        let src = "impl E { async fn f(&mut self) {\n // audit: emit-before-io exempt - reason: caller persists\n self.lifecycle.destroy(id).await;\n } }";
        assert!(audit(src).is_empty());
    }

    #[test]
    fn a_file_that_does_not_parse_is_counted_as_an_error_not_a_pass() {
        let (mut findings, mut errors) = (Vec::new(), Vec::new());
        audit_source_into(
            Path::new("t.rs"),
            "fn broken( {",
            &mut findings,
            &mut errors,
        );
        assert!(findings.is_empty());
        assert_eq!(errors.len(), 1, "an unparsable file must make the run fail");
        assert!(errors[0].contains("t.rs"), "{errors:?}");
    }

    #[test]
    fn the_target_crate_is_found_from_any_working_directory() {
        let src = workspace_root().join(TARGET_CRATES[0]).join("src");
        assert!(src.is_dir(), "{} must exist", src.display());
    }
}
