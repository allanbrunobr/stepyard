import express from 'express';
import type { Server } from 'http';
import type { AddressInfo } from 'net';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { jsonBodyParser } from './body';
import { errorHandler } from './errors';

// SEG-TS-06 (analysis 2026-11-09): the global 100 kB JSON limit made every
// artifact upload above ~75 kB fail with 413, although the documented limit
// is 10 MiB.

describe('jsonBodyParser', () => {
  let server: Server;
  let url: string;
  const saved = { secret: process.env.API_SECRET, max: process.env.STEPYARD_ARTIFACT_MAX_BYTES };

  beforeAll(async () => {
    process.env.API_SECRET = 'test-secret';
    process.env.STEPYARD_ARTIFACT_MAX_BYTES = String(1024 * 1024); // 1 MiB decoded
    const app = express();
    app.use(jsonBodyParser());
    app.post('/api/workflows/:runId/artifacts', (req, res) => {
      res.json({ chars: String(req.body.content_base64 ?? '').length });
    });
    app.post('/api/events', (req, res) => res.json({ keys: Object.keys(req.body).length }));
    app.use(errorHandler);
    await new Promise<void>((resolve) => {
      server = app.listen(0, '127.0.0.1', () => resolve());
    });
    url = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  });
  afterAll(() => {
    server.close();
    for (const [key, value] of [['API_SECRET', saved.secret], ['STEPYARD_ARTIFACT_MAX_BYTES', saved.max]] as const) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  });

  const post = (path: string, body: unknown, auth = true) =>
    fetch(`${url}${path}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', ...(auth ? { authorization: 'Bearer test-secret' } : {}) },
      body: JSON.stringify(body),
    });
  const artifact = (bytes: number) => ({ name: 'a.bin', content_base64: Buffer.alloc(bytes, 7).toString('base64') });

  it('accepts an authenticated artifact well above 100 kB (it used to be 413)', async () => {
    const res = await post('/api/workflows/r1/artifacts', artifact(300 * 1024));
    expect(res.status).toBe(200);
    expect(((await res.json()) as { chars: number }).chars).toBe(409600);
  });

  it('refuses an unauthenticated upload before reading the large body', async () => {
    const res = await post('/api/workflows/r1/artifacts', artifact(300 * 1024), false);
    expect(res.status).toBe(401);
  });

  it('keeps 100 kB on every other route', async () => {
    const res = await post('/api/events', { blob: 'x'.repeat(200 * 1024) });
    expect(res.status).toBe(413);
  });

  it('still caps the artifact at the configured size (base64 form)', async () => {
    const res = await post('/api/workflows/r1/artifacts', artifact(2 * 1024 * 1024));
    expect(res.status).toBe(413);
  });
});
