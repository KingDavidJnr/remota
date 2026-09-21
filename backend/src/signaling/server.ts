import { WebSocketServer, WebSocket } from "ws";
import { IncomingMessage } from "http";
import { Server } from "http";
import prisma from "../lib/prisma";

// ─── Types ────────────────────────────────────────────────────────────────────

type Role = "controller" | "participant";

interface RoomClient {
  ws: WebSocket;
  role: Role;
  token: string;
}

type SignalType =
  | "join"
  | "offer"
  | "answer"
  | "ice_candidate"
  | "participant_joined"
  | "participant_left"
  | "active"
  | "terminate"
  | "error";

interface SignalMessage {
  type: SignalType;
  [key: string]: unknown;
}

// ─── Room Registry ────────────────────────────────────────────────────────────

// Map<roomToken, Map<clientId, RoomClient>>
const rooms = new Map<string, Map<string, RoomClient>>();

let _clientId = 0;
function nextClientId() {
  return String(++_clientId);
}

function send(ws: WebSocket, msg: SignalMessage) {
  if (ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify(msg));
  }
}

function broadcastToRoom(
  token: string,
  msg: SignalMessage,
  exclude?: WebSocket
) {
  const room = rooms.get(token);
  if (!room) return;
  for (const client of room.values()) {
    if (client.ws !== exclude) {
      send(client.ws, msg);
    }
  }
}

function getClientByRole(
  token: string,
  role: Role
): RoomClient | undefined {
  const room = rooms.get(token);
  if (!room) return undefined;
  for (const client of room.values()) {
    if (client.role === role) return client;
  }
  return undefined;
}

// ─── Cleanup ──────────────────────────────────────────────────────────────────

export async function terminateRoom(token: string, initiator?: WebSocket) {
  broadcastToRoom(token, { type: "terminate" }, initiator);

  const room = rooms.get(token);
  if (room) {
    for (const client of room.values()) {
      client.ws.close();
    }
    rooms.delete(token);
  }

  try {
    await prisma.room.update({
      where: { token },
      data: { status: "ENDED" },
    });
  } catch {
    // Room may not exist — not fatal
  }
}

/**
 * Called by the expiry job to close WebSocket clients for rooms that have
 * just been marked EXPIRED in the database.
 */
export function terminateExpiredRooms(tokens: string[]) {
  for (const token of tokens) {
    const room = rooms.get(token);
    if (!room) continue;
    broadcastToRoom(token, { type: "terminate" });
    for (const client of room.values()) {
      client.ws.close();
    }
    rooms.delete(token);
  }
}

// ─── WebSocket Server Factory ─────────────────────────────────────────────────

export function createSignalingServer(httpServer: Server) {
  const wss = new WebSocketServer({ server: httpServer, path: "/ws" });

  wss.on("connection", (ws: WebSocket, _req: IncomingMessage) => {
    const clientId = nextClientId();
    let assignedToken: string | null = null;

    ws.on("message", async (raw) => {
      let msg: SignalMessage;

      try {
        msg = JSON.parse(raw.toString()) as SignalMessage;
      } catch {
        send(ws, { type: "error", message: "Invalid JSON" });
        return;
      }

      // ── join ───────────────────────────────────────────────────────────────
      if (msg.type === "join") {
        const token = msg.token as string;
        const role = msg.role as Role;

        if (!token || !["controller", "participant"].includes(role)) {
          send(ws, { type: "error", message: "Invalid join payload" });
          return;
        }

        // Validate room in DB
        let room;
        try {
          room = await prisma.room.findUnique({ where: { token } });
        } catch {
          send(ws, { type: "error", message: "DB error" });
          return;
        }

        if (!room) {
          send(ws, { type: "error", message: "Room not found" });
          return;
        }

        if (
          room.status === "EXPIRED" ||
          room.status === "ENDED" ||
          room.expiresAt < new Date()
        ) {
          send(ws, { type: "error", message: "Room is no longer active" });
          return;
        }

        // Prevent duplicate roles
        const existing = getClientByRole(token, role);
        if (existing) {
          send(ws, {
            type: "error",
            message: `A ${role} is already connected`,
          });
          return;
        }

        // Register client
        if (!rooms.has(token)) {
          rooms.set(token, new Map());
        }
        rooms.get(token)!.set(clientId, { ws, role, token });
        assignedToken = token;

        // Update room status when participant joins
        if (role === "participant") {
          try {
            await prisma.room.update({
              where: { token },
              data: { status: "CONNECTING" },
            });
          } catch { /* non-fatal */ }

          // Notify controller that participant joined
          const controller = getClientByRole(token, "controller");
          if (controller) {
            send(controller.ws, { type: "participant_joined" });
          }
        }

        if (role === "controller") {
          // Check if participant already present
          const participant = getClientByRole(token, "participant");
          if (participant) {
            send(ws, { type: "participant_joined" });
          }
        }

        return;
      }

      // All further messages require the client to have joined
      if (!assignedToken) {
        send(ws, { type: "error", message: "Not joined to any room" });
        return;
      }

      const token = assignedToken;

      // ── offer (controller → participant) ───────────────────────────────────
      if (msg.type === "offer") {
        const participant = getClientByRole(token, "participant");
        if (participant) {
          send(participant.ws, { type: "offer", sdp: msg.sdp });
        }
        return;
      }

      // ── answer (participant → controller) ──────────────────────────────────
      if (msg.type === "answer") {
        const controller = getClientByRole(token, "controller");
        if (controller) {
          send(controller.ws, { type: "answer", sdp: msg.sdp });
        }
        return;
      }

      // ── ice_candidate (relayed to the other side) ──────────────────────────
      if (msg.type === "ice_candidate") {
        broadcastToRoom(token, { type: "ice_candidate", candidate: msg.candidate }, ws);
        return;
      }

      // ── active (participant desktop signals WebRTC is connected) ───────────
      if (msg.type === "active") {
        try {
          await prisma.room.update({
            where: { token },
            data: { status: "ACTIVE" },
          });
        } catch { /* non-fatal */ }
        // Forward to controller so it can update its UI state if needed
        const controller = getClientByRole(token, "controller");
        if (controller) {
          send(controller.ws, { type: "active" });
        }
        return;
      }

      // ── terminate ──────────────────────────────────────────────────────────
      if (msg.type === "terminate") {
        await terminateRoom(token, ws);
        return;
      }
    });

    ws.on("close", async () => {
      if (!assignedToken) return;

      const token = assignedToken;
      const room = rooms.get(token);
      if (!room) return;

      const leaving = room.get(clientId);
      room.delete(clientId);

      if (leaving?.role === "participant") {
        const controller = getClientByRole(token, "controller");
        if (controller) {
          send(controller.ws, { type: "participant_left" });
        }
        // Participant leaving = connection over
        await terminateRoom(token);
      } else if (leaving?.role === "controller") {
        // Controller leaving = also end
        await terminateRoom(token);
      }
    });

    ws.on("error", (err) => {
      console.error(`[WS] client ${clientId} error:`, (err as Error).message);
    });
  });

  console.log("[WS] Signaling server attached at /ws");
  return wss;
}
