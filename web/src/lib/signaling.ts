const WS_BASE = (import.meta.env.VITE_BACKEND_URL as string)
  .replace(/^https/, "wss")
  .replace(/^http/, "ws");

export function getWsUrl(): string {
  return `${WS_BASE}/ws`;
}

export type SignalMessage = Record<string, unknown> & { type: string };

export class SignalingClient {
  private ws: WebSocket | null = null;
  private listeners: Array<(msg: SignalMessage) => void> = [];
  private closeListeners: Array<() => void> = [];

  connect(): Promise<void> {
    return new Promise((resolve, reject) => {
      this.ws = new WebSocket(getWsUrl());
      this.ws.onopen = () => resolve();
      this.ws.onerror = () => reject(new Error("WebSocket connection failed"));
      this.ws.onmessage = (e) => {
        try {
          const msg = JSON.parse(e.data as string) as SignalMessage;
          this.listeners.forEach((fn) => fn(msg));
        } catch {
          // ignore unparseable frames
        }
      };
      this.ws.onclose = () => {
        this.closeListeners.forEach((fn) => fn());
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
    this.ws?.close();
    this.ws = null;
  }
}
