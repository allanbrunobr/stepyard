import { Pool } from 'pg';
import { logger } from './logger';

export const pool = new Pool({
  user: process.env.POSTGRES_USER || 'stepyard',
  // No default password: set POSTGRES_PASSWORD (see .env.example).
  password: process.env.POSTGRES_PASSWORD,
  host: process.env.POSTGRES_HOST || 'db',
  port: parseInt(process.env.POSTGRES_PORT || '5432', 10),
  database: process.env.POSTGRES_DB || 'stepyard',
  // Fail a request fast when Postgres is down instead of hanging it.
  connectionTimeoutMillis: parseInt(process.env.POSTGRES_CONNECT_TIMEOUT_MS || '5000', 10),
});

// An idle client losing its connection (Postgres restart, network blip) makes
// the Pool emit 'error'. Without a listener, EventEmitter throws and the whole
// API process exits. The pool discards the broken client by itself.
pool.on('error', (err) => {
  logger.error({ err }, 'Idle Postgres client error; client discarded');
});
