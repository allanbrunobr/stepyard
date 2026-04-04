import { Express } from 'express';
import overviewRouter from './overview';
import analyticsRouter from './analytics';

export function registerRoutes(app: Express): void {
  app.use('/api/overview', overviewRouter);
  app.use('/api/analytics', analyticsRouter);
}
