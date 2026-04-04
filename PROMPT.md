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

**Implementation Notes:**

- Use `pg` Pool with parameterized SQL queries (no ORM -- see ADR-001)
- Query parameters validated with Zod schemas (ADR-007)
- Filter parameters build dynamic WHERE clauses with AND logic
- Pagination uses `LIMIT/OFFSET` with a COUNT query for total
- Sort columns restricted to allowlist to prevent SQL injection
- Steps ordered by `id` (serial PK, reflects insertion/execution order)

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
- All filter state stored in URL query parameters (ADR-013) for shareable/bookmarkable views

**Implementation Notes:**

- Use TanStack Query hooks for data fetching with `useSearchParams` for filter state
- Shadcn/ui Table, Select, Badge, Button components
- Create `WorkflowTable.tsx` and `WorkflowFilters.tsx` in `packages/web/src/components/workflow/`
- StatusBadge component from shared components (`packages/web/src/components/shared/`)
- Distinct filter values fetched via a separate lightweight API call or derived from workflow data
- Pagination component with page numbers, prev/next controls

---

### Story 3.3: Build Workflow Detail View

**Directory:** `packages/web/src/pages/WorkflowDetail/`

**Acceptance Criteria:**

- Run header displays: Run ID, Developer, Workflow Type, Target, Repository, Status badge, Total Duration, Total Tokens, Total Cost, Start Time, End Time
- Failed runs: error message displayed prominently in header area
- Vertical step timeline rendered in execution order
- Per step: name, type (cmd/chat/agent/gate/map), status badge, duration, tokens in, tokens out, sandbox indicator (icon or label)
- Failed steps: red highlight (border or background) with error message inline
- Back button returns to Workflow Log page preserving previous filters (via URL query params)
- Page loads data from `GET /api/workflows/:run_id`

**Implementation Notes:**

- Create `StepTimeline.tsx` in `packages/web/src/components/workflow/`
- Use Shadcn/ui Card, Badge components for run header and step cards
- Step types displayed as secondary badges or labels
- Sandbox indicator as a small icon or chip when `sandboxed === true`
- Back navigation uses `useNavigate(-1)` or a link with preserved search params from referrer
- TanStack Query hook for fetching single workflow run with steps

---

### Story 3.4: Implement CSV Data Export

**Trigger:** "Export CSV" button on Workflow Log page

**Acceptance Criteria:**

- Exports the currently filtered dataset (respects all active filters)
- CSV columns: Timestamp, Developer, Workflow Type, Target, Repository, Status, Duration (ms), Tokens, Cost (USD), Error, Sandbox Confirmed
- UTF-8 encoded CSV with header row
- Filename includes date range: `minion-dashboard-export-YYYY-MM-DD-to-YYYY-MM-DD.csv`
- Completes in < 10 seconds for up to 10,000 rows
- No source code, diffs, or stdout content in export -- metadata only

**Implementation Notes:**

- API endpoint: `GET /api/workflows/export?user_name=...&status=...&from=...&to=...` returns CSV stream
- Set `Content-Type: text/csv` and `Content-Disposition: attachment; filename=...` headers
- Use streaming response to handle large datasets efficiently
- Frontend triggers download via `window.location` or anchor element with constructed URL
- Route must be registered BEFORE the `:runId` param route to avoid conflicts

---

## Territory Rules

### Owned (full read/write)

- `packages/api/src/routes/workflows.ts` -- All workflow API endpoints (list, detail, CSV export)
- `packages/web/src/pages/Workflows/` -- Workflow Log page and related components
- `packages/web/src/pages/WorkflowDetail/` -- Workflow Detail View page
- `packages/web/src/components/workflow/` -- Workflow-specific components (WorkflowTable, WorkflowFilters, StepTimeline)

### Read-Only (consume, never modify)

- `packages/api/src/db/` -- Database pool, migrations, connection setup
- `types/` (or `packages/shared/`) -- Shared TypeScript type definitions
- `packages/web/src/lib/api-client.ts` -- Fetch wrapper with base URL and error handling
- `packages/web/src/components/ui/` -- Shadcn/ui base components
- `packages/web/src/components/layout/` -- Sidebar, PageLayout

### Forbidden (never touch)

- `packages/api/src/routes/overview.ts` -- Owned by wt2
- `packages/api/src/routes/analytics.ts` -- Owned by wt4
- `packages/web/src/pages/Overview/` -- Owned by wt2
- `packages/web/src/pages/Developers/` -- Owned by wt4
- `packages/web/src/pages/Costs/` -- Owned by wt4

### Shared (append-only)

