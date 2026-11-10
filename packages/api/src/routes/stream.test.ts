import express from 'express';
import type { Server } from 'http';
import type { AddressInfo } from 'net';
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';

// Pre-PR contradiction (2026-11-10): a run in `dispatch_failed` (engine died
// before reporting) never got `event: done` on the log stream, so
// `stepyard remote logs` hung forever.

const db = vi.hoisted(() => ({ query: vi.fn() }));
vi.mock('../db', async (importOriginal) => {
  const real = await importOriginal<typeof import('../db')>();
  real.pool.query = db.query as unknown as typeof real.pool.query;
  return real;
});

import { errorHandler } from '../errors';
import { workflowsRouter } from './workflows';

const RUN_ID = '6f1c2f43-2f43-4c2f-8f43-2f432f432f43';

describe('GET /api/workflows/:runId/logs/stream', () => {
  let server: Server;
  let url: string;
  const saved = process.env.API_SECRET;

  beforeAll(async () => {
    process.env.API_SECRET = 'test-secret';
    db.query.mockImplementation(async (sql: string) => {
      if (sql.includes('FROM workflow_runs')) {
        return { rows: [{ run_id: RUN_ID, status: 'dispatch_failed', dispatch_log_path: null }], rowCount: 1 };
      }
      return { rows: [], rowCount: 0 };
    });
    const app = express();
    app.use('/api', workflowsRouter);
    app.use(errorHandler);
    await new Promise<void>((resolve) => {
      server = app.listen(0, '127.0.0.1', () => resolve());
    });
    url = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  });
  afterAll(() => {
    server.close();
    if (saved === undefined) delete process.env.API_SECRET;
    else process.env.API_SECRET = saved;
  });

  it('ends with `done` for a dispatch_failed run', async () => {
    const res = await fetch(`${url}/api/workflows/${RUN_ID}/logs/stream`, {
      headers: { authorization: 'Bearer test-secret' },
      signal: AbortSignal.timeout(8000),
    });
    expect(res.status).toBe(200);
    // The server closes the stream after `done`; reading to the end proves it.
    const body = await res.text();
    expect(body).toContain('event: done');
    expect(body).toContain('"status":"dispatch_failed"');
  });
});
