//! BUG-HARNESS-08 (analysis 2026-11-09): a step's full stdout went into one
//! session-log row, and the log is replayed in full several times per step.
//! A step printing 3 MB must leave a log row of about the 1 MiB cap.
//! Needs `DATABASE_URL`; prints `[skip]` without it.

use std::time::Duration;

use assert_cmd::Command;
use uuid::Uuid;

mod support;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_huge_step_output_is_capped_in_the_session_log() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("[skip] DATABASE_URL not set");
        return;
    };
    let repo = support::temp_git_repo();
    let wf = repo.path().join("big.yaml");
    std::fs::write(
        &wf,
        "name: big-output\nsteps:\n  - name: big\n    type: cmd\n    run: \"head -c 3000000 /dev/zero | tr '\\\\0' a\"\n  - name: after\n    type: cmd\n    run: \"echo done\"\n",
    )
    .unwrap();
    let tenant = format!("output-cap-{}", Uuid::new_v4());
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
        .env("DATABASE_URL", &url)
        .env("STEPYARD_TENANT", &tenant)
        .env("STEPYARD_NO_FILE_LOGS", "1")
        .env_remove("STEPYARD_MAX_STEP_OUTPUT_BYTES")
        .timeout(Duration::from_secs(120))
        .assert()
        .success();

    let pool = sqlx::PgPool::connect(&url).await.expect("connect");
    let largest: i32 = sqlx::query_scalar(
        "SELECT max(octet_length(e.payload::text)) FROM session_events e \
         JOIN sessions s ON s.id = e.session_id WHERE s.tenant_id = $1",
    )
    .bind(&tenant)
    .fetch_one(&pool)
    .await
    .expect("query");
    assert!(
        (1_048_576..1_200_000).contains(&largest),
        "largest log row must be about the 1 MiB cap, got {largest} bytes"
    );
}
