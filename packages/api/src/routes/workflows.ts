import { Router, Request, Response } from 'express';
import { z } from 'zod';
import { spawn } from 'child_process';
import { randomUUID } from 'crypto';
import { closeSync, existsSync, mkdirSync, openSync, readSync, statSync } from 'fs';
import { basename, join } from 'path';
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

function envFlag(name: string): boolean {
  const value = process.env[name]?.trim().toLowerCase();
  return value === '1' || value === 'true' || value === 'yes';
}

// TODO (post-MVP): Add auth middleware for read endpoints
// For PoC, access is controlled at network/firewall level on the VPS
// workflowsRouter.use(requireAuth);

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
  const filename = `stepyard-dashboard-export-${fromDate}-to-${toDate}.csv`;

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

// --- GET /workflows/:runId/logs/stream (SSE snapshots for remote CLI) ---

const TERMINAL_STATUSES = new Set(['completed', 'success', 'failed', 'cancelled', 'canceled']);
const MAX_LOG_CHUNK_BYTES = 8192;

interface WorkflowSnapshot {
  run: Record<string, unknown>;
  steps: Record<string, unknown>[];
}

async function loadWorkflowSnapshot(runId: string): Promise<WorkflowSnapshot | null> {
  const runResult = await pool.query('SELECT * FROM workflow_runs WHERE run_id = $1', [runId]);
  if (runResult.rows.length === 0) {
    return null;
  }

  const stepsResult = await pool.query(
    'SELECT * FROM workflow_steps WHERE run_id = $1 ORDER BY id',
    [runId],
  );

  return {
    run: runResult.rows[0],
    steps: stepsResult.rows,
  };
}

function writeSse(res: Response, event: string, data: unknown): void {
  res.write(`event: ${event}\n`);
  res.write(`data: ${JSON.stringify(data)}\n\n`);
}

function dispatchLogPath(snapshot: WorkflowSnapshot): string | null {
  const path = snapshot.run.dispatch_log_path;
  return typeof path === 'string' && path.length > 0 ? path : null;
}

function readDispatchLogChunk(path: string, offset: number): { text: string; nextOffset: number; hasMore: boolean } | null {
  try {
    const stat = statSync(path);
    if (!stat.isFile() || stat.size <= offset) {
      return null;
    }

    const bytesToRead = Math.min(stat.size - offset, MAX_LOG_CHUNK_BYTES);
    const fd = openSync(path, 'r');
    try {
      const buffer = Buffer.alloc(bytesToRead);
      const bytesRead = readSync(fd, buffer, 0, bytesToRead, offset);
      return {
        text: sanitizeLogText(buffer.subarray(0, bytesRead).toString('utf8')),
        nextOffset: offset + bytesRead,
        hasMore: stat.size > offset + bytesRead,
      };
    } finally {
      closeSync(fd);
    }
  } catch (err) {
    logger.warn({ err, path }, 'Unable to read dispatch log chunk');
    return null;
  }
}

function sanitizeLogText(input: string): string {
  let output = '';
  for (const char of input) {
    const cp = char.codePointAt(0);
    if (cp == null) continue;
    const allowedControl = char === '\n' || char === '\r' || char === '\t';
    const bidiOverride = (cp >= 0x202a && cp <= 0x202e) || (cp >= 0x2066 && cp <= 0x2069);
    if ((!allowedControl && cp <= 0x1f) || cp === 0x7f || (cp >= 0x80 && cp <= 0x9f) || bidiOverride) {
      output += `\\u{${cp.toString(16).padStart(4, '0')}}`;
    } else {
      output += char;
    }
  }
  return output;
}

