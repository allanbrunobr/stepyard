import express from "express";
import cors from "cors";
import pino from "pino";
import router from "./routes";

const app = express();
const logger = pino();
const PORT = process.env.PORT || 3001;

app.use(cors());
app.use(express.json());
app.use("/api", router);

app.listen(PORT, () => {
  logger.info(`API server listening on port ${PORT}`);
});

export default app;
