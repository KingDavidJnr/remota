import "dotenv/config";
import http from "http";
import express from "express";
import cors from "cors";
import roomsRouter from "./routes/rooms";
import { createSignalingServer } from "./signaling/server";
import { startExpiryJob } from "./jobs/expiry";

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

app.get("/health", (_req, res) => {
  res.json({ ok: true });
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
