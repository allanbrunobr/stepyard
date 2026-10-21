-- Migration 004: Link dashboard runs to the dispatch process log.

ALTER TABLE workflow_runs ADD COLUMN IF NOT EXISTS dispatch_log_path TEXT;
