import { useEffect, useRef, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { SignalingClient } from "../lib/signaling";
import { buildIceServers } from "../lib/ice";
import { saveSession, clearSession } from "../lib/session";
import ConfirmDialog from "../components/ConfirmDialog";

type Status = "requesting" | "connecting" | "active" | "ended";

export default function BrowserSession() {
  const { token } = useParams<{ token: string }>();
  const navigate = useNavigate();

  const sigRef = useRef<SignalingClient | null>(null);
  const pcRef = useRef<RTCPeerConnection | null>(null);
  const streamRef = useRef<MediaStream | null>(null);

  const [status, setStatus] = useState<Status>("requesting");
  const [error, setError] = useState<string | null>(null);
  const [showConfirm, setShowConfirm] = useState(false);

  useEffect(() => {
    if (!token) return;

    const sig = new SignalingClient();
    sigRef.current = sig;

    // Buffer for messages that arrive before the PC is ready
    const pendingMessages: Array<Record<string, unknown> & { type: string }> = [];
    let pc: RTCPeerConnection | null = null;

    async function handleSignalMessage(msg: Record<string, unknown> & { type: string }) {
      if (!pc) {
        // PC not ready yet — buffer the message
        pendingMessages.push(msg);
        return;
      }
      await processMessage(pc, msg);
    }

    async function processMessage(pc: RTCPeerConnection, msg: Record<string, unknown> & { type: string }) {
      if (msg.type === "offer") {
        await pc.setRemoteDescription(
          new RTCSessionDescription(msg.sdp as RTCSessionDescriptionInit)
        );
        const answer = await pc.createAnswer();
        await pc.setLocalDescription(answer);
        sig.send({ type: "answer", sdp: pc.localDescription });
      }

      if (msg.type === "ice_candidate" && msg.candidate) {
        try {
          await pc.addIceCandidate(
            new RTCIceCandidate(msg.candidate as RTCIceCandidateInit)
          );
        } catch { /* ignore candidates before remote description */ }
      }

      if (msg.type === "terminate") {
        cleanup();
      }
    }

    async function start() {
      // Step 1 — connect to signaling immediately and register handler
      // BEFORE showing the getDisplayMedia picker, so we never miss the offer
      await sig.connect();
      sig.onMessage(handleSignalMessage);
      sig.onClose(() => cleanup());
      sig.send({ type: "join", token, role: "participant" });

      // Save session for refresh recovery
      saveSession({ token: token!, role: "participant-browser", path: `/browser-session/${token}` });

      // Warn on refresh while active
      const beforeUnload = (e: BeforeUnloadEvent) => { e.preventDefault(); };
      window.addEventListener("beforeunload", beforeUnload);

      // Step 2 — request screen capture (shows picker dialog)
      let stream: MediaStream;
      try {
        stream = await navigator.mediaDevices.getDisplayMedia({
          video: { frameRate: 30 },
          audio: false,
        });
        streamRef.current = stream;
      } catch {
        setError("Screen sharing was denied or cancelled.");
        setStatus("ended");
        return;
      }

      stream.getVideoTracks()[0]?.addEventListener("ended", () => cleanup());

      setStatus("connecting");

      // Step 3 — set up peer connection with the captured stream
      pc = new RTCPeerConnection({ iceServers: buildIceServers() });
      pcRef.current = pc;

      for (const track of stream.getTracks()) {
        pc.addTrack(track, stream);
      }

      pc.onicecandidate = (e) => {
        if (e.candidate) {
          sig.send({ type: "ice_candidate", candidate: e.candidate.toJSON() });
        }
      };

      pc.onconnectionstatechange = () => {
        if (pc!.connectionState === "connected") {
          setStatus("active");
          sig.send({ type: "active" });
        }
        if (
          pc!.connectionState === "failed" ||
          pc!.connectionState === "disconnected" ||
          pc!.connectionState === "closed"
        ) {
          cleanup();
        }
      };

      // Step 4 — drain any buffered messages that arrived during getDisplayMedia
      for (const msg of pendingMessages.splice(0)) {
        await processMessage(pc, msg);
      }
    }

    function cleanup() {
      clearSession();
      streamRef.current?.getTracks().forEach((t) => t.stop());
      sig.close();
      pcRef.current?.close();
      setStatus("ended");
    }

    void start();
    return () => cleanup();
  }, [token]);

  function handleTerminate() {
    setShowConfirm(true);
  }

  function confirmTerminate() {
    clearSession();
    sigRef.current?.send({ type: "terminate" });
    sigRef.current?.close();
    pcRef.current?.close();
    streamRef.current?.getTracks().forEach((t) => t.stop());
    navigate("/");
  }

  if (status === "requesting") {
    return (
      <div className="min-h-screen bg-gray-950 text-white flex items-center justify-center">
        <p className="text-gray-400">Waiting for screen sharing permission…</p>
      </div>
    );
  }

  if (status === "ended" && error) {
    return (
      <div className="min-h-screen bg-gray-950 text-white flex flex-col items-center justify-center gap-4 px-4">
        <p className="text-red-400 text-sm">{error}</p>
        <button
          onClick={() => navigate("/")}
          className="text-blue-400 hover:text-blue-300 text-sm transition-colors"
        >
          Go home
        </button>
      </div>
    );
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
          <span className={`w-2.5 h-2.5 rounded-full inline-block animate-pulse ${
            status === "active" ? "bg-green-400" : "bg-yellow-400"
          }`} />
          <span className={`font-medium ${status === "active" ? "text-green-400" : "text-yellow-400"}`}>
            {status === "active" ? "Screen sharing is active" : "Establishing connection…"}
          </span>
        </div>
        <p className="text-gray-400 text-sm max-w-sm">
          {status === "active"
            ? "The other person can see your screen. This is view-only — they cannot control your computer."
            : "Connecting to the remote viewer…"}
        </p>
      </div>

      {status === "active" && (
        <div className="bg-blue-900/20 border border-blue-800 text-blue-300 text-sm rounded-xl px-5 py-3 max-w-sm w-full text-center">
          🌐 Browser mode — view only. No mouse or keyboard control.
        </div>
      )}

      <button
        onClick={handleTerminate}
        className="bg-red-600 hover:bg-red-500 text-white font-semibold px-6 py-2.5 rounded-xl transition-colors"
      >
        Stop Sharing
      </button>

      {showConfirm && (
        <ConfirmDialog
          message="Are you sure you want to stop sharing? This will end the session for both parties."
          confirmLabel="Stop Sharing"
          onConfirm={confirmTerminate}
          onCancel={() => setShowConfirm(false)}
        />
      )}
    </div>
  );
}
