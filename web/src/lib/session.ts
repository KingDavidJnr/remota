// Persists the current session role + token in sessionStorage so that
// a page refresh can detect an interrupted session and offer to reconnect.

const KEY = "remota_session";

export interface SessionRecord {
  token: string;
  role: "controller" | "participant-browser" | "participant-desktop";
  path: string; // the URL path to navigate back to
}

export function saveSession(record: SessionRecord) {
  sessionStorage.setItem(KEY, JSON.stringify(record));
}

export function clearSession() {
  sessionStorage.removeItem(KEY);
}

export function getSession(): SessionRecord | null {
  const raw = sessionStorage.getItem(KEY);
  if (!raw) return null;
  try {
    return JSON.parse(raw) as SessionRecord;
  } catch {
    return null;
  }
}
