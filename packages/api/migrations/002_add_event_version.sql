-- Migration 002: Add event_version for monotonic stale-event guard

ALTER TABLE workflow_runs ADD COLUMN IF NOT EXISTS event_version INTEGER NOT NULL DEFAULT 1;
