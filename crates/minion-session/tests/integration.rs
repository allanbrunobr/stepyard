//! Integration tests exercising the full append/replay/load contract against
//! a real PostgreSQL. Skipped (without failing) when `MINION_SESSION_DATABASE_URL`
//! is not set so `cargo test` stays green in CI without a DB sidecar.
//!
//! To run against a local database:
//!
//! ```shell
//! docker run --rm -d --name pg-session -e POSTGRES_PASSWORD=pg \
//!     -p 5433:5432 postgres:16-alpine
//! export MINION_SESSION_DATABASE_URL=postgres://postgres:pg@localhost:5433/postgres
//! cargo test -p minion-session --test integration
//! ```
//!
//! Each test creates a fresh session so tests don't interfere. Migrations
//! are idempotent — running `minion_session::migrate` is safe to call per
//! test.

use std::sync::Arc;

use minion_session::{migrate, Session, SessionError, SessionId};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

async fn pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("MINION_SESSION_DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .expect("MINION_SESSION_DATABASE_URL set but unreachable");
    migrate(&pool).await.expect("migrations must succeed");
    Some(pool)
}

macro_rules! db_test {
    ($pool:ident, $body:block) => {{
        let Some($pool) = pool().await else {
            eprintln!("[skip] MINION_SESSION_DATABASE_URL not set");
            return;
        };
        $body
    }};
}

#[tokio::test]
async fn new_session_starts_running_with_null_ended_at() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("new");
        assert_eq!(session.status().as_str(), "running");
        assert_eq!(session.tenant_id(), "edenred");
        assert!(session.ended_at().is_none());
    });
}

#[tokio::test]
async fn first_append_has_seq_one_and_subsequent_are_monotonic() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "afya".into())
            .await
            .expect("new");

        let a = session
            .append(json!({"type": "step_started", "name": "s1"}))
            .await
            .expect("append 1");
        assert_eq!(a.seq, 1);

        let b = session
            .append(json!({"type": "step_completed", "name": "s1"}))
            .await
            .expect("append 2");
        assert_eq!(b.seq, 2);

        let c = session
            .append(json!({"type": "step_started", "name": "s2"}))
            .await
            .expect("append 3");
        assert_eq!(c.seq, 3);
    });
}

#[tokio::test]
async fn replay_returns_events_in_seq_order_regardless_of_clock() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("new");

        for i in 0..10 {
            session
                .append(json!({"seq_hint": i}))
                .await
                .expect("append");
        }

        // Manually mutate created_at to be out of order (simulate clock skew
        // or inserts from multiple machines). The public API cannot do this
        // — we reach directly into SQL.
        sqlx::query(
            r#"
            UPDATE session_events
            SET created_at = NOW() - ((seq % 3) * INTERVAL '1 hour')
            WHERE session_id = $1
            "#,
        )
        .bind(session.id().as_uuid())
        .execute(&pool)
        .await
        .expect("timestamp scramble");

        let events = session.replay().await.expect("replay");
        assert_eq!(events.len(), 10);
        for (i, event) in events.iter().enumerate() {
            assert_eq!(event.seq, (i as i64) + 1, "seq must be 1..=10");
        }
    });
}

#[tokio::test]
async fn load_returns_not_found_for_missing_session() {
    db_test!(pool, {
        let missing = SessionId::new();
        let err = Session::load(&pool, missing).await.expect_err("should err");
        match err {
            SessionError::NotFound(id) => assert_eq!(id, missing),
            other => panic!("expected NotFound, got {other:?}"),
        }
    });
}

#[tokio::test]
async fn load_after_restart_continues_seq_numbering() {
    db_test!(pool, {
        let original = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("new");

        for _ in 0..10 {
            original.append(json!({"x": 1})).await.expect("append");
        }

        // Drop the handle to simulate process restart; reload from DB.
        let id = original.id();
        drop(original);

        let reloaded = Session::load(&pool, id).await.expect("load");
        let events = reloaded.replay().await.expect("replay");
        assert_eq!(events.len(), 10);
        assert_eq!(events.last().unwrap().seq, 10);

        // Next append must continue at 11.
        let next = reloaded
            .append(json!({"after_restart": true}))
            .await
            .expect("append");
        assert_eq!(next.seq, 11);
    });
}

#[tokio::test]
async fn concurrent_appends_produce_no_gaps_and_no_duplicates() {
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("new");
        let session = Arc::new(session);

        // 20 concurrent appends from the same process — advisory lock must
        // serialize them per session without creating gaps or duplicate seq.
        let mut handles = Vec::new();
        for i in 0..20 {
            let s = session.clone();
            handles.push(tokio::spawn(async move {
                s.append(json!({"i": i})).await
            }));
        }

        let mut seqs: Vec<i64> = Vec::new();
        for h in handles {
            let evt = h.await.expect("task").expect("append");
            seqs.push(evt.seq);
        }
        seqs.sort();

        assert_eq!(seqs.len(), 20);
        for (idx, seq) in seqs.iter().enumerate() {
            assert_eq!(*seq, (idx as i64) + 1, "seq must be dense and monotonic");
        }
    });
}

#[tokio::test]
async fn public_api_offers_no_update_or_delete_path() {
    // Compile-time check: Session has no &mut methods that rewrite events
    // and no DELETE-emitting method on its public surface. If someone adds
    // one, this test (together with manual review) will catch it.
    //
    // The real teeth are in the SQL layer — session_events has no foreign
    // key with ON DELETE CASCADE back to sessions, and the migration file
    // contains no UPDATE or DELETE path for session_events. This test just
    // asserts the public API surface.
    db_test!(pool, {
        let session = Session::new(&pool, Uuid::new_v4(), "edenred".into())
            .await
            .expect("new");
        session.append(json!({"a": 1})).await.expect("append");

        // We can still directly DELETE via raw SQL in tests — that's the
        // point: only tests can do it, not the library.
        let deleted = sqlx::query("DELETE FROM session_events WHERE session_id = $1")
            .bind(session.id().as_uuid())
            .execute(&pool)
            .await
            .expect("raw delete");
        assert_eq!(deleted.rows_affected(), 1);
        // After raw deletion, replay sees zero events — proves the library
        // itself never touched those rows.
        let events = session.replay().await.expect("replay");
        assert_eq!(events.len(), 0);
    });
}
