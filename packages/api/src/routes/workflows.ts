import { Router, Request, Response } from 'express';
import { z } from 'zod';
import { pool } from '../db';
import { logger } from '../logger';

export const workflowsRouter = Router();

function requireAuth(req: Request, res: Response, next: Function) {
  const authHeader = req.headers.authorization;
  const secret = process.env.API_SECRET;
  if (!secret || !authHeader || !authHeader.startsWith('Bearer ') || authHeader.slice(7) !== secret) {
    res.status(401).json({ error: { code: 'UNAUTHORIZED', message: 'Invalid or missing authorization' } });
    return;
  }
  next();
}

// Apply to all workflow routes
workflowsRouter.use(requireAuth);

// --- Validation Schemas ---

const SORTABLE_COLUMNS = [
  'started_at',
  'user_name',
  'workflow',
  'status',
  'duration_ms',
  'total_tokens',
  'cost_usd',
] as const;

const listQuerySchema = z.object({
  page: z.coerce.number().int().min(1).default(1),
  limit: z.coerce.number().int().min(1).max(100).default(25),
  sort: z.enum(SORTABLE_COLUMNS).default('started_at'),
  order: z.enum(['asc', 'desc']).default('desc'),
  user_name: z.string().optional(),
  workflow: z.string().optional(),
  status: z.string().optional(),
  from: z.string().optional(),
  to: z.string().optional(),
});

// --- Helper: build WHERE clause from filters ---

interface WhereClause {
  text: string;
  values: unknown[];
}

function buildWhereClause(filters: {
  user_name?: string;
  workflow?: string;
  status?: string;
  from?: string;
  to?: string;
}): WhereClause {
  const conditions: string[] = [];
  const values: unknown[] = [];
  let paramIdx = 1;

  if (filters.user_name) {
    conditions.push(`user_name = $${paramIdx++}`);
    values.push(filters.user_name);
  }
  if (filters.workflow) {
    conditions.push(`workflow = $${paramIdx++}`);
    values.push(filters.workflow);
  }
  if (filters.status) {
    conditions.push(`status = $${paramIdx++}`);
    values.push(filters.status);
  }
  if (filters.from) {
    conditions.push(`started_at >= $${paramIdx++}`);
    values.push(filters.from);
  }
  if (filters.to) {
    conditions.push(`started_at < ($${paramIdx++}::date + INTERVAL '1 day')`);
    values.push(filters.to);
  }

  const text = conditions.length > 0 ? `WHERE ${conditions.join(' AND ')}` : '';
  return { text, values };
}

// --- CSV Export (registered BEFORE :run_id to avoid param conflict) ---

workflowsRouter.get('/workflows/export', async (req: Request, res: Response) => {

  const parsed = listQuerySchema.omit({ page: true, limit: true, sort: true, order: true }).safeParse(req.query);
  if (!parsed.success) {
    res.status(400).json({ error: { code: 'VALIDATION_ERROR', message: 'Invalid query parameters', details: parsed.error.issues } });
    return;
  }

  const { user_name, workflow, status, from, to } = parsed.data;
  const where = buildWhereClause({ user_name, workflow, status, from, to });

  const fromDate = from ?? 'all';
  const toDate = to ?? 'all';
  const filename = `minion-dashboard-export-${fromDate}-to-${toDate}.csv`;

  res.setHeader('Content-Type', 'text/csv; charset=utf-8');
  res.setHeader('Content-Disposition', `attachment; filename="${filename}"`);

  const csvHeader =
    'Timestamp,Developer,Workflow Type,Target,Repository,Status,Duration (ms),Tokens,Cost (USD),Error,Sandbox Confirmed\n';
  res.write(csvHeader);

  const client = await pool.connect();
  try {
    const query = `
      SELECT r.started_at, r.user_name, r.workflow, r.target, r.repo, r.status,
             r.duration_ms, r.total_tokens, r.cost_usd, r.error,
             COALESCE(bool_or(s.sandboxed), false) AS sandbox_confirmed
      FROM workflow_runs r
      LEFT JOIN workflow_steps s ON s.run_id = r.run_id
      ${where.text}
      GROUP BY r.run_id
      ORDER BY r.started_at DESC
    `;

    // Use a SQL cursor to stream rows in batches, avoiding loading all rows into memory
    const cursorName = 'csv_export_cursor';
    const batchSize = 500;

    await client.query('BEGIN');
    await client.query(`DECLARE ${cursorName} CURSOR FOR ${query}`, where.values);

    let batch;
    do {
      batch = await client.query(`FETCH ${batchSize} FROM ${cursorName}`);
      for (const row of batch.rows) {
        const fields = [
          csvEscape(String(row.started_at)),
          csvEscape(row.user_name ?? ''),
          csvEscape(row.workflow ?? ''),
          csvEscape(row.target ?? ''),
          csvEscape(row.repo ?? ''),
          csvEscape(row.status ?? ''),
          String(row.duration_ms ?? ''),
          String(row.total_tokens ?? ''),
          String(row.cost_usd ?? ''),
          csvEscape(row.error ?? ''),
          row.sandbox_confirmed ? 'Yes' : 'No',
        ];
        res.write(fields.join(',') + '\n');
      }
    } while (batch.rows.length === batchSize);

    await client.query(`CLOSE ${cursorName}`);
    await client.query('COMMIT');
    res.end();
  } catch (err) {
    await client.query('ROLLBACK').catch(() => {});
    logger.error(err, 'Failed to export CSV');
    if (!res.headersSent) {
      res.status(500).json({ error: { code: 'INTERNAL_ERROR', message: 'Failed to export CSV' } });
    } else {
      res.end();
    }
  } finally {
    client.release();
  }
});

