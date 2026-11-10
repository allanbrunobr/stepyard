//! MANUT-15 (analysis 2026-11-09): diagnostic logs had no timestamp and no
//! session id, so a log line could not be matched to its session log.
//! With `STEPYARD_LOG_FORMAT=json`, a warning raised inside a v2 step must be
//! a JSON line carrying a timestamp and the `session_id` of that session.
//! Needs `DATABASE_URL`; prints `[skip]` without it.

use std::time::Duration;

use assert_cmd::Command;
use uuid::Uuid;

mod support;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn json_logs_carry_the_session_id_of_the_running_step() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("[skip] DATABASE_URL not set");
        return;
    };
    let repo = support::temp_git_repo();
    let wf = repo.path().join("loop.yaml");
    // A repeat without a breaking gate hits max_iterations, which the
    // harness reports with a tracing warning from inside `Engine::step`.
    std::fs::write(
        &wf,
        "name: log-json\nscopes:\n  inner:\n    steps:\n      - name: tick\n        type: cmd\n        run: \"echo tick\"\nsteps:\n  - name: loop\n    type: repeat\n    scope: inner\n    max_iterations: 1\n",
    )
    .unwrap();
    let tenant = format!("log-json-{}", Uuid::new_v4());
    let out = Command::cargo_bin("stepyard")
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
        .env("STEPYARD_LOG_FORMAT", "json")
        .env("RUST_LOG", "warn")
        .timeout(Duration::from_secs(120))
        .output()
        .expect("run stepyard");
    assert!(out.status.success(), "stepyard failed: {out:?}");

    let pool = sqlx::PgPool::connect(&url).await.expect("connect");
    let session_id: Uuid = sqlx::query_scalar("SELECT id FROM sessions WHERE tenant_id = $1")
        .bind(&tenant)
        .fetch_one(&pool)
        .await
        .expect("one session for the tenant");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let warning = stdout
        .lines()
        .filter(|l| l.starts_with('{'))
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|v| {
            v["fields"]["message"]
                .as_str()
                .is_some_and(|m| m.contains("max_iterations"))
        })
        .unwrap_or_else(|| panic!("no JSON max_iterations warning in stdout:\n{stdout}"));

    assert!(
        warning["timestamp"].is_string(),
        "JSON log line must carry a timestamp: {warning}"
    );
    assert_eq!(
        warning["span"]["session_id"].as_str(),
        Some(session_id.to_string().as_str()),
        "warning must carry the running session's id: {warning}"
    );
}
