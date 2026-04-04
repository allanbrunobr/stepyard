# PROMPT.md — Worktree 2 (wt2)

> **Branch:** `minion-engine-bmad-wt2`
> **Project:** Minion Dashboard
> **Epic:** Epic 2 — Dashboard Overview and Monitoring
> **Assigned Features:** 5, 6, 7, 8 (Stories 2.1, 2.2, 2.3, 2.4)

---

## Mission

Build the Dashboard Overview page and its supporting API endpoints. When this worktree merges, users can open the dashboard and immediately see summary metrics (total workflows, tokens, cost, active developers), a daily usage chart, peak hours visualization, date range filtering, and 30-second auto-refresh.

---

## Stories

### Story 2.1: Build API Aggregation Endpoints for Overview Data

**Acceptance Criteria:**

- `GET /api/overview/summary?from=&to=` returns `{ total_runs, total_tokens, total_cost_usd, active_developers }`
- `GET /api/overview/daily-usage?from=&to=` returns array of `{ date, count }` per day (including zero-count days)
- `GET /api/overview/peak-hours?from=&to=` returns 24 objects `{ hour: 0-23, count }`
- Default range: last 30 days when `from`/`to` are omitted
- Response < 500ms for 10k runs

### Story 2.2: Build Dashboard Overview Page with Summary Cards

**Acceptance Criteria:**

- Four summary cards: Total Workflows, Total Tokens, Estimated Cost ($), Active Developers
- Page loads < 2 seconds
- Shadcn/ui Card components + Tailwind CSS
- Navigation sidebar with links: Overview (active), Workflow Log, Developer Activity, Cost Tracking
- Empty state: cards show `0` / `$0.00` with no errors

### Story 2.3: Add Usage Graph and Peak Hours Visualization

**Acceptance Criteria:**

- Recharts line/area chart: workflow count per day over selected period
- Labeled axes (date on X, count on Y), tooltips on hover
- Peak hours visualization: color intensity or bar height per hour (0-23)

### Story 2.4: Add Date Range Filter and Auto-Refresh

**Acceptance Criteria:**

- Date selector with predefined options: Today, 7d, 30d (default), 90d, Custom
- Selecting any option updates all cards, charts, and peak hours
- Auto-refresh every 30s when browser tab is active
- Polling pauses when tab is inactive, resumes when active

---

## Territory Rules

### Owned Directories (full read/write)

These are the files and directories this worktree owns. Create, modify, and delete freely:

- `packages/api/src/routes/overview.ts` — Overview API route handlers
- `packages/web/src/pages/Overview/` — Overview page and subcomponents
- `packages/web/src/components/charts/` — Recharts chart components (UsageChart, PeakHours)
- `packages/web/src/components/ui/` — Shadcn/ui components (Card, Button, DatePicker, etc.)
- `packages/web/src/hooks/` — Custom React hooks (useOverviewData, useAutoRefresh, useDateRange)
- `packages/web/src/lib/api-client.ts` — API client/fetch wrapper
- `packages/web/src/components/layout/` — Sidebar, Layout, Navigation components

### Read-Only Files (consume, never modify)

These are created by wt1. Read and import from them but never edit:

- `packages/api/src/db/` — Database connection pool, query helpers
- `packages/api/src/index.ts` — Express app bootstrap and server setup
- `types/` — Shared TypeScript type definitions

### Forbidden Directories (do not touch)

These belong to other worktrees. Do not read, write, or reference:

- `packages/api/migrations/` — Database migrations (wt1)
- `packages/api/src/routes/events.ts` — Event ingestion endpoint (wt1)
- `packages/web/src/pages/Workflows/` — Workflow Log and Detail (wt3)
- `packages/web/src/pages/Developers/` — Developer Activity (wt4)
- `packages/web/src/pages/Costs/` — Cost Tracking (wt4)

### Shared Files (coordination required)

- **`packages/api/src/routes/index.ts`** — `append_only`. Register overview routes by adding an import and `app.use('/api/overview', overviewRouter)`. Do not modify existing lines.
- **`packages/web/src/App.tsx`** — `single_owner` by wt2. This worktree owns the routing setup. Define all routes (Overview, Workflow Log, Developers, Costs) with placeholder components for pages owned by other worktrees.

