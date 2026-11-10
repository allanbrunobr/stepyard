//! P1-1 (analysis 2026-11-09, ARQ-01 / BUG-CLI-01 / BUG-HARNESS-01) — two
//! real `stepyard execute` processes against one database. The second one's
//! startup reconcile used to mark the first one's live session `failed`.
//! Covered for a long run on v1 (dashboard dispatch and Slack use it) and on
//! v2. The long run's `started_at` is pushed past the grace window, so only
//! the liveness lease can protect it.
//!
//! Needs `DATABASE_URL`; prints `[skip]` and returns without it.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use uuid::Uuid;

mod support;

struct KillOnDrop(Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn stepyard(args: &[&str], url: &str, tenant: &str, cwd: &std::path::Path) -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin("stepyard"));
    cmd.current_dir(cwd)
        .args(args)
        .env("DATABASE_URL", url)
        .env("STEPYARD_TENANT", tenant)
        .env("STEPYARD_NO_FILE_LOGS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd
}

async fn session_of(pool: &sqlx::PgPool, tenant: &str) -> Option<(Uuid, String)> {
    sqlx::query_as::<_, (Uuid, String)>("SELECT id, status FROM sessions WHERE tenant_id = $1")
        .bind(tenant)
        .fetch_optional(pool)
        .await
        .expect("query session")
}

/// Bounded wait with a short poll: the long run must have inserted its
/// session row before the second process starts.
async fn wait_for_session(pool: &sqlx::PgPool, tenant: &str) -> Uuid {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some((id, _)) = session_of(pool, tenant).await {
            return id;
        }
        assert!(
            Instant::now() < deadline,
            "long run never created its session"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Wait until the owner holds its liveness lease, read from `pg_locks` so the
/// test never takes the lock itself (the owner's `pg_try_advisory_lock` would
/// then fail). Backdating `started_at` before this point would create a state
/// that cannot happen for real: the grace window covers the gap between the
/// session INSERT and the lease.
async fn wait_for_lease(pool: &sqlx::PgPool, session: Uuid) {
    let key = stepyard_harness::startup::session_lease_key(session);
    let (high, low) = ((key >> 32) as u32 as i64, key as u32 as i64);
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let held: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_locks WHERE locktype = 'advisory' AND granted \
             AND classid = ($1::bigint)::oid AND objid = ($2::bigint)::oid AND objsubid = 1)",
        )
        .bind(high)
        .bind(low)
        .fetch_one(pool)
        .await
        .expect("query pg_locks");
        if held {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the long run never took its lease"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

async fn second_run_spares_the_first(engine: &str) {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("[skip] DATABASE_URL not set");
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect");
    // A fresh database has no tables until some process migrates it.
    stepyard_session::migrate(&pool).await.expect("migrations");
    let dir = support::temp_git_repo();
    let wf = dir.path().join("long.yaml");
    std::fs::write(
        &wf,
        "name: long-run\nsteps:\n  - name: wait\n    type: cmd\n    run: \"sleep 20\"\n    config:\n      timeout: \"120s\"\n",
    )
    .unwrap();

    let long_tenant = format!("concurrent-long-{}", Uuid::new_v4());
    let mut long = KillOnDrop(
        stepyard(
            &[
                "execute",
                wf.to_str().unwrap(),
                "--no-sandbox",
                "--engine",
                engine,
            ],
            &url,
            &long_tenant,
            dir.path(),
        )
        .spawn()
        .expect("spawn long run"),
    );
    let long_id = wait_for_session(&pool, &long_tenant).await;
    wait_for_lease(&pool, long_id).await;
    sqlx::query("UPDATE sessions SET started_at = NOW() - INTERVAL '1 hour' WHERE id = $1")
        .bind(long_id)
        .execute(&pool)
        .await
        .expect("backdate");

    // The second process runs the startup reconcile, then a short workflow.
    let short_repo = support::temp_git_repo();
    let short_wf = support::repo_path("workflows/hello-world-cmd.yaml");
    let status = stepyard(
        &["execute", short_wf.to_str().unwrap(), "--no-sandbox"],
        &url,
        &format!("concurrent-short-{}", Uuid::new_v4()),
        short_repo.path(),
    )
    .status()
    .expect("run second process");
    assert!(status.success(), "second run failed");

    let (_, status_now) = session_of(&pool, &long_tenant).await.expect("long session");
    assert_eq!(
        status_now, "running",
        "the {engine} run still in progress must not be reconciled by another process"
    );

    let deadline = Instant::now() + Duration::from_secs(90);
    let exit = loop {
        if let Some(code) = long.0.try_wait().expect("try_wait") {
            break code;
        }
        assert!(Instant::now() < deadline, "long run did not finish");
        std::thread::sleep(Duration::from_millis(200));
    };
    assert!(
        exit.success(),
        "long {engine} run must finish successfully: {exit:?}"
    );
    let (_, final_status) = session_of(&pool, &long_tenant).await.expect("long session");
    assert_eq!(final_status, "completed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_execute_does_not_fail_a_live_v1_run() {
    second_run_spares_the_first("v1").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_execute_does_not_fail_a_live_v2_run() {
    second_run_spares_the_first("v2").await;
}
