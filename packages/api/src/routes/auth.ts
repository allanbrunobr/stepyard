import { Router, Request, Response } from 'express';
import { z } from 'zod';
import {
  clearSessionCookieHeader,
  createSessionToken,
  isDashboardAuthenticated,
  secretMatches,
  sessionCookieHeader,
} from '../auth';

export const authRouter = Router();

const loginSchema = z.object({
  secret: z.string().min(1),
});

authRouter.get('/auth/session', (req: Request, res: Response) => {
  res.json({ authenticated: isDashboardAuthenticated(req) });
});

authRouter.post('/auth/login', (req: Request, res: Response) => {
  const parsed = loginSchema.safeParse(req.body);
  if (!parsed.success) {
    res.status(400).json({ error: { code: 'VALIDATION_FAILED', message: 'Invalid login payload' } });
    return;
  }

  const secret = process.env.API_SECRET;
  if (!secret || !secretMatches(parsed.data.secret, secret)) {
    res.status(401).json({ error: { code: 'UNAUTHORIZED', message: 'Invalid secret' } });
    return;
  }

  res.setHeader('Set-Cookie', sessionCookieHeader(createSessionToken(secret)));
  res.json({ authenticated: true });
});

authRouter.post('/auth/logout', (_req: Request, res: Response) => {
  res.setHeader('Set-Cookie', clearSessionCookieHeader());
  res.json({ authenticated: false });
});