workflowsRouter.get('/workflows/:runId/logs/stream', requireAuth, async (req: Request, res: Response) => {
  const runId = String(req.params.runId);

  try {
    const initial = await loadWorkflowSnapshot(runId);
    if (!initial) {
      res.status(404).json({ error: { code: 'NOT_FOUND', message: `Workflow run ${runId} not found` } });
      return;
    }

    res.setHeader('Content-Type', 'text/event-stream; charset=utf-8');
    res.setHeader('Cache-Control', 'no-cache, no-transform');
    res.setHeader('Connection', 'keep-alive');
    res.flushHeaders?.();

    let seq = 0;
    let logSeq = 0;
    let logOffset = 0;
    let lastPayload = '';
    const emitSnapshot = async (): Promise<boolean> => {
      const snapshot = await loadWorkflowSnapshot(runId);
      if (!snapshot) {
        writeSse(res, 'error', { code: 'NOT_FOUND', message: `Workflow run ${runId} not found` });
        return true;
      }

      const logPath = dispatchLogPath(snapshot);
      let logHasMore = false;
      if (logPath) {
        const chunk = readDispatchLogChunk(logPath, logOffset);
        if (chunk && chunk.text.length > 0) {
          logOffset = chunk.nextOffset;
          logHasMore = chunk.hasMore;
          writeSse(res, 'log', { seq: ++logSeq, text: chunk.text });
        }
      }

      const payload = JSON.stringify(snapshot);
      if (payload !== lastPayload) {
        lastPayload = payload;
        writeSse(res, 'snapshot', { seq: ++seq, ...snapshot });
      } else {
        writeSse(res, 'ping', { seq });
      }

      const status = String(snapshot.run.status ?? '').toLowerCase();
      if (TERMINAL_STATUSES.has(status) && !logHasMore) {
        writeSse(res, 'done', { seq, status });
        return true;
      }

      return false;
    };

    if (await emitSnapshot()) {
      res.end();
      return;
    }

    const interval = setInterval(() => {
      emitSnapshot()
        .then((done) => {
          if (done) {
            clearInterval(interval);
            res.end();
          }
        })
        .catch((err) => {
          logger.error(err, 'Failed to stream workflow logs');
          writeSse(res, 'error', { code: 'INTERNAL_ERROR', message: 'Failed to stream workflow logs' });
          clearInterval(interval);
          res.end();
        });
    }, 1000);

    req.on('close', () => clearInterval(interval));
  } catch (err) {
    logger.error(err, 'Failed to open workflow log stream');
    if (!res.headersSent) {
      res.status(500).json({ error: { code: 'INTERNAL_ERROR', message: 'Failed to open workflow log stream' } });
    } else {
      writeSse(res, 'error', { code: 'INTERNAL_ERROR', message: 'Failed to open workflow log stream' });
      res.end();
    }
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

// --- Dispatch (Epic 5 Story 5.1) ---

const dispatchSchema = z.object({
  workflow: z
    .string()
    .min(1)
    .max(200)
    .refine((v) => v === basename(v) && !v.includes('/') && !v.includes('..'), {
      message: 'workflow must be a plain basename (no path separators)',
    }),
  target: z.string().min(1).max(500),
  repo: z
    .string()
    .regex(/^[\w.-]+\/[\w.-]+$/, 'repo must be OWNER/REPO')
    .optional(),
  branch: z.string().max(200).optional(),
  vars: z.record(z.string(), z.string()).optional(),
});

workflowsRouter.post('/workflows/dispatch', requireAuth, async (req: Request, res: Response) => {
  const parsed = dispatchSchema.safeParse(req.body);
  if (!parsed.success) {
    res.status(400).json({
      error: { code: 'VALIDATION_FAILED', message: 'Invalid dispatch payload', details: parsed.error.issues },
    });
    return;
  }

  const { workflow, target, repo, branch, vars } = parsed.data;
  if (!repo && !envFlag('STEPYARD_DISPATCH_ALLOW_LOCAL_WORKSPACE')) {
    res.status(400).json({
      error: {
        code: 'REPO_REQUIRED',
        message:
          'Remote dispatch requires repo (OWNER/REPO). Set default_repo in remote.toml, pass --repo, or explicitly enable STEPYARD_DISPATCH_ALLOW_LOCAL_WORKSPACE=true on the API host.',
      },
    });
    return;
  }

  const workflowsDir = process.env.STEPYARD_WORKFLOWS_DIR || '/root/.stepyard/workflows';
  const workflowFile = join(workflowsDir, `${workflow}.yaml`);

  if (!existsSync(workflowFile)) {
    res.status(404).json({
      error: {
        code: 'WORKFLOW_NOT_FOUND',
        message: `Workflow '${workflow}' not found in ${workflowsDir}`,
      },
    });
    return;
  }

  const stepyardArgs: string[] = ['execute'];
  if (repo) stepyardArgs.push('--repo', repo);
  if (branch) stepyardArgs.push('--var', `branch=${branch}`);
  if (vars) {
    for (const [k, v] of Object.entries(vars)) {
      stepyardArgs.push('--var', `${k}=${v}`);
    }
  }
  stepyardArgs.push(workflowFile, '--', target);

  // Two deploy modes:
  //   1. `STEPYARD_DISPATCH_SSH_HOST` set → SSH back to host, run stepyard there
  //      (used when the API runs in a container that doesn't have stepyard installed)
  //   2. otherwise → spawn `stepyard` directly from $PATH
  // NOTE: Default binary name is still 'minion' until the VPS migration flips
  // the installed binary — override with STEPYARD_BINARY=stepyard once deployed.
  // Log file for the detached process. Persists after the api request returns
  // so operators can inspect failures post-hoc. Written to /tmp; container-local.
  const logDir = process.env.STEPYARD_DISPATCH_LOG_DIR || '/tmp';
  try { mkdirSync(logDir, { recursive: true }); } catch {}
  const runId = randomUUID();
  const ts = new Date().toISOString().replace(/[:.]/g, '-');
  const logPath = join(logDir, `stepyard-dispatch-${ts}-${runId}.log`);

  const dispatchEnv = {
    ...process.env,
    STEPYARD_DASHBOARD_RUN_ID: runId,
    STEPYARD_DISPATCH_LOG_PATH: logPath,
  };

  const sshHost = process.env.STEPYARD_DISPATCH_SSH_HOST;
  const [binary, args] = sshHost
    ? buildSshCommand(sshHost, stepyardArgs, dispatchEnv)
    : [process.env.STEPYARD_BINARY || 'minion', stepyardArgs];

  logger.info(
    { binary, args, workflow, target, repo, branch, sshHost: !!sshHost, runId, logPath },
    'Dispatching stepyard run',
  );

  try {
    await pool.query(
      `INSERT INTO workflow_runs (
        run_id, user_name, workflow, target, repo, status, started_at,
        dispatch_log_path, event_version, updated_at
      ) VALUES ($1, $2, $3, $4, $5, $6, NOW(), $7, 0, NOW())
      ON CONFLICT (run_id) DO UPDATE SET
        status = EXCLUDED.status,
        dispatch_log_path = EXCLUDED.dispatch_log_path,
        updated_at = NOW()`,
      [
        runId,
        process.env.STEPYARD_USER || 'remote',
        workflow,
        target,
        repo ?? null,
        'dispatching',
        logPath,
      ],
    );

    const logFd = openSync(logPath, 'a');
    const child = spawn(binary, args, {
      detached: true,
      stdio: ['ignore', logFd, logFd],
      env: dispatchEnv,
    });
    child.on('error', (err) => {
      logger.error({ err, args }, 'Spawned stepyard emitted error event');
    });
    child.unref();
    closeSync(logFd);
    const pid = child.pid;
    if (typeof pid !== 'number') {
      await markDispatchFailed(runId, 'Failed to spawn stepyard (no pid)');
      res.status(500).json({ error: { code: 'SPAWN_FAILED', message: 'Failed to spawn stepyard (no pid)' } });
      return;
    }
    res.status(202).json({
      run_id: runId,
      dispatched_at: new Date().toISOString(),
      pid,
      workflow,
      target,
      repo: repo ?? null,
      branch: branch ?? null,
      log: logPath,
    });
  } catch (err) {
    logger.error({ err, args }, 'Failed to spawn stepyard');
    await markDispatchFailed(runId, (err as Error).message);
    res.status(500).json({ error: { code: 'SPAWN_FAILED', message: (err as Error).message } });
  }
});

// --- Helpers ---

/**
 * Wrap a stepyard invocation in an SSH call. The remote side runs the engine
 * via `bash -lc '<envs> exec stepyard ...'` so the host's PATH is loaded and
 * workflow-level env vars are forwarded from STEPYARD_SSH_ENV_FORWARD.
 *
 * STEPYARD_SSH_ENV_FORWARD accepts a comma-separated list of:
 *   - `NAME`          → forward `NAME=$NAME` (simple)
 *   - `NAME:SOURCE`   → forward `NAME=$SOURCE` (remap — e.g. when the api
 *                       container's DATABASE_URL points at `db` but the host
 *                       needs `localhost`)
 */
function buildSshCommand(sshHost: string, stepyardArgs: string[], env: NodeJS.ProcessEnv): [string, string[]] {
  const forward = (process.env.STEPYARD_SSH_ENV_FORWARD || '')
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean);
  const exports: string[] = [];
  for (const name of ['STEPYARD_DASHBOARD_RUN_ID', 'STEPYARD_DISPATCH_LOG_PATH']) {
    const value = env[name];
    if (value != null) {
      exports.push(`${name}=${shellQuote(value)}`);
    }
  }
  for (const entry of forward) {
    const [target, source] = entry.includes(':') ? entry.split(':', 2) : [entry, entry];
    const value = process.env[source];
    if (value != null) {
      exports.push(`${target}=${shellQuote(value)}`);
    }
  }
  // Ensure a clean git-enabled CWD for the engine. Required when not in --repo
  // mode because stepyard's sandbox_up expects a git repo to copy as workspace.
  // The dir is created idempotently and reused across dispatches.
  const ws = process.env.STEPYARD_DISPATCH_WORKSPACE || '/tmp/stepyard-dispatch-ws';
  const prep = [
    `mkdir -p ${shellQuote(ws)}`,
    `cd ${shellQuote(ws)}`,
    `[ -d .git ] || { git init -q && echo dispatch > .dispatch && git add .dispatch && git -c user.email=stepyard@localhost -c user.name=Stepyard commit -qm init ; }`,
  ].join(' && ');
  // Use an absolute path so we don't rely on the login shell's PATH order.
  // Some hosts have multiple stepyard binaries (e.g. ~/.cargo/bin vs /usr/local/bin)
  // which may be different versions. Default still points at the legacy
  // /usr/local/bin/minion path; override STEPYARD_HOST_BINARY post-VPS-migration.
  const stepyardBin = process.env.STEPYARD_HOST_BINARY || '/usr/local/bin/minion';
  const remoteCmd = `${prep} && ${exports.join(' ')} exec ${shellQuote(stepyardBin)} ${stepyardArgs.map(shellQuote).join(' ')}`;
  // ssh concatenates remote argv with spaces before feeding it to the remote
  // shell, which then re-parses. Pass the bash invocation as ONE argument so
  // the `&&` / quoting in remoteCmd survives.
  const sshArgs = [
    '-o', 'StrictHostKeyChecking=no',
    '-o', 'BatchMode=yes',
    sshHost,
    `bash -lc ${shellQuote(remoteCmd)}`,
  ];
  return ['ssh', sshArgs];
}

async function markDispatchFailed(runId: string, message: string): Promise<void> {
  try {
    await pool.query(
      `UPDATE workflow_runs
       SET status = 'dispatch_failed', error = $2, updated_at = NOW()
       WHERE run_id = $1 AND event_version = 0`,
      [runId, message],
    );
  } catch (err) {
    logger.warn({ err, runId }, 'Failed to mark dispatch as failed');
  }
}

function shellQuote(value: string): string {
  if (/^[A-Za-z0-9_\-./=:,@%+]+$/.test(value)) return value;
  return `'${value.replace(/'/g, `'\\''`)}'`;
}

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