- `packages/api/src/routes/index.ts` -- Register workflow routes by appending import + `app.use()` call. Do not modify existing route registrations.
- `packages/web/src/App.tsx` -- Add `<Route path="/workflows" ...>` and `<Route path="/workflows/:runId" ...>` entries. Do not modify existing routes.

---

## Merge Order

**wt3 merges AFTER wt1** (wt1 establishes project structure, Docker setup, database schema, event ingestion).

wt3 can run in parallel with wt2 (Overview) and wt4 (Analytics) since they operate on separate files and routes.

**Dependencies from wt1:**
- Database schema (`workflow_runs`, `workflow_steps` tables) must exist
- Express app bootstrap and middleware must be in place
- `pg` Pool singleton must be available at `packages/api/src/db/pool.ts`
- Shared types (`WorkflowRun`, `WorkflowStep`, `ApiResponse<T>`, `PaginationMeta`) must be defined
- React Router, TanStack Query, Shadcn/ui must be installed and configured
- API client (`packages/web/src/lib/api-client.ts`) must be available
- Layout components (Sidebar with nav links, PageLayout) must exist

---

## Technical Reference

### Database Schema (read-only context)

```sql
-- workflow_runs
run_id        UUID PRIMARY KEY
user_name     VARCHAR NOT NULL
workflow      VARCHAR NOT NULL
target        VARCHAR
repo          VARCHAR
status        VARCHAR NOT NULL        -- 'success' | 'failed' | 'running'
duration_ms   INTEGER
total_tokens  INTEGER
cost_usd      NUMERIC(10,4)
started_at    TIMESTAMPTZ NOT NULL
finished_at   TIMESTAMPTZ
error         TEXT
created_at    TIMESTAMPTZ DEFAULT NOW()
updated_at    TIMESTAMPTZ DEFAULT NOW()

-- workflow_steps
id            SERIAL PRIMARY KEY
run_id        UUID FK -> workflow_runs ON DELETE CASCADE
step_name     VARCHAR NOT NULL
step_type     VARCHAR NOT NULL        -- 'cmd' | 'chat' | 'agent' | 'gate' | 'map'
status        VARCHAR NOT NULL
duration_ms   INTEGER
tokens_in     INTEGER
tokens_out    INTEGER
sandboxed     BOOLEAN DEFAULT FALSE
```

### Key Indexes

```sql
idx_workflow_runs_started_at          (started_at DESC)
idx_workflow_runs_user_name           (user_name)
idx_workflow_runs_status              (status)
idx_workflow_runs_workflow            (workflow)
idx_workflow_runs_repo                (repo)
idx_workflow_runs_started_at_status   (started_at DESC, status)
idx_workflow_steps_run_id             (run_id)
```

### API Response Envelope

```typescript
// Success
{ data: T, meta?: { total: number, page: number, limit: number, totalPages: number } }

// Error
{ error: { code: string, message: string, details?: any } }
```

### Frontend Routes

```
/workflows           -> WorkflowLogPage
/workflows/:runId    -> WorkflowDetailPage
```

### Key Libraries

| Layer    | Library                | Purpose                              |
|----------|------------------------|--------------------------------------|
| API      | pg (node-postgres)     | Database queries, connection pooling |
| API      | zod                    | Request validation                   |
| API      | pino                   | Structured JSON logging              |
| Frontend | @tanstack/react-query  | Server state, caching, refetch       |
| Frontend | react-router-dom v6    | Client-side routing, useSearchParams |
| Frontend | shadcn/ui              | UI component primitives              |
| Frontend | tailwindcss            | Utility-first CSS                    |
| Frontend | recharts               | Charts (if needed for cost column)   |

### Naming Conventions

- API files: `kebab-case.ts` (e.g., `workflow-service.ts`)
- React components: `PascalCase.tsx` (e.g., `WorkflowTable.tsx`)
- Hooks: `use-kebab-case.ts` exporting `useCamelCase` (e.g., `use-workflows.ts` -> `useWorkflows`)
- DB columns: `snake_case`
- API query params: `snake_case` (`user_name`, `started_at`)
- Route params: `:camelCase` (`:runId`)

---

## Implementation Order

1. **Story 3.1** -- Build workflow API endpoints (list, detail, export endpoint skeleton)
2. **Story 3.2** -- Build Workflow Log page with table, filters, pagination, sorting
3. **Story 3.3** -- Build Workflow Detail View with step timeline
4. **Story 3.4** -- Implement CSV export (wire up export button, streaming CSV response)

Each story should be committed separately. Run `npx tsc --noEmit` and verify no type errors before committing.
