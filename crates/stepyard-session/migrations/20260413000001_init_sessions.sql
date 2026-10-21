-- Minion Session storage — append-only event log.
-- Invariants (see ARCHITECTURE.md § Invariants #2):
--   * session_events rows are NEVER updated or deleted after insert.
--   * (session_id, seq) is UNIQUE; seq is monotonic per session starting at 1.
--   * No ON DELETE CASCADE — even if a session is removed, its events survive
--     for audit (NFC2 from epics.md Epic 1).

-- Required for gen_random_uuid(); available by default in PG 13+.
CREATE EXTENSION IF NOT EXISTS "pgcrypto";

CREATE TABLE IF NOT EXISTS sessions (
    id          UUID        PRIMARY KEY,
    workflow_id UUID        NOT NULL,
    tenant_id   TEXT        NOT NULL,
    status      TEXT        NOT NULL DEFAULT 'running'
                            CHECK (status IN ('running', 'completed', 'failed', 'cancelled')),
    started_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    ended_at    TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_sessions_workflow_id ON sessions(workflow_id);
CREATE INDEX IF NOT EXISTS idx_sessions_tenant_id   ON sessions(tenant_id);
CREATE INDEX IF NOT EXISTS idx_sessions_status      ON sessions(status) WHERE status = 'running';

CREATE TABLE IF NOT EXISTS session_events (
    id         UUID        PRIMARY KEY,
    session_id UUID        NOT NULL REFERENCES sessions(id),
    seq        BIGINT      NOT NULL CHECK (seq >= 1),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    payload    JSONB       NOT NULL
);

-- Monotonic seq per session (enforces NFC2 no-gaps expectation at DB level).
CREATE UNIQUE INDEX IF NOT EXISTS idx_session_events_seq
    ON session_events(session_id, seq);

-- Replay is always by session_id ordered by seq; seq covers that already,
-- but this helps range scans over recent events.
CREATE INDEX IF NOT EXISTS idx_session_events_created
    ON session_events(session_id, created_at DESC);
