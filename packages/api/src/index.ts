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
import { errorHandler } from './errors';
import analyticsRouter from './routes/analytics';
import overviewRouter from './routes/overview';

dotenv.config({ path: '../../.env' });

const app = express();
const port = parseInt(process.env.API_PORT || process.env.PORT || '3001', 10);
// Loopback unless the operator opts in. Compose sets API_HOST=0.0.0.0
// because the web container reaches this process by the service name.
const host = process.env.API_HOST?.trim() || '127.0.0.1';

app.use(cors({
  origin: process.env.CORS_ORIGIN || 'http://localhost:5173',
  credentials: true,
}));
app.use(express.json({ limit: '100kb' }));

app.use('/api', healthRouter);
app.use('/api', authRouter);
app.use('/api', eventsRouter);
app.use('/api', workflowsRouter);
app.use('/api/overview', requireDashboardAuth, overviewRouter);
app.use('/api/analytics', requireDashboardAuth, analyticsRouter);
// Must be registered after every router.
app.use(errorHandler);

// Last line of defence: route handlers forward their errors, but a stray
// rejection elsewhere (a timer, a listener) must be logged, not end the API.
process.on('unhandledRejection', (reason) => {
  logger.error({ err: reason }, 'Unhandled promise rejection');
});

async function start(): Promise<void> {
  await runMigrations();
  startCleanupScheduler();

  app.listen(port, host, () => {
    logger.info(`API server listening on ${host}:${port}`);
  });
}

start().catch((err) => {
  logger.error(err, 'Failed to start server');
  process.exit(1);
});

export default app;
