import { WebSocketServer, WebSocket } from "ws";
import { IncomingMessage } from "http";
import { Server } from "http";
import prisma from "../lib/prisma";

// ─── Types ────────────────────────────────────────────────────────────────────

type Role = "controller" | "participant" | "viewer";

interface RoomClient {
  ws: WebSocket;
  role: Role;
  token: string;
  clientId: string;
}

type SignalType =
  | "join"
  | "offer"
  | "answer"
  | "ice_candidate"
  | "participant_joined"
  | "participant_left"
  | "viewer_joined"
  | "viewer_left"
  | "active"
  | "ping"
  | "pong"
  | "terminate"
  | "error";

interface SignalMessage {
  type: SignalType;
  [key: string]: unknown;
}

// ─── Room Registry ────────────────────────────────────────────────────────────

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

function getClientById(token: string, clientId: string): RoomClient | undefined {
  return rooms.get(token)?.get(clientId);
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

  // Server-side heartbeat — terminate connections that stop responding
  const heartbeat = setInterval(() => {
    wss.clients.forEach((ws) => {
      const client = ws as WebSocket & { isAlive?: boolean };
      if (client.isAlive === false) {
        client.terminate();
        return;
      }
      client.isAlive = false;
      client.ping();
    });
  }, 30_000);

  wss.on("close", () => clearInterval(heartbeat));

  wss.on("connection", (ws: WebSocket, _req: IncomingMessage) => {
    const client = ws as WebSocket & { isAlive?: boolean };
    client.isAlive = true;
    ws.on("pong", () => { client.isAlive = true; });

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

      // ── ping/pong keepalive ────────────────────────────────────────────────
      if (msg.type === "ping") {
        send(ws, { type: "pong" });
        return;
      }

      // ── join ───────────────────────────────────────────────────────────────
      if (msg.type === "join") {
        const token = msg.token as string;
        const role = msg.role as Role;

        if (!token || !["controller", "participant", "viewer"].includes(role)) {
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

        // Controller and participant are unique; viewers are not
        if (role === "controller" || role === "participant") {
          const existing = getClientByRole(token, role);
          if (existing) {
            send(ws, {
              type: "error",
              message: `A ${role} is already connected`,
            });
            return;
          }
        }

        // Register client
        if (!rooms.has(token)) {
          rooms.set(token, new Map());
        }
        rooms.get(token)!.set(clientId, { ws, role, token, clientId });
        assignedToken = token;

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

        if (role === "viewer") {
          // Notify controller so it can create an offer for this viewer
          const controller = getClientByRole(token, "controller");
          if (controller) {
            send(controller.ws, { type: "viewer_joined", viewerId: clientId });
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
      // Controller may send an offer targeted at a specific viewer (viewerId)
      // or the general participant
      if (msg.type === "offer") {
        const viewerId = msg.viewerId as string | undefined;
        if (viewerId) {
          // Directed offer to a specific viewer
          const viewer = getClientById(token, viewerId);
          if (viewer) {
            send(viewer.ws, { type: "offer", sdp: msg.sdp });
          }
        } else {
          // Offer to the participant
          const participant = getClientByRole(token, "participant");
          if (participant) {
            send(participant.ws, { type: "offer", sdp: msg.sdp });
          }
        }
        return;
      }

      // ── answer ─────────────────────────────────────────────────────────────
      if (msg.type === "answer") {
        const controller = getClientByRole(token, "controller");
        if (controller) {
          const senderRole = rooms.get(token)?.get(clientId)?.role;
          // Only tag with fromId for viewer answers so the controller can route
          // them to the correct viewer PC. Participant answers go untagged.
          if (senderRole === "viewer") {
            send(controller.ws, { type: "answer", sdp: msg.sdp, fromId: clientId });
          } else {
            send(controller.ws, { type: "answer", sdp: msg.sdp });
          }
        }
        return;
      }

      // ── ice_candidate ──────────────────────────────────────────────────────
      // Viewers and participants send candidates; controller sends targeted ones
      if (msg.type === "ice_candidate") {
        const targetId = msg.targetId as string | undefined;
        if (targetId) {
          // Targeted candidate from controller to specific viewer/participant
          const target = getClientById(token, targetId);
          if (target) {
            send(target.ws, { type: "ice_candidate", candidate: msg.candidate });
          }
        } else {
          // From viewer/participant — send to controller
          const controller = getClientByRole(token, "controller");
          if (controller) {
            const senderRole = rooms.get(token)?.get(clientId)?.role;
            // Only tag with fromId for viewer ICE so controller routes to correct viewer PC
            if (senderRole === "viewer") {
              send(controller.ws, {
                type: "ice_candidate",
                candidate: msg.candidate,
                fromId: clientId,
              });
            } else {
              send(controller.ws, {
                type: "ice_candidate",
                candidate: msg.candidate,
              });
            }
          }
        }
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

      if (leaving?.role === "viewer") {
        // Viewers leaving do not terminate the room
        const controller = getClientByRole(token, "controller");
        if (controller) {
          send(controller.ws, { type: "viewer_left", viewerId: clientId });
        }
        return;
      }

      if (leaving?.role === "participant") {
        const controller = getClientByRole(token, "controller");
        if (controller) {
          send(controller.ws, { type: "participant_left" });
        }
        await terminateRoom(token);
      } else if (leaving?.role === "controller") {
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
