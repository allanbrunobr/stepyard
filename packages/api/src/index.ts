import express from 'express';
import dotenv from 'dotenv';
import { logger } from './logger';
import { runMigrations } from './migrations';
import { healthRouter } from './routes/health';
import { eventsRouter } from './routes/events';
import { startCleanupScheduler } from './scheduler';

dotenv.config({ path: '../../.env' });

const app = express();
const port = parseInt(process.env.API_PORT || '3001', 10);

app.use(express.json());

app.use('/api', healthRouter);
app.use('/api', eventsRouter);

async function start(): Promise<void> {
  await runMigrations();
  startCleanupScheduler();

  app.listen(port, '0.0.0.0', () => {
    logger.info(`API server listening on port ${port}`);
  });
}

start().catch((err) => {
  logger.error(err, 'Failed to start server');
  process.exit(1);
});
