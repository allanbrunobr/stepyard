import { Router, Request, Response } from 'express';
import { z } from 'zod';
import { pool } from '../db';
import { logger } from '../logger';

export const eventsRouter = Router();

const StepSchema = z.object({
  step_name: z.string(),
  step_type: z.string().optional(),
  status: z.string(),
  duration_ms: z.number().int().optional(),
  tokens_in: z.number().int().optional(),
  tokens_out: z.number().int().optional(),
  sandboxed: z.boolean().optional(),
  error: z.string().optional(),
});

const EventSchema = z.object({
  run_id: z.string().uuid(),
  user_name: z.string().min(1),
  workflow: z.string().min(1),
  target: z.string().optional(),
  repo: z.string().optional(),
  status: z.string().min(1),
  duration_ms: z.number().int().optional(),
  total_tokens: z.number().int().optional(),
  cost_usd: z.number().optional(),
  started_at: z.string(),
  finished_at: z.string().optional(),
  error: z.string().optional(),
  event_version: z.number().int().min(1).default(1),
  steps: z.array(StepSchema).optional(),
});

function authenticate(req: Request, res: Response): boolean {
  const authHeader = req.headers.authorization;
  if (!authHeader || !authHeader.startsWith('Bearer ')) {
    res.status(401).json({ error: 'Missing or invalid authorization header' });
    return false;
  }

  const token = authHeader.slice(7);
  const secret = process.env.API_SECRET;
  if (!secret || token !== secret) {
    res.status(401).json({ error: 'Invalid token' });
    return false;
  }

  return true;
}

eventsRouter.post('/events', async (req: Request, res: Response) => {
  if (!authenticate(req, res)) return;

  const parsed = EventSchema.safeParse(req.body);
  if (!parsed.success) {
    res.status(400).json({
      error: 'Validation failed',
      details: parsed.error.issues,
    });
    return;
  }

  const data = parsed.data;
  const client = await pool.connect();

  try {
    await client.query('BEGIN');

    const upsertResult = await client.query(
      `INSERT INTO workflow_runs (
        run_id, user_name, workflow, target, repo, status,
        duration_ms, total_tokens, cost_usd, started_at, finished_at, error, event_version, updated_at
      ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, NOW())
      ON CONFLICT (run_id) DO UPDATE SET
        user_name = EXCLUDED.user_name,
        workflow = EXCLUDED.workflow,
        target = EXCLUDED.target,
        repo = EXCLUDED.repo,
        status = EXCLUDED.status,
        duration_ms = EXCLUDED.duration_ms,
        total_tokens = EXCLUDED.total_tokens,
        cost_usd = EXCLUDED.cost_usd,
        started_at = EXCLUDED.started_at,
        finished_at = EXCLUDED.finished_at,
        error = EXCLUDED.error,
        event_version = EXCLUDED.event_version,
        updated_at = NOW()
      WHERE workflow_runs.event_version < EXCLUDED.event_version
      RETURNING (xmax = 0) AS is_insert`,
      [
        data.run_id, data.user_name, data.workflow, data.target ?? null,
        data.repo ?? null, data.status, data.duration_ms ?? null,
        data.total_tokens ?? null, data.cost_usd ?? null,
        data.started_at, data.finished_at ?? null, data.error ?? null,
        data.event_version,
      ]
    );

    if (upsertResult.rowCount === 0) {
      await client.query('ROLLBACK');
      res.status(409).json({ error: 'Stale event rejected — a newer update already exists' });
      return;
    }

    const isInsert = upsertResult.rows[0].is_insert;

    if (data.steps && data.steps.length > 0) {
      // Replace steps only when the payload explicitly includes them
      await client.query('DELETE FROM workflow_steps WHERE run_id = $1', [data.run_id]);

      for (const step of data.steps) {
        await client.query(
          `INSERT INTO workflow_steps (run_id, step_name, step_type, status, duration_ms, tokens_in, tokens_out, sandboxed, error)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)`,
          [
            data.run_id, step.step_name, step.step_type ?? null,
            step.status, step.duration_ms ?? null,
            step.tokens_in ?? null, step.tokens_out ?? null,
            step.sandboxed ?? null, step.error ?? null,
          ]
        );
      }
    }

    await client.query('COMMIT');

    const statusCode = isInsert ? 201 : 200;
    res.status(statusCode).json({
      message: isInsert ? 'Event created' : 'Event updated',
      run_id: data.run_id,
    });
  } catch (err) {
    await client.query('ROLLBACK');
    logger.error(err, 'Failed to process event');
    res.status(500).json({ error: 'Internal server error' });
  } finally {
    client.release();
  }
});
