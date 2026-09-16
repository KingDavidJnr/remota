const BASE = import.meta.env.VITE_BACKEND_URL as string;

export interface RoomResponse {
  id: string;
  token: string;
  status: string;
  expiresAt: string;
}

export async function createRoom(): Promise<RoomResponse> {
  const res = await fetch(`${BASE}/rooms`, { method: "POST" });
  if (!res.ok) throw new Error("Failed to create room");
  return res.json() as Promise<RoomResponse>;
}

export async function getRoom(token: string): Promise<RoomResponse> {
  const res = await fetch(`${BASE}/rooms/${token}`);
  if (res.status === 404) throw new Error("Room not found");
  if (res.status === 410) throw new Error("Room expired or ended");
  if (!res.ok) throw new Error("Failed to fetch room");
  return res.json() as Promise<RoomResponse>;
}
