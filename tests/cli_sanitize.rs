//! Round 3 Story 4 — CLI error-printer display-boundary sanitization.
//!
//! Drives the built `stepyard` binary with a path that will fail to
//! open, where the path itself carries raw ANSI escapes and a bidi
//! override (U+202E). The anyhow chain surfaced at `src/main.rs`'s
//! error printer must route that body through `display::sanitize_human`
//! so the operator's terminal cannot be coloured/reversed by content
//! the binary learned at runtime.
//!
//! The red `error:` ANSI prefix the CLI writes itself is part of the
//! CLI's own UI and deliberately stays raw. This test asserts about
//! the error-body section only — specifically, the path string embedded
//! in the parse-failure context.
//!
//! No DB or Docker required: `stepyard validate` is a pure-parse path.

use assert_cmd::Command;
use uuid::Uuid;

/// Matches the red `error:` ANSI prefix the CLI writes at `src/main.rs`.
/// The prefix itself is NOT sanitized — it is first-party UI chrome —
/// so the test strips it before asserting on the untrusted body.
const CLI_RED_PREFIX: &str = "\x1b[31merror:\x1b[0m ";

#[test]
fn cli_error_printer_escapes_ansi_and_bidi_override_in_error_body() {
    // Path will not exist; the uuid keeps concurrent test runs from
    // racing on the same missing filename. The raw `\x1b[31mred\x1b[0m`
    // simulates an ANSI attempt (e.g. a malicious error message that
    // coloured its own output) and `\u{202e}` is the bidi-override
    // classic visual-reversal attack.
    let missing = format!(
        "/tmp/stepyard-nonexistent-\x1b[31mred\x1b[0m-\u{202e}txetdrawkcab-{}.yaml",
        Uuid::new_v4()
    );

    let out = Command::cargo_bin("stepyard")
        .expect("binary built via cargo test")
        .args(["validate", missing.as_str()])
        .output()
        .expect("spawn stepyard");

    assert!(
        !out.status.success(),
        "binary should have exited non-zero for missing file"
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    let body = stderr.strip_prefix(CLI_RED_PREFIX).unwrap_or(&stderr);

    // The raw ANSI colour-switch sequence that arrived via the untrusted
    // path string MUST NOT reach the terminal verbatim.
    assert!(
        !body.contains("\x1b[31mred"),
        "raw ANSI leaked from error body: {stderr:?}"
    );
    // …and its sanitized form must be present instead.
    assert!(
        body.contains("\\u{001b}[31mred"),
        "expected escaped ESC in error body, got: {stderr:?}"
    );

    // U+202E must be absent verbatim — any bidi-override that survives
    // would let the terminal visually reverse surrounding text.
    assert!(
        !body.contains('\u{202e}'),
        "raw U+202E leaked into stderr body: {stderr:?}"
    );
    assert!(
        body.contains("\\u{202e}"),
        "expected escaped U+202E in error body: {stderr:?}"
    );
}
