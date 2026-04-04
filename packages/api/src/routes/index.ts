import { Express } from 'express';
import overviewRouter from './overview';

export function registerRoutes(app: Express): void {
  app.use('/api/overview', overviewRouter);
}
