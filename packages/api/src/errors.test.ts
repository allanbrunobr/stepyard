import express, { Router } from 'express';
import type { AddressInfo } from 'net';
import type { Server } from 'http';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';

// P1-5 (analysis 2026-11-09, BUG-CLI-05 / SEG-TS-03): a failing request must
// answer 5xx, never become an unhandled rejection that ends the API process.

const db = vi.hoisted(() => ({ connect: vi.fn() }));
vi.mock('./db', async (importOriginal) => {
  const real = await importOriginal<typeof import('./db')>();
  // Keep the real Pool object (to test its 'error' listener) but let each
  // test decide what `connect()` does.
  real.pool.connect = db.connect as unknown as typeof real.pool.connect;
  return real;
});

import { errorHandler, forwardAsyncErrors } from './errors';
import { pool } from './db';
import { eventsRouter } from './routes/events';

async function listen(app: express.Express): Promise<{ server: Server; url: string }> {
  return new Promise((resolve) => {
    const server = app.listen(0, '127.0.0.1', () => {
      const { port } = server.address() as AddressInfo;
      resolve({ server, url: `http://127.0.0.1:${port}` });
    });
  });
}

describe('async route errors', () => {
  let server: Server;
  let url: string;
  const unhandled = vi.fn();

  beforeAll(async () => {
    const router = forwardAsyncErrors(Router());
    router.get('/db-down', async () => {
      const err = Object.assign(new Error('connect ECONNREFUSED postgres://stepyard:pw@db:5432'), {
        code: 'ECONNREFUSED',
      });
      throw err;
    });
    router.get('/boom', async () => {
      throw new Error('SELECT secret FROM table');
    });
    router.get('/sync-throw', () => {
      throw new Error('sync');
    });
    router.get('/ok', async (_req, res) => {
      res.json({ ok: true });
    });
    const app = express();
    app.use(express.json({ limit: '1kb' }));
    app.use(router);
    app.post('/echo', (req, res) => res.json(req.body));
    app.use(errorHandler);
    ({ server, url } = await listen(app));
    process.on('unhandledRejection', unhandled);
  });
  afterAll(() => {
    process.off('unhandledRejection', unhandled);
    server.close();
  });

  it('answers 503 for a database outage, without leaking the message', async () => {
    const res = await fetch(`${url}/db-down`);
    expect(res.status).toBe(503);
    const body = await res.text();
    expect(body).toContain('DATABASE_UNAVAILABLE');
    expect(body).not.toContain('pw@db');
  });

  it('answers 500 for any other rejection or throw, without leaking it', async () => {
    const res = await fetch(`${url}/boom`);
    expect(res.status).toBe(500);
    expect(await res.text()).not.toContain('SELECT');
    expect((await fetch(`${url}/sync-throw`)).status).toBe(500);
    expect((await fetch(`${url}/ok`)).status).toBe(200);
  });

  it('keeps body-parser client errors as 4xx', async () => {
    const bad = await fetch(`${url}/echo`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{' });
    expect(bad.status).toBe(400);
    const big = await fetch(`${url}/echo`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ x: 'a'.repeat(4096) }),
    });
    expect(big.status).toBe(413);
  });

  it('produced no unhandled rejection', () => {
    expect(unhandled).not.toHaveBeenCalled();
  });
});

describe('POST /api/events while Postgres is down', () => {
  let server: Server;
  let url: string;
  const saved = process.env.API_SECRET;
  const event = {
    run_id: '6f1c2f43-2f43-4c2f-8f43-2f432f432f43',
    user_name: 'u',
    workflow: 'w',
    status: 'running',
    started_at: '2026-11-10T00:00:00Z',
  };
  const post = () =>
    fetch(`${url}/api/events`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', authorization: 'Bearer test-secret' },
      body: JSON.stringify(event),
    });

  beforeAll(async () => {
    process.env.API_SECRET = 'test-secret';
    const app = express();
    app.use(express.json());
    app.use('/api', eventsRouter);
    app.use(errorHandler);
    ({ server, url } = await listen(app));
  });
  afterAll(() => {
    server.close();
    if (saved === undefined) delete process.env.API_SECRET;
    else process.env.API_SECRET = saved;
  });
  beforeEach(() => db.connect.mockReset());
  afterEach(() => vi.restoreAllMocks());

  it('answers 503 when the pool cannot connect (used to be an unhandled rejection)', async () => {
    db.connect.mockRejectedValue(Object.assign(new Error('connect ECONNREFUSED'), { code: 'ECONNREFUSED' }));
    const res = await post();
    expect(res.status).toBe(503);
  });

  it('destroys the client when ROLLBACK also fails, and still answers', async () => {
    const release = vi.fn();
    const dead = Object.assign(new Error('Connection terminated unexpectedly'), {});
    db.connect.mockResolvedValue({ query: vi.fn().mockRejectedValue(dead), release });
    const res = await post();
    expect(res.status).toBe(500);
    expect(release).toHaveBeenCalledTimes(1);
    // release(err) makes pg destroy the client instead of reusing it.
    expect(release.mock.calls[0][0]).toBeInstanceOf(Error);
  });
});

describe('pool', () => {
  it("has an 'error' listener, so an idle client error does not throw", () => {
    expect(pool.listenerCount('error')).toBeGreaterThan(0);
    expect(() => pool.emit('error', new Error('terminating connection due to administrator command'))).not.toThrow();
  });
});
