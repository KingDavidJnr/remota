/**
 * Build the RTCIceServer list from environment variables.
 *
 * Required env var:
 *   VITE_BACKEND_URL – base URL of the backend (already used elsewhere)
 *
 * Optional TURN env vars (all must be set together to enable TURN):
 *   VITE_TURN_URL      e.g. turn:turn.remota.quickdesk.tech:3478
 *   VITE_TURN_USERNAME
 *   VITE_TURN_PASSWORD
 */
export function buildIceServers(): RTCIceServer[] {
  const servers: RTCIceServer[] = [
    { urls: "stun:stun.l.google.com:19302" },
  ];

  const turnUrl = import.meta.env.VITE_TURN_URL as string | undefined;
  const turnUser = import.meta.env.VITE_TURN_USERNAME as string | undefined;
  const turnPass = import.meta.env.VITE_TURN_PASSWORD as string | undefined;

  if (turnUrl && turnUser && turnPass) {
    servers.push({
      urls: turnUrl,
      username: turnUser,
      credential: turnPass,
    });
  }

  return servers;
}
