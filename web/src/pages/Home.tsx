import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { createRoom, getRoom } from "../lib/api";
import { clearSession, getSession } from "../lib/session";

export default function Home() {
  const navigate = useNavigate();
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [interrupted, setInterrupted] = useState<{ path: string; label: string } | null>(null);

  // Check for an interrupted session on mount
  useEffect(() => {
    const record = getSession();
    if (!record) return;

    // Verify the room is still active before offering to resume
    getRoom(record.token)
      .then((room) => {
        if (room.status === "WAITING" || room.status === "CONNECTING" || room.status === "ACTIVE") {
          const label =
            record.role === "controller"
              ? "Resume your session as Controller"
              : "Resume your session as Participant";
          setInterrupted({ path: record.path, label });
        } else {
          clearSession();
        }
      })
      .catch(() => clearSession());
  }, []);

  async function handleCreate() {
    clearSession();
    setLoading(true);
    setError(null);
    try {
      const room = await createRoom();
      navigate(`/wait/${room.token}`);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Something went wrong");
    } finally {
      setLoading(false);
    }
  }

  function handleResume() {
    if (interrupted) navigate(interrupted.path);
  }

  function handleDismiss() {
    clearSession();
    setInterrupted(null);
  }

  return (
    <div className="min-h-screen bg-gray-950 text-white flex flex-col items-center justify-center gap-8 px-4">
      <div className="flex flex-col items-center gap-2">
        <h1 className="text-4xl font-bold tracking-tight">Remota</h1>
        <p className="text-gray-400 text-center max-w-sm">
          Lightweight remote access. No account needed.
        </p>
      </div>

      {/* Interrupted session banner */}
      {interrupted && (
        <div className="bg-yellow-900/30 border border-yellow-700 rounded-xl p-4 max-w-sm w-full flex flex-col gap-3">
          <p className="text-yellow-300 text-sm text-center">
            You have an interrupted session.
          </p>
          <div className="flex gap-2">
            <button
              onClick={handleResume}
              className="flex-1 bg-yellow-600 hover:bg-yellow-500 text-white text-sm font-semibold py-2 rounded-lg transition-colors"
            >
              {interrupted.label}
            </button>
            <button
              onClick={handleDismiss}
              className="flex-1 bg-gray-800 hover:bg-gray-700 text-gray-300 text-sm font-medium py-2 rounded-lg transition-colors"
            >
              Dismiss
            </button>
          </div>
        </div>
      )}

      <button
        onClick={handleCreate}
        disabled={loading}
        className="bg-blue-600 hover:bg-blue-500 disabled:opacity-50 disabled:cursor-not-allowed text-white font-semibold px-8 py-3 rounded-xl transition-colors"
      >
        {loading ? "Creating…" : "Create Connection"}
      </button>

      {error && (
        <p className="text-red-400 text-sm">{error}</p>
      )}
    </div>
  );
}
