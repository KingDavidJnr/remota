import { useEffect, useRef, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { SignalingClient } from "../lib/signaling";
import { saveSession, clearSession } from "../lib/session";
import ConfirmDialog from "../components/ConfirmDialog";

// CONTRACT:
// This page is an OBSERVER — it does not own the WebRTC session.
// The desktop app owns the session. This page shows status and provides
// an End button which sends a terminate REQUEST to the server.
// It navigates away ONLY when it receives "terminate" from the server.

type SessionStatus = "waiting_for_app" | "active" | "ended";

export default function ParticipantSession() {
  const { token } = useParams<{ token: string }>();
  const navigate = useNavigate();

  const sigRef = useRef<SignalingClient | null>(null);
  const [status, setStatus] = useState<SessionStatus>("waiting_for_app");
  const [copied, setCopied] = useState(false);
  const [showConfirm, setShowConfirm] = useState(false);

  const deepLink = `remota://session/${token ?? ""}`;

  useEffect(() => {
    if (!token) return;

    const sig = new SignalingClient();
    sigRef.current = sig;
    saveSession({ token, role: "participant-desktop", path: `/session/${token}` });

    async function watch() {
      await sig.connect();

      sig.onMessage((msg) => {
        if (msg.type === "active") {
          setStatus("active");
        }
        // CONTRACT: Only navigate on server's terminate
        if (msg.type === "terminate") {
          clearSession();
          setStatus("ended");
          setTimeout(() => navigate("/"), 2000);
        }
      });
      // Do NOT register sig.onClose — server grace period handles disconnects
    }

    void watch();

    return () => {
      sig.close();
    };
  }, [token, navigate]);

  async function handleCopyToken() {
    await navigator.clipboard.writeText(token ?? "");
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  }

  function handleTerminate() {
    setShowConfirm(true);
  }

  function confirmTerminate() {
    // Send terminate REQUEST to server — server will broadcast to all parties
    sigRef.current?.send({ type: "terminate" });
    // Do not navigate yet — wait for server to send terminate back to us
  }

  if (status === "ended") {
    return (
      <div className="min-h-screen bg-gray-950 text-white flex items-center justify-center">
        <p className="text-gray-400">Connection ended.</p>
      </div>
    );
  }

  return (
    <div className="min-h-screen bg-gray-950 text-white flex flex-col items-center justify-center gap-8 px-4">
      <div className="flex flex-col items-center gap-2 text-center">
        <div className="flex items-center gap-2">
          <span className={`w-2.5 h-2.5 rounded-full inline-block ${
            status === "active" ? "bg-green-400 animate-pulse" : "bg-yellow-400 animate-pulse"
          }`} />
          <span className={`font-medium ${status === "active" ? "text-green-400" : "text-yellow-400"}`}>
            {status === "active" ? "Remote connection is active" : "Waiting for desktop app..."}
          </span>
        </div>
        <p className="text-gray-400 text-sm max-w-sm text-center">
          {status === "active"
            ? "The other person can see and control your screen."
            : "Open the Remota desktop app on this computer to start the session."}
        </p>
      </div>

      {status === "waiting_for_app" && (
        <div className="bg-gray-900 border border-gray-800 rounded-2xl p-6 max-w-md w-full flex flex-col gap-4">
          <h3 className="font-semibold text-sm text-gray-300 uppercase tracking-wide">How to connect</h3>
          <ol className="text-sm text-gray-400 flex flex-col gap-3 list-decimal list-inside">
            <li>Download and open <strong className="text-white">Remota Desktop</strong> on this computer.</li>
            <li>Enter the session token below, or click the launch button.</li>
            <li>Grant the requested screen recording and accessibility permissions.</li>
          </ol>
          <div className="flex flex-col gap-1">
            <p className="text-xs text-gray-500 uppercase tracking-wide">Session token</p>
            <div className="flex items-center gap-2">
              <code className="flex-1 bg-gray-800 text-green-300 text-sm px-3 py-2 rounded-lg font-mono break-all">
                {token}
              </code>
              <button
                onClick={handleCopyToken}
                className="bg-gray-700 hover:bg-gray-600 text-white text-xs font-medium px-3 py-2 rounded-lg transition-colors whitespace-nowrap"
              >
                {copied ? "Copied!" : "Copy"}
              </button>
            </div>
          </div>
          <a
            href={deepLink}
            className="w-full bg-blue-600 hover:bg-blue-500 text-white font-semibold py-3 rounded-xl transition-colors text-center text-sm"
          >
            Launch Remota Desktop
          </a>
          <p className="text-xs text-gray-600 text-center">
            The launch button works if Remota Desktop is already installed.
          </p>
        </div>
      )}

      {status === "active" && (
        <div className="bg-gray-900 border border-gray-800 rounded-xl p-5 text-sm text-gray-400 max-w-sm w-full text-center">
          Remote control is in progress. You can end it at any time below.
        </div>
      )}

      <button
        onClick={handleTerminate}
        className="bg-red-600 hover:bg-red-500 text-white font-semibold px-6 py-2.5 rounded-xl transition-colors"
      >
        End Connection
      </button>

      {showConfirm && (
        <ConfirmDialog
          message="Are you sure you want to end this connection? The session will be terminated for both parties."
          confirmLabel="End Connection"
          onConfirm={confirmTerminate}
          onCancel={() => setShowConfirm(false)}
        />
      )}
    </div>
  );
}
