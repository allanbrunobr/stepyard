// Provided by wt1 — database connection pool
import { Pool } from 'pg';

export const pool = new Pool({
  connectionString: process.env.DATABASE_URL || 'postgresql://localhost:5432/minion_dashboard',
});
