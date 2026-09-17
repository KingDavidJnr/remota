import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { getRoom } from "../lib/api";

type Step = "consent" | "mode";

// getDisplayMedia is desktop-only — not supported on any mobile browser
const supportsDisplayMedia =
  typeof navigator !== "undefined" &&
  typeof navigator.mediaDevices?.getDisplayMedia === "function" &&
  !/Android|iPhone|iPad|iPod/i.test(navigator.userAgent);

export default function JoinConsent() {
  const { token } = useParams<{ token: string }>();
  const navigate = useNavigate();

  const [checking, setChecking] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [step, setStep] = useState<Step>("consent");

  useEffect(() => {
    if (!token) return;
    getRoom(token)
      .then(() => setChecking(false))
      .catch((err: unknown) =>
        setError(err instanceof Error ? err.message : "Invalid link")
      );
  }, [token]);

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

  // ── Step 1: Consent ────────────────────────────────────────────────────────
  if (step === "consent") {
    return (
      <div className="min-h-screen bg-gray-950 text-white flex items-center justify-center px-4">
        <div className="bg-gray-900 border border-gray-800 rounded-2xl p-8 max-w-md w-full flex flex-col items-center gap-6 text-center">
          <div className="flex flex-col gap-2">
            <h2 className="text-2xl font-semibold">Someone wants to connect to your computer.</h2>
            <p className="text-gray-400 text-sm">
              By accepting, you will allow the other person to{" "}
              <strong className="text-white">view your screen</strong> remotely.
            </p>
          </div>

          <div className="bg-yellow-900/30 border border-yellow-700 text-yellow-300 text-sm rounded-xl px-4 py-3 w-full">
            Only accept if you trust the person who sent this link.
          </div>

          <div className="flex flex-col gap-3 w-full">
            <button
              onClick={() => setStep("mode")}
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

  // ── Step 2: Mode selection ─────────────────────────────────────────────────
  return (
    <div className="min-h-screen bg-gray-950 text-white flex items-center justify-center px-4">
      <div className="bg-gray-900 border border-gray-800 rounded-2xl p-8 max-w-md w-full flex flex-col gap-6">
        <div className="text-center flex flex-col gap-1">
          <h2 className="text-xl font-semibold">How do you want to share?</h2>
          <p className="text-gray-400 text-sm">Choose a connection mode.</p>
        </div>

        {/* Browser mode */}
        <button
          onClick={() => supportsDisplayMedia && navigate(`/browser-session/${token ?? ""}`)}
          disabled={!supportsDisplayMedia}
          className={`flex flex-col gap-1.5 border text-left px-5 py-4 rounded-xl transition-colors group ${
            supportsDisplayMedia
              ? "bg-gray-800 hover:bg-gray-700 border-gray-700 hover:border-blue-500"
              : "bg-gray-900 border-gray-800 opacity-50 cursor-not-allowed"
          }`}
        >
          <div className="flex items-center gap-2">
            <span className="text-lg">🌐</span>
            <span className={`font-semibold transition-colors ${supportsDisplayMedia ? "text-white group-hover:text-blue-400" : "text-gray-500"}`}>
              Share from Browser
            </span>
            {supportsDisplayMedia ? (
              <span className="ml-auto text-xs bg-green-900/50 text-green-400 border border-green-800 px-2 py-0.5 rounded-full">
                No install
              </span>
            ) : (
              <span className="ml-auto text-xs bg-gray-800 text-gray-500 border border-gray-700 px-2 py-0.5 rounded-full">
                Desktop only
              </span>
            )}
          </div>
          <p className="text-sm text-gray-400 pl-7">
            {supportsDisplayMedia
              ? "Share your screen directly from this browser tab. The other person can view your screen only."
              : "Screen sharing from a browser is not supported on mobile devices."}
          </p>
        </button>

        {/* Desktop app mode */}
        <button
          onClick={() => navigate(`/session/${token ?? ""}`)}
          className="flex flex-col gap-1.5 bg-gray-800 hover:bg-gray-700 border border-gray-700 hover:border-blue-500 text-left px-5 py-4 rounded-xl transition-colors group"
        >
          <div className="flex items-center gap-2">
            <span className="text-lg">🖥️</span>
            <span className="font-semibold text-white group-hover:text-blue-400 transition-colors">
              Use Desktop App
            </span>
            <span className="ml-auto text-xs bg-blue-900/50 text-blue-400 border border-blue-800 px-2 py-0.5 rounded-full">
              Full control
            </span>
          </div>
          <p className="text-sm text-gray-400 pl-7">
            Download and run the Remota Desktop app. The other person can view and fully control your computer.
          </p>
        </button>

        {/* Viewer mode */}
        <button
          onClick={() => navigate(`/viewer-session/${token ?? ""}`)}
          className="flex flex-col gap-1.5 bg-gray-800 hover:bg-gray-700 border border-gray-700 hover:border-purple-500 text-left px-5 py-4 rounded-xl transition-colors group"
        >
          <div className="flex items-center gap-2">
            <span className="text-lg">👁️</span>
            <span className="font-semibold text-white group-hover:text-purple-400 transition-colors">
              Join as Viewer
            </span>
            <span className="ml-auto text-xs bg-purple-900/50 text-purple-400 border border-purple-800 px-2 py-0.5 rounded-full">
              Watch only
            </span>
          </div>
          <p className="text-sm text-gray-400 pl-7">
            Watch and listen to the shared screen without sharing your own. Mic optional.
          </p>
        </button>

        <button
          onClick={handleDecline}
          className="text-gray-500 hover:text-gray-400 text-sm text-center transition-colors"
        >
          Cancel
        </button>
      </div>
    </div>
  );
}
