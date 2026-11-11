//! What a later step reads from a v2 `call`: `steps.<call>.stdout` is the
//! scope's `outputs:` rendered after the run, or the last scope step's
//! `stdout` without `outputs:` (docs/STEP-TYPES.md, `call`). The shipped
//! workflows/flaky-test-fix.yaml relies on it.
//! Needs `DATABASE_URL`; prints `[skip]` without it.

use std::time::Duration;

use assert_cmd::Command;
use uuid::Uuid;

mod support;

fn run_v2(yaml: &str, url: &str) -> std::process::Output {
    let repo = support::temp_git_repo();
    let wf = repo.path().join("call.yaml");
    std::fs::write(&wf, yaml).unwrap();
    Command::cargo_bin("stepyard")
        .unwrap()
        .current_dir(repo.path())
        .args([
            "execute",
            wf.to_str().unwrap(),
            "--no-sandbox",
            "--engine",
            "v2",
        ])
        .env("DATABASE_URL", url)
        .env("STEPYARD_TENANT", format!("call-stdout-{}", Uuid::new_v4()))
        .env("STEPYARD_NO_FILE_LOGS", "1")
        .timeout(Duration::from_secs(120))
        .output()
        .expect("run stepyard")
}

#[test]
fn a_call_exposes_its_outputs_template_or_last_stdout_as_stdout() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        assert!(
            std::env::var_os("STEPYARD_REQUIRE_DB").is_none(),
            "DATABASE_URL not set but STEPYARD_REQUIRE_DB is set: a database test must not skip"
        );
        eprintln!("[skip] DATABASE_URL not set");
        return;
    };
    // With `outputs:`: the rendered template.
    let with_outputs = "name: call-out\nscopes:\n  s:\n    steps:\n      - name: run\n        type: cmd\n        run: \"printf hello\"\n    outputs: \"got {{ steps.run.stdout }}\"\nsteps:\n  - name: c\n    type: call\n    scope: s\n  - name: check\n    type: cmd\n    run: \"test {{ steps.c.stdout | shell_quote }} = 'got hello'\"\n";
    let out = run_v2(with_outputs, &url);
    assert!(out.status.success(), "with outputs: {out:?}");
    // Without `outputs:`: the last scope step's stdout.
    let without = "name: call-last\nscopes:\n  s:\n    steps:\n      - name: first\n        type: cmd\n        run: \"printf one\"\n      - name: last\n        type: cmd\n        run: \"printf two\"\nsteps:\n  - name: c\n    type: call\n    scope: s\n  - name: check\n    type: cmd\n    run: \"test {{ steps.c.stdout | shell_quote }} = 'two'\"\n";
    let out = run_v2(without, &url);
    assert!(out.status.success(), "without outputs: {out:?}");
}