---

## Merge Order

**wt2 merges AFTER wt1.**

This means wt1's code (database layer, Express app, types, event ingestion) will already be on `main` when wt2 merges. Build against those interfaces.

---

## Dependencies — What wt2 Consumes from wt1

| Artifact | Expected Location | What It Provides |
|----------|-------------------|------------------|
| PostgreSQL connection pool | `packages/api/src/db/pool.ts` | `pool` instance for running SQL queries |
| Database tables | `workflow_runs`, `workflow_steps` | Tables with indexes on `started_at`, `user_name`, `status`, `run_id` |
| Express app setup | `packages/api/src/index.ts` | Configured Express app with JSON parsing, CORS, error handling |
| Health endpoint | `GET /api/health` | Confirms API is running |
| Shared types | `types/` | `WorkflowRun`, `WorkflowStep`, and related TypeScript interfaces |

---

## Tech Stack

| Layer | Technology |
|-------|------------|
| API | Node.js + Express + TypeScript |
| Database | PostgreSQL 16 (raw SQL, parameterized queries, no ORM) |
| Frontend | React + Vite + TypeScript |
| UI Components | Shadcn/ui + Tailwind CSS |
| Charts | Recharts |
| Deployment | Docker + docker-compose |

---

## Implementation Guidance

### API Layer (`packages/api/src/routes/overview.ts`)

1. Import the pool from `../db/pool` (provided by wt1).
2. Implement three route handlers:
   - `GET /summary` — Single SQL query with `COUNT(*)`, `SUM(total_tokens)`, `SUM(cost_usd)`, `COUNT(DISTINCT user_name)` filtered by `started_at BETWEEN $1 AND $2`.
   - `GET /daily-usage` — SQL grouping by `DATE(started_at)`. Fill zero-count days in application code using a date range loop.
   - `GET /peak-hours` — SQL `EXTRACT(HOUR FROM started_at)` with `GROUP BY`. Ensure all 24 hours are represented.
3. Parse `from`/`to` query params; default to `NOW() - INTERVAL '30 days'` and `NOW()`.
4. Register routes in `packages/api/src/routes/index.ts` (append only).

### Frontend Layer

1. **API Client** (`packages/web/src/lib/api-client.ts`): Thin fetch wrapper with base URL from env, JSON parsing, error handling.
2. **Hooks**:
   - `useOverviewData(from, to)` — Fetches summary, daily-usage, and peak-hours in parallel.
   - `useDateRange()` — Manages selected range state, computes `from`/`to` dates.
   - `useAutoRefresh(callback, intervalMs)` — Calls callback on interval, pauses on `document.hidden`.
3. **Layout** (`packages/web/src/components/layout/`): Sidebar with nav links, main content area.
4. **Overview Page** (`packages/web/src/pages/Overview/`):
   - Summary cards using Shadcn/ui `Card`.
   - Usage chart using Recharts `AreaChart` or `LineChart`.
   - Peak hours using Recharts `BarChart` or custom heatmap.
   - Date range selector toolbar.
5. **App Routing** (`packages/web/src/App.tsx`): React Router with routes for `/` (Overview), `/workflows`, `/developers`, `/costs`. Non-owned pages render a placeholder `<div>Coming Soon</div>`.

### Key Constraints

- All timestamps in UTC (`TIMESTAMPTZ`). Frontend formats to user locale.
- No ORM. Use parameterized SQL queries only (`$1`, `$2` placeholders).
- No authentication on read endpoints (MVP).
- Response format must be JSON. Use consistent error format: `{ "error": "<code>", "message": "<detail>" }`.
- Numbers: format tokens with locale separators, cost with 2 decimal places and `$` prefix.

---

## Definition of Done

A story is complete when:

1. Code compiles with zero TypeScript errors (`tsc --noEmit`)
2. All acceptance criteria are met and manually verifiable
3. API endpoints return correct data shapes and handle edge cases (empty data, missing params)
4. UI renders correctly with data and in empty state
5. No console errors in browser dev tools
6. Code follows existing project conventions (file naming, export patterns)
