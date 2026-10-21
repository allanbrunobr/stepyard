import express from 'express';
import cors from 'cors';
import dotenv from 'dotenv';
import { logger } from './logger';
import { runMigrations } from './migrations';
import { requireDashboardAuth } from './auth';
import { healthRouter } from './routes/health';
import { authRouter } from './routes/auth';
import { eventsRouter } from './routes/events';
import { workflowsRouter } from './routes/workflows';
import { startCleanupScheduler } from './scheduler';
import analyticsRouter from './routes/analytics';
import overviewRouter from './routes/overview';

dotenv.config({ path: '../../.env' });

const app = express();
const port = parseInt(process.env.API_PORT || process.env.PORT || '3001', 10);

app.use(cors({
  origin: process.env.CORS_ORIGIN || 'http://localhost:5173',
  credentials: true,
}));
app.use(express.json());

app.use('/api', healthRouter);
app.use('/api', authRouter);
app.use('/api', eventsRouter);
app.use('/api', workflowsRouter);
app.use('/api/overview', requireDashboardAuth, overviewRouter);
app.use('/api/analytics', requireDashboardAuth, analyticsRouter);

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

export default app;
