import { Router, Request, Response } from "express";
import prisma from "../lib/prisma";
import { generateToken, expiresInMinutes } from "../lib/token";

const router = Router();

// POST /rooms  — create a new room
router.post("/", async (_req: Request, res: Response) => {
  try {
    const token = generateToken();
    const room = await prisma.room.create({
      data: {
        token,
        status: "WAITING",
        expiresAt: expiresInMinutes(60), // rooms expire after 60 minutes
      },
    });

    res.status(201).json({
      id: room.id,
      token: room.token,
      status: room.status,
      expiresAt: room.expiresAt,
    });
  } catch (err) {
    console.error("[POST /rooms]", err);
    res.status(500).json({ error: "Failed to create room" });
  }
});

// GET /rooms/:token  — validate a room by its token
router.get("/:token", async (req: Request, res: Response) => {
  const token = req.params["token"] as string;

  try {
    const room = await prisma.room.findUnique({ where: { token } });

    if (!room) {
      res.status(404).json({ error: "Room not found" });
      return;
    }

    // Mark as expired if past expiry and still WAITING/CONNECTING
    if (
      room.expiresAt < new Date() &&
      (room.status === "WAITING" || room.status === "CONNECTING")
    ) {
      await prisma.room.update({
        where: { id: room.id },
        data: { status: "EXPIRED" },
      });
      res.status(410).json({ error: "Room has expired" });
      return;
    }

    if (room.status === "EXPIRED") {
      res.status(410).json({ error: "Room has expired" });
      return;
    }

    if (room.status === "ENDED") {
      res.status(410).json({ error: "Room has ended" });
      return;
    }

    res.json({
      id: room.id,
      token: room.token,
      status: room.status,
      expiresAt: room.expiresAt,
    });
  } catch (err) {
    console.error("[GET /rooms/:token]", err);
    res.status(500).json({ error: "Failed to fetch room" });
  }
});

export default router;
