// Provided by wt1 — Express app bootstrap
import express from 'express';
import cors from 'cors';
import { registerRoutes } from './routes';

const app = express();
const PORT = process.env.PORT || 3001;

app.use(cors());
app.use(express.json());

// Health check (wt1)
app.get('/api/health', (_req, res) => {
  res.json({ status: 'ok' });
});

// Register all route modules
registerRoutes(app);

app.listen(PORT, () => {
  console.log(`API server running on port ${PORT}`);
});

export default app;
