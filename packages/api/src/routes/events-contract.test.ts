import { readFileSync } from 'fs';
import { join } from 'path';
import { describe, expect, it } from 'vitest';
import { EventSchema } from './events';

// ARQ-03 (analysis 2026-11-09): the Rust DashboardSubscriber and this API
// each typed the event payload by hand. `tests/fixtures/dashboard-event.json`
// is the payload the Rust side actually serializes (a Rust golden test keeps
// it in sync); this side must accept it.

describe('dashboard event contract', () => {
  it('accepts the payload the Rust dashboard subscriber sends', () => {
    const fixture = JSON.parse(
      readFileSync(join(__dirname, '../../../../tests/fixtures/dashboard-event.json'), 'utf8'),
    );
    const parsed = EventSchema.safeParse(fixture);
    expect(parsed.success, JSON.stringify(parsed.success ? null : parsed.error.issues)).toBe(true);
  });

  it('accepts null for optional step fields (older CLIs in the field send them)', () => {
    const parsed = EventSchema.safeParse({
      run_id: '6f1c2f43-2f43-4c2f-8f43-2f432f432f43',
      user_name: 'u',
      workflow: 'w',
      status: 'success',
      started_at: '2026-11-10T00:00:00Z',
      steps: [
        { step_name: 'lint', step_type: 'cmd', status: 'completed', duration_ms: 5, tokens_in: null, tokens_out: null, sandboxed: false, error: null },
      ],
    });
    expect(parsed.success, JSON.stringify(parsed.success ? null : parsed.error.issues)).toBe(true);
  });
});
