-- Migration 005: Store per-run artifact metadata for remote dashboard downloads.

CREATE TABLE IF NOT EXISTS workflow_artifacts (
  artifact_id UUID PRIMARY KEY,
  run_id UUID NOT NULL REFERENCES workflow_runs(run_id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  content_type TEXT,
  size_bytes INTEGER NOT NULL,
  storage_path TEXT NOT NULL,
  created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_workflow_artifacts_run_id ON workflow_artifacts(run_id);
