import { createHmac, timingSafeEqual } from 'crypto';
import type { NextFunction, Request, RequestHandler, Response } from 'express';

const COOKIE_NAME = 'stepyard_dashboard_session';
const COOKIE_MAX_AGE_SECONDS = 7 * 24 * 60 * 60;

function apiSecret(): string | null {
  const secret = process.env.API_SECRET;
  return secret && secret.length > 0 ? secret : null;
}

function sign(value: string, secret: string): string {
  return createHmac('sha256', secret).update(value).digest('base64url');
}

function parseCookies(header: string | undefined): Record<string, string> {
  if (!header) return {};
  const cookies: Record<string, string> = {};
  for (const part of header.split(';')) {
    const [rawName, ...rawValue] = part.trim().split('=');
    if (!rawName || rawValue.length === 0) continue;
    try {
      cookies[rawName] = decodeURIComponent(rawValue.join('='));
    } catch {
      continue;
    }
  }
  return cookies;
}

function bearerMatches(req: Request, secret: string): boolean {
  const authHeader = req.headers.authorization;
  return !!authHeader && authHeader.startsWith('Bearer ') && secretMatches(authHeader.slice(7), secret);
}

export function secretMatches(candidate: string, secret: string): boolean {
  const actualBuffer = Buffer.from(candidate);
  const expectedBuffer = Buffer.from(secret);
  return actualBuffer.length === expectedBuffer.length && timingSafeEqual(actualBuffer, expectedBuffer);
}

function validSessionCookie(req: Request, secret: string): boolean {
  const token = parseCookies(req.headers.cookie)[COOKIE_NAME];
  if (!token) return false;

  const parts = token.split('.');
  if (parts.length !== 3) return false;
  const [version, issuedAt, signature] = parts;
  if (version !== 'v1' || !issuedAt || !signature) return false;

  const issuedAtMs = Number.parseInt(issuedAt, 10);
  if (!Number.isFinite(issuedAtMs)) return false;
  const now = Date.now();
  if (issuedAtMs > now) return false;
  if (now - issuedAtMs > COOKIE_MAX_AGE_SECONDS * 1000) return false;

  const expected = sign(issuedAt, secret);
  const actualBuffer = Buffer.from(signature);
  const expectedBuffer = Buffer.from(expected);
  return actualBuffer.length === expectedBuffer.length && timingSafeEqual(actualBuffer, expectedBuffer);
}

export function createSessionToken(secret: string): string {
  const issuedAt = String(Date.now());
  return `v1.${issuedAt}.${sign(issuedAt, secret)}`;
}

/**
 * `Secure` is always set when the host is not loopback. The env flag can
 * turn it on for localhost. It cannot turn it off for any other host.
 */
export function cookieSecureForHost(host: string | undefined): boolean {
  const name = (host ?? '').replace(/^\[|\]$/g, '').split('%')[0];
  const loopback = name === 'localhost' || name === '127.0.0.1' || name === '::1';
  if (!loopback) return true;
  const flag = process.env.DASHBOARD_COOKIE_SECURE?.trim().toLowerCase();
  return flag === '1' || flag === 'true' || flag === 'yes';
}

export function sessionCookieHeader(token: string, host?: string): string {
  const secureAttr = cookieSecureForHost(host) ? '; Secure' : '';
  return `${COOKIE_NAME}=${encodeURIComponent(token)}; HttpOnly; SameSite=Lax; Path=/api; Max-Age=${COOKIE_MAX_AGE_SECONDS}${secureAttr}`;
}

export function clearSessionCookieHeader(host?: string): string {
  const secureAttr = cookieSecureForHost(host) ? '; Secure' : '';
  return `${COOKIE_NAME}=; HttpOnly; SameSite=Lax; Path=/api; Max-Age=0${secureAttr}`;
}

export function isDashboardAuthenticated(req: Request): boolean {
  const secret = apiSecret();
  if (!secret) return false;
  return bearerMatches(req, secret) || validSessionCookie(req, secret);
}

export function requireAuth(req: Request, res: Response, next: NextFunction) {
  const secret = apiSecret();
  if (!secret || !bearerMatches(req, secret)) {
    res.status(401).json({ error: { code: 'UNAUTHORIZED', message: 'Invalid or missing authorization' } });
    return;
  }
  next();
}

export const requireDashboardAuth: RequestHandler = (req, res, next) => {
  if (!isDashboardAuthenticated(req)) {
    res.status(401).json({ error: { code: 'UNAUTHORIZED', message: 'Dashboard login required' } });
    return;
  }
  next();
};
