import { WebSocketServer, WebSocket } from "ws";
import { IncomingMessage } from "http";
import { Server } from "http";
import prisma from "../lib/prisma";

// ─── Contract ─────────────────────────────────────────────────────────────────
//
// The server is the ONLY entity that terminates a session.
// Clients send a "terminate" REQUEST. The server broadcasts "terminate" to ALL
// parties and then closes all connections. Clients act only on receiving
// "terminate" from the server — never on WebSocket close or WebRTC state.
//
// WS close (tab close, network drop) → 10s grace period → server terminates
// Explicit terminate message from any client → server terminates immediately
//
// ─────────────────────────────────────────────────────────────────────────────

type Role = "controller" | "participant" | "observer";

interface RoomClient {
  ws: WebSocket & { isAlive?: boolean };
  role: Role;
  token: string;
}

type SignalType =
  | "join"
  | "offer"
  | "answer"
  | "ice_candidate"
  | "participant_joined"
  | "active"
  | "terminate"
  | "ping"
  | "pong"
  | "error";

interface SignalMessage {
  type: SignalType;
  [key: string]: unknown;
}

// ─── Room Registry ────────────────────────────────────────────────────────────

const rooms = new Map<string, Map<string, RoomClient>>();
// Track pending grace timers so we can cancel them if the client reconnects
const graceTimers = new Map<string, ReturnType<typeof setTimeout>>();

let _clientId = 0;
function nextClientId() {
  return String(++_clientId);
}

function send(ws: WebSocket, msg: SignalMessage) {
  if (ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify(msg));
  }
}

function broadcastToRoom(token: string, msg: SignalMessage, exclude?: WebSocket) {
  const room = rooms.get(token);
  if (!room) return;
  for (const client of room.values()) {
    if (client.ws !== exclude) {
      send(client.ws, msg);
    }
  }
}

function getClientByRole(token: string, role: Role): RoomClient | undefined {
  const room = rooms.get(token);
  if (!room) return undefined;
  for (const client of room.values()) {
    if (client.role === role) return client;
  }
  return undefined;
}

// ─── Termination (server is sole authority) ───────────────────────────────────

