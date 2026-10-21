import { Pool } from 'pg';

export const pool = new Pool({
  user: process.env.POSTGRES_USER || 'minion',
  password: process.env.POSTGRES_PASSWORD || 'minion_secret',
  host: process.env.POSTGRES_HOST || 'db',
  port: parseInt(process.env.POSTGRES_PORT || '5432', 10),
  database: process.env.POSTGRES_DB || 'minion_engine',
});
