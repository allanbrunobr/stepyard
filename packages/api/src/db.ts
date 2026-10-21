import { Pool } from 'pg';

export const pool = new Pool({
  user: process.env.POSTGRES_USER || 'minion',
  // No default password: set POSTGRES_PASSWORD (see .env.example).
  password: process.env.POSTGRES_PASSWORD,
  host: process.env.POSTGRES_HOST || 'db',
  port: parseInt(process.env.POSTGRES_PORT || '5432', 10),
  database: process.env.POSTGRES_DB || 'minion_engine',
});
