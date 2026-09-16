import prisma from "../lib/prisma";
import { terminateExpiredRooms } from "../signaling/server";

const INTERVAL_MS = 60 * 1000; // run every 60 seconds

export function startExpiryJob() {
  async function expire() {
    try {
      // Find tokens of rooms about to be expired (so we can notify WS clients)
      const toExpire = await prisma.room.findMany({
        where: {
          status: { in: ["WAITING", "CONNECTING", "ACTIVE"] },
          expiresAt: { lt: new Date() },
        },
        select: { token: true },
      });

      if (toExpire.length === 0) return;

      const tokens = toExpire.map((r) => r.token);

      // Mark as EXPIRED in DB
      await prisma.room.updateMany({
        where: { token: { in: tokens } },
        data: { status: "EXPIRED" },
      });

      // Close any live WebSocket connections for these rooms
      terminateExpiredRooms(tokens);

      console.log(`[expiry] Expired ${tokens.length} room(s) and closed their connections`);
    } catch (err) {
      console.error("[expiry] Failed to expire rooms:", err);
    }
  }

  // Run immediately on startup, then on interval
  void expire();
  const handle = setInterval(() => void expire(), INTERVAL_MS);

  return () => clearInterval(handle);
}
