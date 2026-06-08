import { describe, expect, it } from 'vitest';
import { secretMatches } from './auth';

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