-- Migration 003: Add error column to workflow_steps for per-step failure messages
ALTER TABLE workflow_steps ADD COLUMN IF NOT EXISTS error TEXT;
