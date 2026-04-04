-- Migration 001: Create workflow_runs and workflow_steps tables

CREATE TABLE IF NOT EXISTS workflow_runs (
  run_id UUID PRIMARY KEY,
  user_name TEXT,
  workflow TEXT,
  target TEXT,
  repo TEXT,
  status TEXT,
  duration_ms INTEGER,
  total_tokens INTEGER,
  cost_usd NUMERIC,
  started_at TIMESTAMPTZ,
  finished_at TIMESTAMPTZ,
  error TEXT,
  created_at TIMESTAMPTZ DEFAULT NOW(),
  updated_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS workflow_steps (
  id SERIAL PRIMARY KEY,
  run_id UUID REFERENCES workflow_runs(run_id) ON DELETE CASCADE,
  step_name TEXT,
  step_type TEXT,
  status TEXT,
  duration_ms INTEGER,
  tokens_in INTEGER,
  tokens_out INTEGER,
  sandboxed BOOLEAN
);

CREATE INDEX IF NOT EXISTS idx_workflow_runs_started_at ON workflow_runs(started_at);
CREATE INDEX IF NOT EXISTS idx_workflow_runs_user_name ON workflow_runs(user_name);
CREATE INDEX IF NOT EXISTS idx_workflow_runs_status ON workflow_runs(status);
CREATE INDEX IF NOT EXISTS idx_workflow_steps_run_id ON workflow_steps(run_id);
