import "dotenv/config";
import http from "http";
import express from "express";
import cors from "cors";
import roomsRouter from "./routes/rooms";
import { createSignalingServer } from "./signaling/server";
import { startExpiryJob } from "./jobs/expiry";
import prisma from "./lib/prisma";

const app = express();
const PORT = process.env.PORT ?? 4000;

// ─── Middleware ───────────────────────────────────────────────────────────────

app.use(
  cors({
    origin: process.env.CORS_ORIGIN ?? "*",
    methods: ["GET", "POST", "OPTIONS"],
  })
);
app.use(express.json());

// ─── Routes ───────────────────────────────────────────────────────────────────

app.get("/health", async (_req, res) => {
  const start = Date.now();
  let dbOk = false;
  let dbLatencyMs: number | null = null;
  let dbError: string | null = null;

  try {
    await prisma.$queryRaw`SELECT 1`;
    dbLatencyMs = Date.now() - start;
    dbOk = true;
  } catch (err) {
    dbLatencyMs = Date.now() - start;
    dbError = err instanceof Error ? err.message : "unknown error";
  }

  const status = dbOk ? 200 : 503;

  res.status(status).json({
    ok: dbOk,
    timestamp: new Date().toISOString(),
    database: {
      ok: dbOk,
      latencyMs: dbLatencyMs,
      ...(dbError ? { error: dbError } : {}),
    },
  });
});

app.use("/rooms", roomsRouter);

// ─── HTTP + WebSocket Server ──────────────────────────────────────────────────

const httpServer = http.createServer(app);
createSignalingServer(httpServer);

// ─── Background Jobs ──────────────────────────────────────────────────────────

startExpiryJob();

// ─── Start ────────────────────────────────────────────────────────────────────

httpServer.listen(PORT, () => {
  console.log(`[server] Listening on port ${PORT}`);
});

// Graceful shutdown
process.on("SIGTERM", () => {
  console.log("[server] SIGTERM received, shutting down");
  httpServer.close(() => process.exit(0));
});
