//! The D8 startup reconcile — binary-side re-export.
//!
//! # Integration contract (Story 2.4)
//!
//! The binary's execute path reaches the reconcile via
//! `crate::startup::reconcile(..)` (binary tree) or
//! `minion_engine::startup::reconcile(..)` (lib tree). Both resolve to the
//! same function symbol re-exported from [`minion_harness::startup`].
//!
//! # Deviations from Story 2.4 as-written
//!
//! 1. **Called from `execute_v2`, not `main()`.** The Story's literal wording
//!    places `reconcile(&pg, &lifecycle).await?` in `main()`, but `main()`
//!    does not construct a `PgPool` or `SandboxLifecycle` today — those are
//!    built inside the execute path. Moving pool + lifecycle construction
//!    up into `main` would be a mechanical but disruptive refactor. The
//!    functional invariant — "reconcile runs before any `Engine::new()`" —
//!    is preserved because `execute_v2` is the only engine-constructing path
//!    and reconcile runs there before the first `HarnessEngine::new(..)`.
//!
//! 2. **Logic in `minion_harness::startup`; this file re-exports.** Story
//!    wording places the function in `src/startup.rs`. It does — but as a
//!    `pub use` re-export. The implementation lives in the harness crate so
//!    `crates/minion-harness/tests/startup_reconcile.rs` can drive it
//!    without introducing a circular `minion-engine → minion-harness`
//!    build dependency (the harness crate cannot depend on the binary).

// `ReconcileError` is part of the public `reconcile` signature (it is the
// `Err` half of the returned `Result`) — callers that want to match on its
// variants reach it via this re-export. The binary tree does not currently
// name the type directly, so `#[allow(unused_imports)]` keeps the compiler
// quiet without forcing us to synthesise an artificial use site.
#[allow(unused_imports)]
pub use minion_harness::startup::{reconcile, ReconcileError, ReconcileReport};