// --- GET /workflows (paginated list) ---

workflowsRouter.get('/workflows', async (req: Request, res: Response) => {

  const parsed = listQuerySchema.safeParse(req.query);
  if (!parsed.success) {
    res.status(400).json({ error: { code: 'VALIDATION_ERROR', message: 'Invalid query parameters', details: parsed.error.issues } });
    return;
  }

  const { page, limit, sort, order, user_name, workflow, status, from, to } = parsed.data;
  const offset = (page - 1) * limit;

  const where = buildWhereClause({ user_name, workflow, status, from, to });

  try {
    const countQuery = `SELECT COUNT(*) FROM workflow_runs ${where.text}`;
    const countResult = await pool.query(countQuery, where.values);
    const total = parseInt(countResult.rows[0].count, 10);
    const totalPages = Math.ceil(total / limit);

    const dataQuery = `
      SELECT run_id, started_at, user_name, workflow, target, repo, status,
             duration_ms, total_tokens, cost_usd
      FROM workflow_runs
      ${where.text}
      ORDER BY ${sort} ${order}
      LIMIT $${where.values.length + 1} OFFSET $${where.values.length + 2}
    `;

    const dataResult = await pool.query(dataQuery, [...where.values, limit, offset]);

    res.json({
      data: dataResult.rows,
      meta: { total, page, limit, totalPages },
    });
  } catch (err) {
    logger.error(err, 'Failed to fetch workflows');
    res.status(500).json({ error: { code: 'INTERNAL_ERROR', message: 'Failed to fetch workflows' } });
  }
});

// --- GET /workflows/distinct (for filter dropdowns) ---

workflowsRouter.get('/workflows/distinct', async (_req: Request, res: Response) => {
  try {
    const [users, workflows, statuses] = await Promise.all([
      pool.query('SELECT DISTINCT user_name FROM workflow_runs ORDER BY user_name'),
      pool.query('SELECT DISTINCT workflow FROM workflow_runs ORDER BY workflow'),
      pool.query('SELECT DISTINCT status FROM workflow_runs ORDER BY status'),
    ]);

    res.json({
      data: {
        user_names: users.rows.map((r: { user_name: string }) => r.user_name),
        workflows: workflows.rows.map((r: { workflow: string }) => r.workflow),
        statuses: statuses.rows.map((r: { status: string }) => r.status),
      },
    });
  } catch (err) {
    logger.error(err, 'Failed to fetch distinct values');
    res.status(500).json({ error: { code: 'INTERNAL_ERROR', message: 'Failed to fetch distinct values' } });
  }
});

// --- GET /workflows/:runId (detail with steps) ---

workflowsRouter.get('/workflows/:runId', async (req: Request, res: Response) => {

  const { runId } = req.params;

  try {
    const runResult = await pool.query('SELECT * FROM workflow_runs WHERE run_id = $1', [runId]);

    if (runResult.rows.length === 0) {
      res.status(404).json({ error: { code: 'NOT_FOUND', message: `Workflow run ${runId} not found` } });
      return;
    }

    const stepsResult = await pool.query(
      'SELECT * FROM workflow_steps WHERE run_id = $1 ORDER BY id',
      [runId],
    );

    res.json({
      data: {
        ...runResult.rows[0],
        steps: stepsResult.rows,
      },
    });
  } catch (err) {
    logger.error(err, 'Failed to fetch workflow detail');
    res.status(500).json({ error: { code: 'INTERNAL_ERROR', message: 'Failed to fetch workflow detail' } });
  }
});

// --- Helpers ---

function csvEscape(value: string): string {
  // Neutralize spreadsheet formula injection
  if (/^[=+\-@\t\r]/.test(value)) {
    value = "'" + value;
  }
  if (value.includes(',') || value.includes('"') || value.includes('\n')) {
    return `"${value.replace(/"/g, '""')}"`;
  }
  return value;
}
