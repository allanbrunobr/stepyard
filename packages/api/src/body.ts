import express, { NextFunction, Request, Response } from 'express';
import { requireAuth } from './auth';

/** Default for STEPYARD_ARTIFACT_MAX_BYTES (decoded artifact size). */
export const DEFAULT_ARTIFACT_MAX_BYTES = 10 * 1024 * 1024;

export function artifactMaxBytes(): number {
  const raw = process.env.STEPYARD_ARTIFACT_MAX_BYTES;
  if (!raw) return DEFAULT_ARTIFACT_MAX_BYTES;
  const parsed = Number.parseInt(raw, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : DEFAULT_ARTIFACT_MAX_BYTES;
}

const ARTIFACT_UPLOAD = /^\/api\/workflows\/[^/]+\/artifacts\/?$/;

/**
 * JSON body parsing for the whole API (SEG-TS-06).
 *
 * Every route keeps the 100 kB limit, except `POST /api/workflows/:id/artifacts`,
 * whose body carries the artifact as base64: the global 100 kB limit made
 * every upload above ~75 kB fail with 413 although the documented limit is
 * STEPYARD_ARTIFACT_MAX_BYTES (10 MiB). That route gets a limit sized for the
 * base64 form of the maximum artifact, and authentication runs BEFORE the
 * large body is read, so an unauthenticated client cannot make the API buffer
 * megabytes.
 */
export function jsonBodyParser(): (req: Request, res: Response, next: NextFunction) => void {
  const small = express.json({ limit: '100kb' });
  const base64Limit = Math.ceil((artifactMaxBytes() * 4) / 3) + 64 * 1024;
  const large = express.json({ limit: base64Limit });
  return (req, res, next) => {
    if (req.method === 'POST' && ARTIFACT_UPLOAD.test(req.path)) {
      requireAuth(req, res, () => large(req, res, next));
      return;
    }
    small(req, res, next);
  };
}