export async function terminateRoom(token: string, initiator?: WebSocket) {
  // Cancel any pending grace timer for this room
  const timer = graceTimers.get(token);
  if (timer) {
    clearTimeout(timer);
    graceTimers.delete(token);
  }

  // Broadcast terminate to all clients (server is telling everyone to clean up)
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

export function terminateExpiredRooms(tokens: string[]) {
  for (const token of tokens) {
    const room = rooms.get(token);
    if (!room) continue;
    broadcastToRoom(token, { type: "terminate" });
    for (const client of room.values()) {
      client.ws.close();
    }
    rooms.delete(token);
    const timer = graceTimers.get(token);
    if (timer) {
      clearTimeout(timer);
      graceTimers.delete(token);
    }
  }
}

// ─── WebSocket Server ─────────────────────────────────────────────────────────

export function createSignalingServer(httpServer: Server) {
  const wss = new WebSocketServer({ server: httpServer, path: "/ws" });

  // Server-side heartbeat — detect dead connections
  const heartbeat = setInterval(() => {
    wss.clients.forEach((rawWs) => {
      const ws = rawWs as WebSocket & { isAlive?: boolean };
      if (ws.isAlive === false) {
        ws.terminate();
        return;
      }
      ws.isAlive = false;
      ws.ping();
    });
  }, 30_000);

  wss.on("close", () => clearInterval(heartbeat));

  wss.on("connection", (rawWs: WebSocket, _req: IncomingMessage) => {
    const ws = rawWs as WebSocket & { isAlive?: boolean };
    ws.isAlive = true;
    ws.on("pong", () => { ws.isAlive = true; });

    const clientId = nextClientId();
    let assignedToken: string | null = null;
    let assignedRole: Role | null = null;

    ws.on("message", async (raw) => {
      let msg: SignalMessage;
      try {
        msg = JSON.parse(raw.toString()) as SignalMessage;
      } catch {
        send(ws, { type: "error", message: "Invalid JSON" });
        return;
      }

      // Keepalive ping from client
      if (msg.type === "ping") {
        send(ws, { type: "pong" });
        return;
      }

      // ── join ───────────────────────────────────────────────────────────────
      if (msg.type === "join") {
        const token = msg.token as string;
        const role = msg.role as Role;

        if (!token || !["controller", "participant", "observer"].includes(role)) {
          send(ws, { type: "error", message: "Invalid join payload" });
          return;
        }

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
        // Observers can have multiple instances; controller/participant are unique
        if (role !== "observer") {
          const existing = getClientByRole(token, role);
          if (existing) {
            send(ws, { type: "error", message: `A ${role} is already connected` });
            return;
          }
        }

        // Cancel any grace timer for this role rejoining
        const timerKey = `${token}:${role}`;
        const existingTimer = graceTimers.get(timerKey);
        if (existingTimer) {
          clearTimeout(existingTimer);
          graceTimers.delete(timerKey);
        }

        if (!rooms.has(token)) {
          rooms.set(token, new Map());
        }
        rooms.get(token)!.set(clientId, { ws, role, token });
        assignedToken = token;
        assignedRole = role;

        if (role === "participant") {
          try {
            await prisma.room.update({
              where: { token },
              data: { status: "CONNECTING" },
            });
          } catch { /* non-fatal */ }

          const controller = getClientByRole(token, "controller");
          if (controller) {
            send(controller.ws, { type: "participant_joined" });
          }
        }

        if (role === "controller") {
          const participant = getClientByRole(token, "participant");
          if (participant) {
            send(ws, { type: "participant_joined" });
          }
        }

        return;
      }

      if (!assignedToken) {
        send(ws, { type: "error", message: "Not joined to any room" });
        return;
      }

      const token = assignedToken;

      // ── offer ──────────────────────────────────────────────────────────────
      if (msg.type === "offer") {
        const participant = getClientByRole(token, "participant");
        if (participant) {
          send(participant.ws, { type: "offer", sdp: msg.sdp });
        }
        return;
      }

      // ── answer ─────────────────────────────────────────────────────────────
      if (msg.type === "answer") {
        const controller = getClientByRole(token, "controller");
        if (controller) {
          send(controller.ws, { type: "answer", sdp: msg.sdp });
        }
        return;
      }

      // ── ice_candidate ──────────────────────────────────────────────────────
      if (msg.type === "ice_candidate") {
        broadcastToRoom(token, { type: "ice_candidate", candidate: msg.candidate }, ws);
        return;
      }

      // ── active ─────────────────────────────────────────────────────────────
      if (msg.type === "active") {
        try {
          await prisma.room.update({
            where: { token },
            data: { status: "ACTIVE" },
          });
        } catch { /* non-fatal */ }
        const controller = getClientByRole(token, "controller");
        if (controller) {
          send(controller.ws, { type: "active" });
        }
        return;
      }

      // ── terminate (client requesting termination) ──────────────────────────
      // Any client can request termination. The server is authoritative —
      // it broadcasts "terminate" to ALL parties including the requester.
      if (msg.type === "terminate") {
        await terminateRoom(token);
        return;
      }
    });

    ws.on("close", async () => {
      if (!assignedToken || !assignedRole) return;

      const token = assignedToken;
      const role = assignedRole;
      const room = rooms.get(token);
      if (!room) return;

      room.delete(clientId);

      // Observers disconnecting do not affect the room
      if (role === "observer") return;

      // Start a grace period for controller/participant.
      // If the client reconnects within 10s, the timer is cancelled.
      // If not, the server terminates the room.
      const timerKey = `${token}:${role}`;
      const timer = setTimeout(async () => {
        graceTimers.delete(timerKey);
        const rejoined = getClientByRole(token, role);
        if (!rejoined) {
          await terminateRoom(token);
        }
      }, 10_000);
      graceTimers.set(timerKey, timer);
    });

    ws.on("error", (err) => {
      console.error(`[WS] client ${clientId} error:`, (err as Error).message);
    });
  });

  console.log("[WS] Signaling server attached at /ws");
  return wss;
}
