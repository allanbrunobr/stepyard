import { Router } from "express";

const router = Router();

// Route registrations below — append only
import analyticsRouter from "./analytics";
router.use("/analytics", analyticsRouter);

export default router;
