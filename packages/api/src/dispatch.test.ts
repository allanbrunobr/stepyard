import { spawnSync } from 'child_process';
import { existsSync, mkdtempSync, symlinkSync, writeFileSync } from 'fs';
import { tmpdir } from 'os';
import { join } from 'path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { allowedDispatchLogPath, buildSshCommand, readDispatchLogChunk } from './dispatch';
import { EventSchema } from './routes/events';

// P1-4 (analysis 2026-11-09, SEG-TS-01 / SEG-TS-02).

describe('dispatch log path (SEG-TS-01)', () => {
  let logDir: string;
  let outside: string;
  const saved = process.env.STEPYARD_DISPATCH_LOG_DIR;

  beforeEach(() => {
    logDir = mkdtempSync(join(tmpdir(), 'stepyard-logs-'));
    outside = mkdtempSync(join(tmpdir(), 'stepyard-secret-'));
    process.env.STEPYARD_DISPATCH_LOG_DIR = logDir;
  });
  afterEach(() => {
    if (saved === undefined) delete process.env.STEPYARD_DISPATCH_LOG_DIR;
    else process.env.STEPYARD_DISPATCH_LOG_DIR = saved;
  });

  it('accepts only stepyard-dispatch-*.log directly inside the log dir', () => {
    const good = join(logDir, 'stepyard-dispatch-2026-11-10T00-00-00-000Z-abc.log');
    expect(allowedDispatchLogPath(good)).toBe(good);
    for (const bad of [
      '/root/.ssh/id_ed25519',
      '/etc/passwd',
      join(logDir, '..', 'stepyard-dispatch-x.log'),
      join(logDir, 'sub', 'stepyard-dispatch-x.log'),
      join(logDir, 'other.log'),
      join(logDir, 'stepyard-dispatch-x.log.bak'),
      `${join(logDir, 'stepyard-dispatch-x.log')}\0`,
      '',
      42,
      null,
    ]) {
      expect(allowedDispatchLogPath(bad), String(bad)).toBeNull();
    }
  });

  it('streams a real dispatch log', () => {
    const path = join(logDir, 'stepyard-dispatch-run.log');
    writeFileSync(path, 'step 1 ok\n');
    expect(readDispatchLogChunk(path, 0, 8192)).toEqual({ text: 'step 1 ok\n', nextOffset: 10, hasMore: false });
  });

  it('refuses a path outside the log dir even when the file exists', () => {
    const secret = join(outside, 'id_ed25519');
    writeFileSync(secret, 'PRIVATE KEY');
    expect(readDispatchLogChunk(secret, 0, 8192)).toBeNull();
  });

  it('refuses a symlink planted under an allowed name', () => {
    const secret = join(outside, 'id_ed25519');
    writeFileSync(secret, 'PRIVATE KEY');
    const link = join(logDir, 'stepyard-dispatch-evil.log');
    symlinkSync(secret, link);
    expect(readDispatchLogChunk(link, 0, 8192)).toBeNull();
  });

  it('POST /api/events can no longer set dispatch_log_path', () => {
    const parsed = EventSchema.parse({
      run_id: '6f1c2f43-2f43-4c2f-8f43-2f432f432f43',
      user_name: 'u',
      workflow: 'w',
      status: 'running',
      started_at: '2026-11-10T00:00:00Z',
      dispatch_log_path: '/root/.ssh/id_ed25519',
    });
    expect(parsed).not.toHaveProperty('dispatch_log_path');
  });
});

describe('SSH dispatch secrets (SEG-TS-02)', () => {
  const SECRET = `s3cr3t-'$(touch PWNED)"\`id\``;
  const savedEnv = { ...process.env };

  beforeEach(() => {
    process.env.GH_TOKEN = SECRET;
    process.env.HOST_DB = 'postgres://u:dbpass@localhost/x';
    process.env.STEPYARD_SSH_ENV_FORWARD = 'GH_TOKEN,DATABASE_URL:HOST_DB,BAD-NAME';
  });
  afterEach(() => {
    process.env = { ...savedEnv };
  });

  it('keeps forwarded secrets out of argv (and therefore out of ps and the log line)', () => {
    const env = { ...process.env, STEPYARD_DASHBOARD_RUN_ID: 'run-1', STEPYARD_DISPATCH_LOG_PATH: '/tmp/x.log' };
    const { binary, args, stdin } = buildSshCommand('host', ['execute', 'w.yaml', '--', '42'], env);
    expect(binary).toBe('ssh');
    const argv = JSON.stringify(args);
    expect(argv).not.toContain('s3cr3t');
    expect(argv).not.toContain('dbpass');
    expect(stdin).toContain('export GH_TOKEN=');
    expect(stdin).toContain('export DATABASE_URL=postgres://u:dbpass@localhost/x');
    expect(stdin).not.toContain('BAD-NAME');
  });

  it('the remote command receives the exact values from stdin, without executing them', () => {
    const ws = mkdtempSync(join(tmpdir(), 'stepyard-ws-'));
    process.env.STEPYARD_DISPATCH_WORKSPACE = ws;
    process.env.STEPYARD_HOST_BINARY = '/bin/sh';
    const env = { ...process.env, STEPYARD_DASHBOARD_RUN_ID: 'run-1' };
    // The "engine" is /bin/sh printing what it received.
    const { args, stdin } = buildSshCommand('host', ['-c', 'printf "%s|%s|%s" "$GH_TOKEN" "$DATABASE_URL" "$STEPYARD_DASHBOARD_RUN_ID"'], env);
    // What sshd hands to the remote login shell is the last argv element.
    const remote = args[args.length - 1];
    const out = spawnSync('sh', ['-c', remote], {
      input: stdin,
      cwd: ws,
      env: { PATH: process.env.PATH, HOME: ws },
      encoding: 'utf8',
      timeout: 30_000,
    });
    expect(out.status, out.stderr).toBe(0);
    expect(out.stdout).toBe(`${SECRET}|postgres://u:dbpass@localhost/x|run-1`);
    expect(existsSync(join(ws, 'PWNED'))).toBe(false);
  });
});
