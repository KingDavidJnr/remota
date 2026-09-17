const WS_BASE = (import.meta.env.VITE_BACKEND_URL as string)
  .replace(/^https/, "wss")
  .replace(/^http/, "ws");

export function getWsUrl(): string {
  return `${WS_BASE}/ws`;
}

export type SignalMessage = Record<string, unknown> & { type: string };

const PING_INTERVAL_MS = 25_000; // send a ping every 25s to keep the connection alive

export class SignalingClient {
  private ws: WebSocket | null = null;
  private listeners: Array<(msg: SignalMessage) => void> = [];
  private closeListeners: Array<() => void> = [];
  private pingTimer: ReturnType<typeof setInterval> | null = null;
  private intentionalClose = false;

  connect(): Promise<void> {
    return new Promise((resolve, reject) => {
      this.intentionalClose = false;
      this.ws = new WebSocket(getWsUrl());

      this.ws.onopen = () => {
        // Start keepalive pings
        this.pingTimer = setInterval(() => {
          if (this.ws?.readyState === WebSocket.OPEN) {
            this.ws.send(JSON.stringify({ type: "ping" }));
          }
        }, PING_INTERVAL_MS);
        resolve();
      };

      this.ws.onerror = () => reject(new Error("WebSocket connection failed"));

      this.ws.onmessage = (e) => {
        try {
          const msg = JSON.parse(e.data as string) as SignalMessage;
          // Ignore pong frames — they're just keepalives
          if (msg.type === "pong") return;
          this.listeners.forEach((fn) => fn(msg));
        } catch {
          // ignore unparseable frames
        }
      };

      this.ws.onclose = () => {
        this._stopPing();
        // Only fire close listeners if this wasn't an intentional close
        if (!this.intentionalClose) {
          this.closeListeners.forEach((fn) => fn());
        }
      };
    });
  }

  send(msg: SignalMessage) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
    }
  }

  onMessage(fn: (msg: SignalMessage) => void) {
    this.listeners.push(fn);
    return () => {
      this.listeners = this.listeners.filter((l) => l !== fn);
    };
  }

  onClose(fn: () => void) {
    this.closeListeners.push(fn);
    return () => {
      this.closeListeners = this.closeListeners.filter((l) => l !== fn);
    };
  }

  close() {
    this.intentionalClose = true;
    this._stopPing();
    this.ws?.close();
    this.ws = null;
  }

  private _stopPing() {
    if (this.pingTimer !== null) {
      clearInterval(this.pingTimer);
      this.pingTimer = null;
    }
  }
}
