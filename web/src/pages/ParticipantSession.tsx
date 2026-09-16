import { useEffect, useRef, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { SignalingClient } from "../lib/signaling";

// The web participant page does NOT do WebRTC or screen capture.
// Its only job is:
//   1. Show the participant a "launch the desktop app" instruction.
//   2. Provide the token the desktop app needs (via deep-link or copy).
//   3. Listen on the signaling socket so it can show live status
//      (waiting → active) and surface an "End Connection" button.
//
// The Rust desktop endpoint joins as "participant" over WebSocket, does the
// actual screen capture and WebRTC, and signals "active" when connected.

type SessionStatus = "waiting_for_app" | "active" | "ended";

export default function ParticipantSession() {
  const { token } = useParams<{ token: string }>();
  const navigate = useNavigate();

  const sigRef = useRef<SignalingClient | null>(null);
  const [status, setStatus] = useState<SessionStatus>("waiting_for_app");
  const [copied, setCopied] = useState(false);

  // Deep-link URI that the desktop app can be launched with.
  // The OS will pass it to the registered remota:// protocol handler.
  const deepLink = `remota://session/${token ?? ""}`;

  useEffect(() => {
    if (!token) return;

    // Connect to signaling as an observer (no role) so we can watch for
    // the "active" and "terminate" events from the desktop endpoint.
    // We intentionally do NOT join as "participant" — that's the desktop's job.
    const sig = new SignalingClient();
    sigRef.current = sig;

    async function watch() {
      await sig.connect();

      sig.onMessage((msg) => {
        if (msg.type === "active") {
          setStatus("active");
        }
        if (msg.type === "terminate" || msg.type === "participant_left") {
          setStatus("ended");
          setTimeout(() => navigate("/"), 2000);
        }
      });

      sig.onClose(() => {
        setStatus("ended");
      });
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
    sigRef.current?.send({ type: "terminate" });
    sigRef.current?.close();
    navigate("/");
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

      {/* Status indicator */}
      <div className="flex flex-col items-center gap-2 text-center">
        <div className="flex items-center gap-2">
          <span className={`w-2.5 h-2.5 rounded-full inline-block ${
            status === "active" ? "bg-green-400 animate-pulse" : "bg-yellow-400 animate-pulse"
          }`} />
          <span className={`font-medium ${status === "active" ? "text-green-400" : "text-yellow-400"}`}>
            {status === "active" ? "Remote connection is active" : "Waiting for desktop app…"}
          </span>
        </div>
        <p className="text-gray-400 text-sm max-w-sm text-center">
          {status === "active"
            ? "The other person can see and control your screen."
            : "Open the Remota desktop app on this computer to start the session."}
        </p>
      </div>

      {/* Launch / token instructions */}
      {status === "waiting_for_app" && (
        <div className="bg-gray-900 border border-gray-800 rounded-2xl p-6 max-w-md w-full flex flex-col gap-4">
          <h3 className="font-semibold text-sm text-gray-300 uppercase tracking-wide">
            How to connect
          </h3>

          <ol className="text-sm text-gray-400 flex flex-col gap-3 list-decimal list-inside">
            <li>Download and open <strong className="text-white">Remota Desktop</strong> on this computer.</li>
            <li>Enter the session token below, or click the launch button.</li>
            <li>Grant the requested screen recording and accessibility permissions.</li>
          </ol>

          {/* Token display */}
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

          {/* Deep-link launch button */}
          <a
            href={deepLink}
            className="w-full bg-blue-600 hover:bg-blue-500 text-white font-semibold py-3 rounded-xl transition-colors text-center text-sm"
          >
            Launch Remota Desktop
          </a>

          <p className="text-xs text-gray-600 text-center">
            The launch button works if Remota Desktop is already installed.
            You can also start it manually and paste the token.
          </p>
        </div>
      )}

      {/* Active session info */}
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
    </div>
  );
}
