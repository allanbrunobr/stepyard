CREATE TABLE IF NOT EXISTS sessions (
    id          TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL,
    tenant_id   TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'running'
                CHECK (status IN ('running', 'completed', 'failed', 'cancelled')),
    started_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    ended_at    TEXT
);

CREATE INDEX IF NOT EXISTS idx_sessions_workflow_id ON sessions(workflow_id);
CREATE INDEX IF NOT EXISTS idx_sessions_tenant_id   ON sessions(tenant_id);
CREATE INDEX IF NOT EXISTS idx_sessions_status      ON sessions(status) WHERE status = 'running';

CREATE TABLE IF NOT EXISTS session_events (
    id         TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    seq        INTEGER NOT NULL CHECK (seq >= 1),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    payload    TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_session_events_seq
    ON session_events(session_id, seq);

CREATE INDEX IF NOT EXISTS idx_session_events_created
    ON session_events(session_id, created_at DESC);
