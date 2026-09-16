import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { createRoom } from "../lib/api";

export default function Home() {
  const navigate = useNavigate();
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleCreate() {
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

  return (
    <div className="min-h-screen bg-gray-950 text-white flex flex-col items-center justify-center gap-8 px-4">
      <div className="flex flex-col items-center gap-2">
        <h1 className="text-4xl font-bold tracking-tight">Remota</h1>
        <p className="text-gray-400 text-center max-w-sm">
          Lightweight remote access. No account needed.
        </p>
      </div>

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
