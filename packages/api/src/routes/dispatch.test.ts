import express from 'express';
import { chmodSync, existsSync, mkdtempSync, readFileSync, writeFileSync } from 'fs';
import type { Server } from 'http';
import type { AddressInfo } from 'net';
import { tmpdir } from 'os';
import { join } from 'path';
import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';

// P1-6 (analysis 2026-11-09, ARQ-02 / BUG-CLI-03 / BUG-CLI-04 / SEG-TS-09):
// the dispatcher built a command line the default engine refused
// (`--var branch=`) or ignored (`--repo` on v2), answered 202, and left the
// run in `dispatching` forever when the child died.

const db = vi.hoisted(() => ({ query: vi.fn() }));
vi.mock('../db', async (importOriginal) => {
  const real = await importOriginal<typeof import('../db')>();
  real.pool.query = db.query as unknown as typeof real.pool.query;
  return real;
});

import { errorHandler } from '../errors';
import { workflowsRouter } from './workflows';

describe('POST /api/workflows/dispatch', () => {
  let server: Server;
  let url: string;
  let dir: string;
  let argvOut: string;
  const savedEnv = { ...process.env };

  beforeAll(async () => {
    dir = mkdtempSync(join(tmpdir(), 'stepyard-dispatch-'));
    writeFileSync(join(dir, 'fix-issue.yaml'), 'name: fix-issue\nsteps: []\n');
    argvOut = join(dir, 'argv.txt');
    // Fake engine: records its argv, then fails like clap would.
    const fake = join(dir, 'fake-stepyard.sh');
    writeFileSync(fake, `#!/bin/sh\nprintf '%s\\n' "$@" > "${argvOut}"\nexit 2\n`);
    chmodSync(fake, 0o755);
    process.env.API_SECRET = 'test-secret';
    process.env.STEPYARD_WORKFLOWS_DIR = dir;
    process.env.STEPYARD_BINARY = fake;
    process.env.STEPYARD_DISPATCH_LOG_DIR = dir;
    delete process.env.STEPYARD_DISPATCH_SSH_HOST;

    const app = express();
    app.use(express.json());
    app.use('/api', workflowsRouter);
    app.use(errorHandler);
    await new Promise<void>((resolve) => {
      server = app.listen(0, '127.0.0.1', () => resolve());
    });
    url = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  });
  afterAll(() => {
    server.close();
    process.env = { ...savedEnv };
  });
  beforeEach(() => {
    db.query.mockReset();
    db.query.mockResolvedValue({ rows: [], rowCount: 1 });
  });

  const dispatch = (body: unknown) =>
    fetch(`${url}/api/workflows/dispatch`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', authorization: 'Bearer test-secret' },
      body: JSON.stringify(body),
    });

  it('runs the v1 engine with BRANCH upper-cased, and marks the run failed when the child dies', async () => {
    const res = await dispatch({
      workflow: 'fix-issue',
      target: '42',
      repo: 'acme/app',
      branch: 'feature/x',
      vars: { MODE: 'fast' },
    });
    expect(res.status).toBe(202);
    const { run_id: runId } = (await res.json()) as { run_id: string };

    await vi.waitFor(() => expect(existsSync(argvOut)).toBe(true), { timeout: 5000 });
    const argv = readFileSync(argvOut, 'utf8').trim().split('\n');
    expect(argv.slice(0, 3)).toEqual(['execute', '--engine', 'v1']);
    expect(argv).toContain('BRANCH=feature/x');
    expect(argv).not.toContain('branch=feature/x');
    expect(argv).toContain('MODE=fast');
    expect(argv.slice(-2)).toEqual(['--', '42']);

    // The child exits 2: the run must not stay in `dispatching`.
    await vi.waitFor(
      () => {
        const failed = db.query.mock.calls.find(([sql]) => String(sql).includes("'dispatch_failed'"));
        expect(failed).toBeDefined();
        expect(failed?.[1]?.[0]).toBe(runId);
        expect(String(failed?.[1]?.[1])).toContain('exit code 2');
      },
      { timeout: 5000 },
    );
  });

  it('refuses var keys the CLI would reject, before spawning anything', async () => {
    const res = await dispatch({ workflow: 'fix-issue', target: '42', repo: 'acme/app', vars: { foo: '1' } });
    expect(res.status).toBe(400);
    expect(db.query).not.toHaveBeenCalled();
  });
});
