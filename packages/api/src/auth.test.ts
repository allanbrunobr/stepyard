import { describe, expect, it } from 'vitest';
import { cookieSecureForHost, secretMatches, sessionCookieHeader } from './auth';

describe('secretMatches', () => {
  it('accepts an exact bearer secret', () => {
    expect(secretMatches('my-secret', 'my-secret')).toBe(true);
  });

  it('rejects a wrong secret', () => {
    expect(secretMatches('wrong', 'my-secret')).toBe(false);
  });

  it('rejects different-length secrets without throwing', () => {
    expect(secretMatches('short', 'much-longer-secret')).toBe(false);
  });
});

describe('session cookie', () => {
  it('sets Secure for a host that is not loopback', () => {
    expect(cookieSecureForHost('dashboard.example')).toBe(true);
    expect(sessionCookieHeader('tok', 'dashboard.example')).toContain('; Secure');
  });

  it('leaves Secure off for localhost unless the operator asks', () => {
    delete process.env.DASHBOARD_COOKIE_SECURE;
    expect(cookieSecureForHost('localhost')).toBe(false);
    expect(cookieSecureForHost('127.0.0.1')).toBe(false);
    expect(sessionCookieHeader('tok', 'localhost')).not.toContain('; Secure');
  });
});