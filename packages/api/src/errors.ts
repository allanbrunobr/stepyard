import type { NextFunction, Request, Response, Router } from 'express';
import { logger } from './logger';

type Handler = (req: Request, res: Response, next: NextFunction) => unknown;

/**
 * Express 4 does not forward a rejected promise from an async handler to the
 * error middleware: the rejection becomes an `unhandledRejection`, which ends
 * a Node >= 15 process (and `tsx watch` does not restart it). Wrapping every
 * handler registered on the router turns a rejection — or a synchronous throw
 * — into `next(err)`, so one failing request (Postgres restarting, an invalid
 * header value) answers 5xx instead of taking the API down.
 *
 * Error middleware (arity 4) and non-function arguments (paths) are left as is.
 */
export function forwardAsyncErrors<R extends Router>(router: R): R {
  for (const method of ['get', 'post', 'put', 'patch', 'delete', 'all', 'use'] as const) {
    const original = (router[method] as (...args: unknown[]) => unknown).bind(router);
    (router as unknown as Record<string, unknown>)[method] = (...args: unknown[]) =>
      original(...args.map(wrapHandler));
  }
  return router;
}

function wrapHandler(arg: unknown): unknown {
  if (typeof arg !== 'function' || arg.length === 4) {
    return arg;
  }
  const handler = arg as Handler;
  return function forwardingHandler(req: Request, res: Response, next: NextFunction): void {
    try {
      const result = handler(req, res, next);
      if (result && typeof (result as Promise<unknown>).then === 'function') {
        (result as Promise<unknown>).catch(next);
      }
    } catch (err) {
      next(err);
    }
  };
}

const DB_UNAVAILABLE_CODES = new Set([
  'ECONNREFUSED',
  'ECONNRESET',
  'ENOTFOUND',
  'ETIMEDOUT',
  'EAI_AGAIN',
  '57P01', // admin_shutdown
  '57P02', // crash_shutdown
  '57P03', // cannot_connect_now
  '08000',
  '08001',
  '08003',
  '08006',
]);

/** True for errors that mean "the database is unreachable right now". */
export function isDatabaseUnavailable(err: unknown): boolean {
  if (!err || typeof err !== 'object') return false;
  const e = err as { code?: unknown; message?: unknown };
  if (typeof e.code === 'string' && DB_UNAVAILABLE_CODES.has(e.code)) return true;
  return typeof e.message === 'string' && /timeout exceeded when trying to connect|Connection terminated/i.test(e.message);
}

/**
 * Last error middleware. Logs the error server-side and answers JSON without
 * the message or stack (they can carry SQL, paths or connection strings).
 * Client errors raised by body-parser (malformed JSON, body too large) keep
 * their 4xx status.
 */
export function errorHandler(err: unknown, req: Request, res: Response, _next: NextFunction): void {
  const status = (err as { status?: unknown; statusCode?: unknown })?.status ??
    (err as { statusCode?: unknown })?.statusCode;
  const clientStatus = typeof status === 'number' && status >= 400 && status < 500 ? status : null;

  if (clientStatus === null) {
    logger.error({ err, method: req.method, path: req.path }, 'Unhandled route error');
  }
  if (res.headersSent) {
    // Streaming responses (CSV export, SSE) already sent a status line.
    res.end();
    return;
  }
  if (clientStatus !== null) {
    res.status(clientStatus).json({ error: { code: 'BAD_REQUEST', message: 'Invalid request' } });
    return;
  }
  if (isDatabaseUnavailable(err)) {
    res.status(503).json({ error: { code: 'DATABASE_UNAVAILABLE', message: 'Database temporarily unavailable' } });
    return;
  }
  res.status(500).json({ error: { code: 'INTERNAL_ERROR', message: 'Internal server error' } });
}
