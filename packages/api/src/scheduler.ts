import cron from 'node-cron';
import { pool } from './db';
import { logger } from './logger';

const RETENTION_DAYS = 90;

async function cleanupOldRecords(): Promise<void> {
  try {
    const result = await pool.query(
      `DELETE FROM workflow_runs WHERE started_at < NOW() - INTERVAL '${RETENTION_DAYS} days'`
    );
    const deletedCount = result.rowCount ?? 0;
    logger.info({ deletedCount }, `Cleanup complete: deleted ${deletedCount} workflow runs older than ${RETENTION_DAYS} days`);
  } catch (err) {
    logger.error(err, 'Cleanup job failed');
  }
}

export function startCleanupScheduler(): void {
  // Run daily at 03:00 UTC
  cron.schedule('0 3 * * *', () => {
    logger.info('Starting scheduled data retention cleanup');
    cleanupOldRecords();
  }, { timezone: 'UTC' });

  logger.info('Data retention cleanup scheduled for daily at 03:00 UTC');
}
