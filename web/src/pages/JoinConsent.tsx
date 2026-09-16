import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { getRoom } from "../lib/api";

export default function JoinConsent() {
  const { token } = useParams<{ token: string }>();
  const navigate = useNavigate();

  const [checking, setChecking] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!token) return;
    getRoom(token)
      .then(() => setChecking(false))
      .catch((err: unknown) =>
        setError(err instanceof Error ? err.message : "Invalid link")
      );
  }, [token]);

  function handleAccept() {
    navigate(`/session/${token ?? ""}`);
  }

  function handleDecline() {
    navigate("/");
  }

  if (checking && !error) {
    return (
      <div className="min-h-screen bg-gray-950 text-white flex items-center justify-center">
        <p className="text-gray-400">Validating link…</p>
      </div>
    );
  }

  if (error) {
    return (
      <div className="min-h-screen bg-gray-950 text-white flex flex-col items-center justify-center gap-4 px-4">
        <h2 className="text-xl font-semibold text-red-400">Link Invalid</h2>
        <p className="text-gray-400 text-sm text-center">{error}</p>
        <button
          onClick={() => navigate("/")}
          className="text-blue-400 hover:text-blue-300 text-sm transition-colors"
        >
          Go home
        </button>
      </div>
    );
  }

  return (
    <div className="min-h-screen bg-gray-950 text-white flex items-center justify-center px-4">
      <div className="bg-gray-900 border border-gray-800 rounded-2xl p-8 max-w-md w-full flex flex-col items-center gap-6 text-center">
        <div className="flex flex-col gap-2">
          <h2 className="text-2xl font-semibold">Someone wants to connect to your computer.</h2>
          <p className="text-gray-400 text-sm">
            By accepting, you will allow the other person to <strong className="text-white">view and control</strong> your computer remotely.
          </p>
        </div>

        <div className="bg-yellow-900/30 border border-yellow-700 text-yellow-300 text-sm rounded-xl px-4 py-3 w-full">
          Only accept if you trust the person who sent this link.
        </div>

        <div className="flex flex-col gap-3 w-full">
          <button
            onClick={handleAccept}
            className="w-full bg-blue-600 hover:bg-blue-500 text-white font-semibold py-3 rounded-xl transition-colors"
          >
            Accept Connection
          </button>
          <button
            onClick={handleDecline}
            className="w-full bg-gray-800 hover:bg-gray-700 text-gray-300 font-medium py-3 rounded-xl transition-colors"
          >
            Decline
          </button>
        </div>
      </div>
    </div>
  );
}
