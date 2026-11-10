import { closeSync, constants, fstatSync, openSync, readSync } from 'fs';
import { basename, dirname, resolve } from 'path';
import { logger } from './logger';

/**
 * Directory where the dispatch handler writes `stepyard-dispatch-*.log`.
 * Only files directly inside it, with that name, can be streamed back.
 */
export function dispatchLogDir(): string {
  return process.env.STEPYARD_DISPATCH_LOG_DIR || '/tmp';
}

const DISPATCH_LOG_NAME = /^stepyard-dispatch-[A-Za-z0-9_-]+\.log$/;

/**
 * Validate a dispatch log path read from `workflow_runs.dispatch_log_path`.
 *
 * The column used to be writable through `POST /api/events`, so any holder of
 * the ingest secret could point it at `/root/.ssh/id_ed25519` and read the
 * file through the log stream. Rows written before the fix may still hold such
 * paths, so the check runs on every read: the path must sit directly inside
 * the dispatch log directory and carry the name the dispatch handler writes.
 * The final component is additionally opened with O_NOFOLLOW (see
 * {@link readDispatchLogChunk}), so a symlink with an allowed name is refused.
 */
export function allowedDispatchLogPath(path: unknown, logDir: string = dispatchLogDir()): string | null {
  if (typeof path !== 'string' || path.length === 0 || path.includes('\0')) {
    return null;
  }
  const resolved = resolve(path);
  if (dirname(resolved) !== resolve(logDir)) {
    return null;
  }
  if (!DISPATCH_LOG_NAME.test(basename(resolved))) {
    return null;
  }
  return resolved;
}

export interface LogChunk {
  text: string;
  nextOffset: number;
  hasMore: boolean;
}

/**
 * Read up to `maxBytes` of a dispatch log from `offset`. Returns null when the
 * path is not an allowed dispatch log, is a symlink, is not a regular file, or
 * has nothing new to read.
 */
export function readDispatchLogChunk(path: string, offset: number, maxBytes: number): LogChunk | null {
  const allowed = allowedDispatchLogPath(path);
  if (allowed === null) {
    return null;
  }
  let fd: number;
  try {
    // O_NOFOLLOW: a symlink planted under an allowed name fails with ELOOP.
    fd = openSync(allowed, constants.O_RDONLY | constants.O_NOFOLLOW);
  } catch (err) {
    const code = (err as NodeJS.ErrnoException).code;
    if (code !== 'ENOENT') {
      logger.warn({ err, path: allowed }, 'Unable to open dispatch log');
    }
    return null;
  }
  try {
    const stat = fstatSync(fd);
    if (!stat.isFile() || stat.size <= offset) {
      return null;
    }
    const bytesToRead = Math.min(stat.size - offset, maxBytes);
    const buffer = Buffer.alloc(bytesToRead);
    const bytesRead = readSync(fd, buffer, 0, bytesToRead, offset);
    return {
      text: sanitizeLogText(buffer.subarray(0, bytesRead).toString('utf8')),
      nextOffset: offset + bytesRead,
      hasMore: stat.size > offset + bytesRead,
    };
  } catch (err) {
    logger.warn({ err, path: allowed }, 'Unable to read dispatch log chunk');
    return null;
  } finally {
    closeSync(fd);
  }
}

export function sanitizeLogText(input: string): string {
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

export function shellQuote(value: string): string {
  if (/^[A-Za-z0-9_\-./=:,@%+]+$/.test(value)) return value;
  return `'${value.replace(/'/g, `'\\''`)}'`;
}

export interface SshDispatch {
  binary: string;
  args: string[];
  /** Shell `export` lines the remote side reads from stdin; never in argv. */
  stdin: string;
}

/**
 * Wrap a stepyard invocation in an SSH call. The remote side runs the engine
 * via `bash -lc '<prep> && <read env from stdin> && exec stepyard ...'` so the
 * host's PATH is loaded.
 *
 * Forwarded variables (STEPYARD_SSH_ENV_FORWARD, plus the dashboard run id and
 * log path) are written to the child's stdin as `export NAME='value'` lines and
 * evaluated remotely. They used to be inlined in the remote command, which put
 * GH_TOKEN, ANTHROPIC_API_KEY and DATABASE_URL in the ssh argv (visible in
 * `ps` on the API host), in the remote `bash -lc` argv, and in the API log.
 *
 * STEPYARD_SSH_ENV_FORWARD accepts a comma-separated list of:
 *   - `NAME`          → forward `NAME=$NAME` (simple)
 *   - `NAME:SOURCE`   → forward `NAME=$SOURCE` (remap — e.g. when the api
 *                       container's DATABASE_URL points at `db` but the host
 *                       needs `localhost`)
 */
export function buildSshCommand(sshHost: string, stepyardArgs: string[], env: NodeJS.ProcessEnv): SshDispatch {
  const forward = (process.env.STEPYARD_SSH_ENV_FORWARD || '')
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean);
  const exports: string[] = [];
  for (const name of ['STEPYARD_DASHBOARD_RUN_ID', 'STEPYARD_DISPATCH_LOG_PATH']) {
    const value = env[name];
    if (value != null) {
      exports.push(`export ${name}=${shellQuote(value)}`);
    }
  }
  for (const entry of forward) {
    const [target, source] = entry.includes(':') ? entry.split(':', 2) : [entry, entry];
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(target)) {
      logger.warn({ name: target }, 'Ignoring invalid STEPYARD_SSH_ENV_FORWARD entry');
      continue;
    }
    const value = process.env[source];
    if (value) {
      exports.push(`export ${target}=${shellQuote(value)}`);
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
  // `eval "$(cat)"` reads the export lines from stdin until EOF; the API ends
  // stdin right after writing them, and the engine then inherits an empty one.
  const readEnv = 'eval "$(cat)"';
  // Use an absolute path so we don't rely on the login shell's PATH order.
  // Some hosts have multiple stepyard binaries (e.g. ~/.cargo/bin vs /usr/local/bin)
  // which may be different versions. Override with STEPYARD_HOST_BINARY.
  const stepyardBin = process.env.STEPYARD_HOST_BINARY || '/usr/local/bin/stepyard';
  const remoteCmd = `${prep} && ${readEnv} && exec ${shellQuote(stepyardBin)} ${stepyardArgs.map(shellQuote).join(' ')}`;
  // ssh concatenates remote argv with spaces before feeding it to the remote
  // shell, which then re-parses. Pass the bash invocation as ONE argument so
  // the `&&` / quoting in remoteCmd survives.
  // Unknown and changed host keys are refused. BatchMode has no prompt,
  // so a missing known_hosts entry fails the dispatch instead of accepting
  // whichever key the server offers.
  const sshArgs = [
    '-o', 'StrictHostKeyChecking=yes',
    '-o', 'BatchMode=yes',
    sshHost,
    `bash -lc ${shellQuote(remoteCmd)}`,
  ];
  return { binary: 'ssh', args: sshArgs, stdin: exports.join('\n') + '\n' };
}
