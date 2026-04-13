//! Stress test — 10 concurrent engines on the same process (Story 2.5).
//!
//! Validates that `Engine: Send + Sync`, that the shared `MockLifecycle`
//! serves many sessions concurrently without data races, and that the
//! Postgres `UNIQUE(session_id, seq)` constraint keeps session logs pristine
//! under load. One of the ten sessions is cancelled mid-flight to prove
//! that a cancel signal does not contaminate its siblings (Invariante 9,
//! NFC2).
//!
//! Skipped gracefully if `MINION_HARNESS_DATABASE_URL` is unset.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use minion_harness::{Engine, HarnessConfig, Step, StepOutcome, Workflow};
use minion_sandbox_orchestrator::{MockLifecycle, SandboxLifecycle};
use minion_session::{migrate, Session, SessionStatus};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

async fn pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("MINION_HARNESS_DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(32) // 10 sessions × ~2 connections each with headroom
        .connect(&url)
        .await
        .expect("reach DB");
    migrate(&pool).await.expect("migrations ok");
    Some(pool)
}

fn five_step_workflow(prefix: &str) -> Workflow {
    Workflow::new(
        format!("stress-{prefix}"),
        (1..=5)
            .map(|i| Step::cmd(format!("s{i}"), format!("echo {prefix}-{i}")))
            .collect(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn ten_concurrent_sessions_with_five_steps_each_stay_isolated_and_fast() {
    let Some(pool) = pool().await else {
        eprintln!("[skip] MINION_HARNESS_DATABASE_URL not set");
        return;
    };

    // Shared MockLifecycle across all sessions — we want to prove the
    // orchestrator itself is race-free.
    let lifecycle: Arc<dyn SandboxLifecycle> = Arc::new(MockLifecycle::new());

    // Per-run tenant id keeps DB queries scoped to THIS test invocation,
    // so back-to-back runs and concurrent test binaries do not see each
    // other's rows.
    let tenant = format!("stress-{}", Uuid::new_v4());

    let start = Instant::now();

    // Spawn 10 engines. Session #4 gets a cancel token we'll flip from
    // the outer task so we can assert that its siblings are unaffected.
    let mut handles = Vec::with_capacity(10);
    let mut cancel_token_for_victim = None;
    let mut victim_session_id = None;

    for i in 0..10 {
        let pool = pool.clone();
        let lifecycle = lifecycle.clone();
        let tenant = tenant.clone();
        let prefix = format!("run{i}");

        // The victim (i = 4) gets Session::new + Engine::new in the outer
        // task so we can grab its cancel token before spawning.
        if i == 4 {
            let session = Session::new(&pool, Uuid::new_v4(), tenant)
                .await
                .expect("new session");
            victim_session_id = Some(session.id());
            let mut engine = Engine::new(
                HarnessConfig::default(),
                session,
                five_step_workflow(&prefix),
                lifecycle,
            );
            cancel_token_for_victim = Some(engine.cancel_token());
            handles.push(tokio::spawn(async move {
                engine.resume().await
            }));
        } else {
            handles.push(tokio::spawn(async move {
                let session = Session::new(&pool, Uuid::new_v4(), tenant)
                    .await
                    .expect("new session");
                let mut engine = Engine::new(
                    HarnessConfig::default(),
                    session,
                    five_step_workflow(&prefix),
                    lifecycle,
                );
                engine.resume().await
            }));
        }
    }

    // Cancel the victim a beat in so at least one real step has run.
    tokio::time::sleep(Duration::from_millis(20)).await;
    cancel_token_for_victim.unwrap().cancel();

    // Collect outcomes.
    let mut results = Vec::new();
    for h in handles {
        results.push(h.await.expect("task"));
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(30),
        "50 step executions should finish under 30s; took {:?}",
        elapsed
    );

    // 9 completions + 1 cancellation.
    let completed = results
        .iter()
        .filter(|r| matches!(r, Ok(StepOutcome::WorkflowCompleted)))
        .count();
    let cancelled = results
        .iter()
        .filter(|r| matches!(r, Ok(StepOutcome::Cancelled)))
        .count();
    assert_eq!(completed, 9, "9 sessions should complete");
    assert_eq!(cancelled, 1, "1 session should be cancelled");

    // Inspect DB — every session's log must be self-contained with dense
    // seq values and no leaks from other sessions.
    let rows: Vec<(Uuid, i64, String)> = sqlx::query_as(
        r#"
        SELECT session_id, seq, payload->>'event' AS event
        FROM session_events
        WHERE session_id IN (
            SELECT id FROM sessions WHERE tenant_id = $1
        )
        ORDER BY session_id, seq
        "#,
    )
    .bind(&tenant)
    .fetch_all(&pool)
    .await
    .expect("stress rows");

    // Group rows by session_id.
    let mut per_session: std::collections::BTreeMap<Uuid, Vec<(i64, String)>> =
        Default::default();
    for (sid, seq, event) in rows {
        per_session.entry(sid).or_default().push((seq, event));
    }

    for (sid, events) in &per_session {
        // Dense seq starting at 1.
        let seqs: Vec<i64> = events.iter().map(|(s, _)| *s).collect();
        let seq_set: HashSet<i64> = seqs.iter().copied().collect();
        assert_eq!(
            seq_set.len(),
            seqs.len(),
            "duplicate seq in session {sid}"
        );
        for (idx, &s) in seqs.iter().enumerate() {
            assert_eq!(s, (idx as i64) + 1, "gap in seq for session {sid}");
        }

        // Must start with workflow_started.
        assert_eq!(
            events.first().map(|(_, e)| e.as_str()),
            Some("workflow_started"),
            "session {sid} must begin with workflow_started"
        );
    }

    // Every completed session: 1 WorkflowStarted + 5x(StepStarted +
    // StepCompleted) + 1 WorkflowCompleted = 12 events.
    let completed_lens: Vec<usize> = per_session
        .iter()
        .filter(|(sid, _)| Some(**sid) != victim_session_id.map(|s| *s.as_uuid()))
        .map(|(_, events)| events.len())
        .collect();
    assert_eq!(
        completed_lens.len(),
        9,
        "expected 9 non-victim sessions in DB"
    );
    for len in &completed_lens {
        assert_eq!(
            *len, 12,
            "completed session should have 12 events, got {len}"
        );
    }

    // The victim — at least a workflow_started, then variable steps, ended
    // by cancellation (session status = cancelled, no workflow_completed
    // at the end).
    let victim_events = &per_session[victim_session_id.unwrap().as_uuid()];
    let last_event = &victim_events.last().unwrap().1;
    assert_ne!(
        last_event, "workflow_completed",
        "cancelled session must not end with workflow_completed"
    );

    let victim_status = sqlx::query_scalar::<_, String>(
        "SELECT status FROM sessions WHERE id = $1",
    )
    .bind(victim_session_id.unwrap().as_uuid())
    .fetch_one(&pool)
    .await
    .expect("victim status");
    assert_eq!(victim_status, "cancelled");

    // No event leak across sessions: the event count from the SQL
    // grouping equals the sum of per-session counts (trivially true by
    // construction, but we also confirm no cross-session duplicates of
    // the (session_id, seq) pair — already enforced by the UNIQUE
    // constraint but worth asserting at the test layer).
    let total_events: usize = per_session.values().map(|v| v.len()).sum();
    let completed_total: usize = 9 * 12;
    assert!(
        total_events >= completed_total,
        "9 completed sessions should contribute at least {completed_total} events; got {total_events}"
    );

    // Every session row exists and has the expected status.
    let statuses: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, status FROM sessions WHERE tenant_id = $1",
    )
    .bind(&tenant)
    .fetch_all(&pool)
    .await
    .expect("statuses");
    let victim_id = victim_session_id.unwrap();
    let victim_uuid = *victim_id.as_uuid();
    for (sid, status) in statuses {
        if sid == victim_uuid {
            assert_eq!(status, "cancelled", "victim must be cancelled");
        } else {
            assert_eq!(
                status, "completed",
                "non-victim session {sid} must be completed"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engine_is_send_sync_compile_time_check() {
    // This function never runs body that touches DB; it exists only for
    // the compile-time assertion. `Engine` must be Send + Sync so
    // `tokio::spawn` can own one across threads.
    fn _assert_send_sync<T: Send + Sync>() {}
    _assert_send_sync::<Engine>();
    _assert_send_sync::<minion_harness::CancelToken>();

    // Runtime body needs to be non-trivial for the test to show up in
    // the summary; just assert status enum equality.
    assert_eq!(SessionStatus::Running.as_str(), "running");
}
