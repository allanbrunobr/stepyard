import fs from 'fs';
import path from 'path';
import { pool } from './db';
import { logger } from './logger';

const MIGRATIONS_DIR = path.join(__dirname, '..', 'migrations');

async function ensureMigrationsTable(): Promise<void> {
  await pool.query(`
    CREATE TABLE IF NOT EXISTS _migrations (
      id SERIAL PRIMARY KEY,
      name TEXT UNIQUE NOT NULL,
      applied_at TIMESTAMPTZ DEFAULT NOW()
    )
  `);
}

async function getAppliedMigrations(): Promise<Set<string>> {
  const result = await pool.query('SELECT name FROM _migrations ORDER BY id');
  return new Set(result.rows.map((r: { name: string }) => r.name));
}

const LOCK_ID = 123456789;

export async function runMigrations(): Promise<void> {
  const lockClient = await pool.connect();
  try {
    await lockClient.query('SELECT pg_advisory_lock($1)', [LOCK_ID]);
    logger.info('Acquired migration advisory lock');

    await ensureMigrationsTable();
    const applied = await getAppliedMigrations();

    const files = fs.readdirSync(MIGRATIONS_DIR)
      .filter((f) => f.endsWith('.sql'))
      .sort();

    for (const file of files) {
      if (applied.has(file)) {
        logger.info(`Migration ${file} already applied, skipping`);
        continue;
      }

      const sql = fs.readFileSync(path.join(MIGRATIONS_DIR, file), 'utf-8');
      logger.info(`Applying migration: ${file}`);

      const client = await pool.connect();
      try {
        await client.query('BEGIN');
        await client.query(sql);
        await client.query('INSERT INTO _migrations (name) VALUES ($1)', [file]);
        await client.query('COMMIT');
        logger.info(`Migration ${file} applied successfully`);
      } catch (err) {
        await client.query('ROLLBACK');
        logger.error(err, `Migration ${file} failed`);
        throw err;
      } finally {
        client.release();
      }
    }
  } finally {
    await lockClient.query('SELECT pg_advisory_unlock($1)', [LOCK_ID]);
    lockClient.release();
    logger.info('Released migration advisory lock');
  }
}
