import { Router, Request, Response } from "express";
import { z } from "zod";
import pool from "../db";

const router = Router();

function isValidDate(value: string): boolean {
  const d = new Date(value + "T00:00:00Z");
  return !isNaN(d.getTime()) && d.toISOString().startsWith(value);
}

const dateRangeSchema = z.object({
  from: z
    .string()
    .regex(/^\d{4}-\d{2}-\d{2}$/, "from must be YYYY-MM-DD")
    .refine(isValidDate, "from is not a valid calendar date"),
  to: z
    .string()
    .regex(/^\d{4}-\d{2}-\d{2}$/, "to must be YYYY-MM-DD")
    .refine(isValidDate, "to is not a valid calendar date"),
});

function parseDateRange(query: Record<string, unknown>) {
  return dateRangeSchema.parse(query);
}

// GET /api/analytics/developers?from=&to=
router.get("/developers", async (req: Request, res: Response) => {
  try {
    const { from, to } = parseDateRange(req.query);

    const result = await pool.query(
      `SELECT
        user_name,
        COUNT(*)::int AS workflow_count,
        COALESCE(SUM(total_tokens), 0)::bigint AS total_tokens,
        COALESCE(SUM(cost_usd), 0)::float AS total_cost_usd
      FROM workflow_runs
      WHERE started_at >= $1 AND started_at < ($2::date + INTERVAL '1 day')
      GROUP BY user_name
      ORDER BY workflow_count DESC`,
      [from, to]
    );

    res.json(result.rows);
  } catch (err) {
    if (err instanceof z.ZodError) {
      res.status(400).json({ error: err.errors });
      return;
    }
    res.status(500).json({ error: "Internal server error" });
  }
});

// GET /api/analytics/costs/by-developer?from=&to=
router.get("/costs/by-developer", async (req: Request, res: Response) => {
  try {
    const { from, to } = parseDateRange(req.query);

    const result = await pool.query(
      `SELECT
        user_name,
        COALESCE(SUM(cost_usd), 0)::float AS cost_usd,
        COALESCE(SUM(total_tokens), 0)::bigint AS total_tokens,
        COUNT(*)::int AS run_count
      FROM workflow_runs
      WHERE started_at >= $1 AND started_at < ($2::date + INTERVAL '1 day')
      GROUP BY user_name
      ORDER BY cost_usd DESC`,
      [from, to]
    );

    res.json(result.rows);
  } catch (err) {
    if (err instanceof z.ZodError) {
      res.status(400).json({ error: err.errors });
      return;
    }
    res.status(500).json({ error: "Internal server error" });
  }
});

// GET /api/analytics/costs/by-workflow?from=&to=
router.get("/costs/by-workflow", async (req: Request, res: Response) => {
  try {
    const { from, to } = parseDateRange(req.query);

    const result = await pool.query(
      `SELECT
        workflow,
        COALESCE(SUM(cost_usd), 0)::float AS cost_usd,
        COALESCE(SUM(total_tokens), 0)::bigint AS total_tokens,
        COUNT(*)::int AS run_count
      FROM workflow_runs
      WHERE started_at >= $1 AND started_at < ($2::date + INTERVAL '1 day')
      GROUP BY workflow
      ORDER BY cost_usd DESC`,
      [from, to]
    );

    res.json(result.rows);
  } catch (err) {
    if (err instanceof z.ZodError) {
      res.status(400).json({ error: err.errors });
      return;
    }
    res.status(500).json({ error: "Internal server error" });
  }
});

// GET /api/analytics/costs/by-repo?from=&to=
router.get("/costs/by-repo", async (req: Request, res: Response) => {
  try {
    const { from, to } = parseDateRange(req.query);

    const result = await pool.query(
      `SELECT
        repo,
        COALESCE(SUM(cost_usd), 0)::float AS cost_usd,
        COALESCE(SUM(total_tokens), 0)::bigint AS total_tokens,
        COUNT(*)::int AS run_count
      FROM workflow_runs
      WHERE started_at >= $1 AND started_at < ($2::date + INTERVAL '1 day')
      GROUP BY repo
      ORDER BY cost_usd DESC`,
      [from, to]
    );

    res.json(result.rows);
  } catch (err) {
    if (err instanceof z.ZodError) {
      res.status(400).json({ error: err.errors });
      return;
    }
    res.status(500).json({ error: "Internal server error" });
  }
});

// GET /api/analytics/costs/daily?from=&to=
router.get("/costs/daily", async (req: Request, res: Response) => {
  try {
    const { from, to } = parseDateRange(req.query);

    const result = await pool.query(
      `SELECT
        d::date AS date,
        COALESCE(SUM(wr.cost_usd), 0)::float AS cost_usd
      FROM generate_series($1::date, $2::date, '1 day'::interval) AS d
      LEFT JOIN workflow_runs wr
        ON DATE(wr.started_at) = d::date
      GROUP BY d::date
      ORDER BY d::date ASC`,
      [from, to]
    );

    res.json(
      result.rows.map((row) => ({
        date: (row.date as Date).toISOString().split("T")[0],
        cost_usd: row.cost_usd,
      }))
    );
  } catch (err) {
    if (err instanceof z.ZodError) {
      res.status(400).json({ error: err.errors });
      return;
    }
    res.status(500).json({ error: "Internal server error" });
  }
});

export default router;
