# PROMPT.md -- Worktree 3 (wt3)

## Branch: `minion-engine-bmad-wt3`

## Project: Minion Dashboard

A full-stack TypeScript dashboard (Node.js/Express API + React/Vite SPA + PostgreSQL) that visualizes workflow execution data from the Minion Engine. Internal tool for EdenRed deployed via Docker Compose on a KingHost VPS.

---

## Epic Assignment

**Epic 3: Workflow Exploration and Audit Trail**
Features 9, 10, 11, 12 (Stories 3.1 through 3.4)

**Goal:** Enable tech leads and compliance officers to search, inspect, and export detailed workflow execution data for diagnosis and compliance reporting.

---

## Stories

### Story 3.1: Build Workflow Log API Endpoints

**File:** `packages/api/src/routes/workflows.ts`

**Acceptance Criteria:**

- `GET /api/workflows?page=1&limit=25&sort=started_at&order=desc` returns `{ data, total, page, limit, totalPages }`
- Each item in `data` includes: `run_id`, `started_at`, `user_name`, `workflow`, `target`, `repo`, `status`, `duration_ms`, `total_tokens`, `cost_usd`
- Filter parameters supported: `user_name`, `workflow`, `status`, `from`, `to` (AND logic)
- Pagination works correctly with filters applied
- `GET /api/workflows/:run_id` returns full `workflow_run` record plus all associated `workflow_steps` ordered by execution sequence
- All responses use the standard API envelope: `{ data, meta? }` for success, `{ error: { code, message, details? } }` for errors
- Response time < 500ms p95 for paginated queries

---

### Story 3.2: Build Workflow Log Page with Filterable Table

**Directory:** `packages/web/src/pages/Workflows/`

**Acceptance Criteria:**

- Table columns: Timestamp, Developer, Workflow, Target, Repository, Status, Duration, Tokens, Cost
- Default sort: Timestamp descending
- Clickable column headers toggle sort direction
- Status badges: green (success), red (failed), yellow (running)
- Filter dropdowns (Developer, Workflow Type, Status) populated from actual DB data (distinct values)
- Date range picker for `from`/`to` filtering
- 25 rows per page with pagination controls at bottom
- Row click navigates to Workflow Detail View (`/workflows/:runId`)
- All filter state stored in URL query parameters for shareable/bookmarkable views

---

### Story 3.3: Build Workflow Detail View

**Directory:** `packages/web/src/pages/WorkflowDetail/`

**Acceptance Criteria:**

- Run header displays: Run ID, Developer, Workflow Type, Target, Repository, Status badge, Total Duration, Total Tokens, Total Cost, Start Time, End Time
- Failed runs: error message displayed prominently in header area
- Vertical step timeline rendered in execution order
- Per step: name, type (cmd/chat/agent/gate/map), status badge, duration, tokens in, tokens out, sandbox indicator
- Failed steps: red highlight with error message inline
- Back button returns to Workflow Log page preserving previous filters

---

### Story 3.4: Implement CSV Data Export

**Trigger:** "Export CSV" button on Workflow Log page

**Acceptance Criteria:**

- Exports the currently filtered dataset (respects all active filters)
- CSV columns: Timestamp, Developer, Workflow Type, Target, Repository, Status, Duration (ms), Tokens, Cost (USD), Error, Sandbox Confirmed
- UTF-8 encoded CSV with header row
- Filename includes date range: `minion-dashboard-export-YYYY-MM-DD-to-YYYY-MM-DD.csv`
- Completes in < 10 seconds for up to 10,000 rows

---

## Merge Order

**wt3 merges AFTER wt1** (wt1 establishes project structure, Docker setup, database schema, event ingestion).

wt3 can run in parallel with wt2 (Overview) and wt4 (Analytics) since they operate on separate files and routes.
