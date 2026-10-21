import { Router, Request, Response } from 'express';
import { pool } from '../db/pool';

interface OverviewSummary {
  total_runs: number;
  total_tokens: number;
  total_cost_usd: number;
  active_developers: number;
}

interface DailyUsage {
  date: string;
  count: number;
}

interface PeakHour {
  hour: number;
  count: number;
}

const router = Router();

function parseDateRange(req: Request): { from: Date; to: Date } {
  const now = new Date();
  const toDate = req.query.to ? new Date(req.query.to as string) : now;
  const fromDate = req.query.from
    ? new Date(req.query.from as string)
    : new Date(now.getTime() - 30 * 24 * 60 * 60 * 1000);

  if (isNaN(fromDate.getTime()) || isNaN(toDate.getTime())) {
    throw new Error('INVALID_DATE');
  }
  if (fromDate > toDate) {
    throw new Error('INVALID_DATE_RANGE');
  }

  const maxRange = 90 * 24 * 60 * 60 * 1000;
  if (toDate.getTime() - fromDate.getTime() > maxRange) {
    throw new Error('INVALID_DATE_RANGE');
  }

  return { from: fromDate, to: toDate };
}

// GET /api/overview/summary
router.get('/summary', async (req: Request, res: Response) => {
  try {
    const { from, to } = parseDateRange(req);

    const result = await pool.query(
      `SELECT
        COUNT(*)::int AS total_runs,
        COALESCE(SUM(total_tokens), 0)::bigint AS total_tokens,
        COALESCE(SUM(cost_usd), 0)::numeric AS total_cost_usd,
        COUNT(DISTINCT user_name)::int AS active_developers
      FROM workflow_runs
      WHERE started_at >= $1 AND started_at <= $2`,
      [from.toISOString(), to.toISOString()]
    );

    const row = result.rows[0];
    const summary: OverviewSummary = {
      total_runs: row.total_runs,
      total_tokens: Number(row.total_tokens),
      total_cost_usd: Number(Number(row.total_cost_usd).toFixed(2)),
      active_developers: row.active_developers,
    };

    res.json(summary);
  } catch (err) {
    if (err instanceof Error && (err.message === 'INVALID_DATE' || err.message === 'INVALID_DATE_RANGE')) {
      res.status(400).json({ error: 'INVALID_DATE_RANGE', message: 'Invalid or inverted date range' });
      return;
    }
    console.error('Error fetching summary:', err);
    res.status(500).json({ error: 'INTERNAL_ERROR', message: 'Failed to fetch summary data' });
  }
});

// GET /api/overview/daily-usage
router.get('/daily-usage', async (req: Request, res: Response) => {
  try {
    const { from, to } = parseDateRange(req);

    const result = await pool.query(
      `SELECT DATE(started_at AT TIME ZONE 'UTC') AS date, COUNT(*)::int AS count
      FROM workflow_runs
      WHERE started_at >= $1 AND started_at <= $2
      GROUP BY DATE(started_at AT TIME ZONE 'UTC')
      ORDER BY date`,
      [from.toISOString(), to.toISOString()]
    );

    // Build a map of existing counts
    const countMap = new Map<string, number>();
    for (const row of result.rows) {
      const dateStr = new Date(row.date).toISOString().split('T')[0];
      countMap.set(dateStr, row.count);
    }

    // Fill zero-count days
    const dailyUsage: DailyUsage[] = [];
    const current = new Date(from);
    current.setUTCHours(0, 0, 0, 0);
    const end = new Date(to);
    end.setUTCHours(23, 59, 59, 999);

    while (current <= end) {
      const dateStr = current.toISOString().split('T')[0];
      dailyUsage.push({
        date: dateStr,
        count: countMap.get(dateStr) || 0,
      });
      current.setUTCDate(current.getUTCDate() + 1);
    }

    res.json(dailyUsage);
  } catch (err) {
    if (err instanceof Error && (err.message === 'INVALID_DATE' || err.message === 'INVALID_DATE_RANGE')) {
      res.status(400).json({ error: 'INVALID_DATE_RANGE', message: 'Invalid or inverted date range' });
      return;
    }
    console.error('Error fetching daily usage:', err);
    res.status(500).json({ error: 'INTERNAL_ERROR', message: 'Failed to fetch daily usage data' });
  }
});

// GET /api/overview/peak-hours
router.get('/peak-hours', async (req: Request, res: Response) => {
  try {
    const { from, to } = parseDateRange(req);

    const result = await pool.query(
      `SELECT EXTRACT(HOUR FROM started_at AT TIME ZONE 'UTC')::int AS hour, COUNT(*)::int AS count
      FROM workflow_runs
      WHERE started_at >= $1 AND started_at <= $2
      GROUP BY EXTRACT(HOUR FROM started_at AT TIME ZONE 'UTC')
      ORDER BY hour`,
      [from.toISOString(), to.toISOString()]
    );

    // Build map and ensure all 24 hours are represented
    const countMap = new Map<number, number>();
    for (const row of result.rows) {
      countMap.set(row.hour, row.count);
    }

    const peakHours: PeakHour[] = [];
    for (let h = 0; h < 24; h++) {
      peakHours.push({
        hour: h,
        count: countMap.get(h) || 0,
      });
    }

    res.json(peakHours);
  } catch (err) {
    if (err instanceof Error && (err.message === 'INVALID_DATE' || err.message === 'INVALID_DATE_RANGE')) {
      res.status(400).json({ error: 'INVALID_DATE_RANGE', message: 'Invalid or inverted date range' });
      return;
    }
    console.error('Error fetching peak hours:', err);
    res.status(500).json({ error: 'INTERNAL_ERROR', message: 'Failed to fetch peak hours data' });
  }
});

export default router;
