import { useEffect, useRef, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { SignalingClient } from "../lib/signaling";
import { buildIceServers } from "../lib/ice";
import { normalizePointer } from "../lib/control";

export default function ControllerRoom() {
  const { token } = useParams<{ token: string }>();
  const navigate = useNavigate();

  const sigRef = useRef<SignalingClient | null>(null);
  const pcRef = useRef<RTCPeerConnection | null>(null);
  const dcRef = useRef<RTCDataChannel | null>(null);
  const videoRef = useRef<HTMLVideoElement>(null);

  const [status, setStatus] = useState<"waiting" | "connecting" | "connected" | "ended">("waiting");
  const [copied, setCopied] = useState(false);

  const joinUrl = `${window.location.origin}/join/${token ?? ""}`;

  useEffect(() => {
    if (!token) return;

    const sig = new SignalingClient();
    sigRef.current = sig;

    let pc: RTCPeerConnection;

    async function start() {
      await sig.connect();
      sig.send({ type: "join", token, role: "controller" });

      sig.onMessage(async (msg) => {
        if (msg.type === "participant_joined") {
          setStatus("connecting");
          await startOffer();
        }

        if (msg.type === "answer") {
          await pc.setRemoteDescription(
            new RTCSessionDescription(msg.sdp as RTCSessionDescriptionInit)
          );
        }

        if (msg.type === "ice_candidate" && msg.candidate) {
          await pc.addIceCandidate(
            new RTCIceCandidate(msg.candidate as RTCIceCandidateInit)
          );
        }

        // Desktop endpoint confirmed WebRTC is active
        if (msg.type === "active") {
          setStatus("connected");
        }

        if (msg.type === "terminate" || msg.type === "participant_left") {
          cleanup();
          navigate("/");
        }
      });
    }

    async function startOffer() {
      pc = new RTCPeerConnection({ iceServers: buildIceServers() });
      pcRef.current = pc;

      // DataChannel for control messages
      const dc = pc.createDataChannel("control");
      dcRef.current = dc;

      // Receive remote screen
      pc.ontrack = (e) => {
        if (videoRef.current && e.streams[0]) {
          videoRef.current.srcObject = e.streams[0];
          setStatus("connected");
        }
      };

      pc.onicecandidate = (e) => {
        if (e.candidate) {
          sig.send({ type: "ice_candidate", candidate: e.candidate.toJSON() });
        }
      };

      pc.onconnectionstatechange = () => {
        if (
          pc.connectionState === "failed" ||
          pc.connectionState === "disconnected" ||
          pc.connectionState === "closed"
        ) {
          cleanup();
          navigate("/");
        }
      };

      const offer = await pc.createOffer();
      await pc.setLocalDescription(offer);
      sig.send({ type: "offer", sdp: pc.localDescription });
    }

    void start();
    return () => cleanup();

    function cleanup() {
      sig.close();
      pc?.close();
      setStatus("ended");
    }
  }, [token, navigate]);

  function sendControl(msg: object) {
    const dc = dcRef.current;
    if (dc?.readyState === "open") {
      dc.send(JSON.stringify(msg));
    }
  }

  function handlePointerMove(e: React.PointerEvent<HTMLVideoElement>) {
    const { x, y } = normalizePointer(e);
    sendControl({ type: "mouse_move", x, y });
  }

  function handlePointerDown(e: React.PointerEvent<HTMLVideoElement>) {
    e.currentTarget.setPointerCapture(e.pointerId);
    const { x, y } = normalizePointer(e);
    const button = e.button === 2 ? "right" : e.button === 1 ? "middle" : "left";
    sendControl({ type: "mouse_button", action: "down", button, x, y });
  }

  function handlePointerUp(e: React.PointerEvent<HTMLVideoElement>) {
    const { x, y } = normalizePointer(e);
    const button = e.button === 2 ? "right" : e.button === 1 ? "middle" : "left";
    sendControl({ type: "mouse_button", action: "up", button, x, y });
  }

  function handleDoubleClick(e: React.MouseEvent<HTMLVideoElement>) {
    const rect = e.currentTarget.getBoundingClientRect();
    const x = (e.clientX - rect.left) / rect.width;
    const y = (e.clientY - rect.top) / rect.height;
    sendControl({ type: "mouse_dblclick", x, y });
  }

  function handleWheel(e: React.WheelEvent<HTMLVideoElement>) {
    sendControl({ type: "scroll", deltaX: e.deltaX, deltaY: e.deltaY });
  }

  function handleKeyDown(e: React.KeyboardEvent<HTMLDivElement>) {
    e.preventDefault();
    sendControl({ type: "keyboard", action: "down", key: e.key });
  }

  function handleKeyUp(e: React.KeyboardEvent<HTMLDivElement>) {
    sendControl({ type: "keyboard", action: "up", key: e.key });
  }

  function handleTerminate() {
    sigRef.current?.send({ type: "terminate" });
    sigRef.current?.close();
    pcRef.current?.close();
    navigate("/");
  }

  async function handleCopy() {
    await navigator.clipboard.writeText(joinUrl);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  }

  // ── Waiting / connecting state ─────────────────────────────────────────────
  if (status === "waiting" || status === "connecting") {
    return (
      <div className="min-h-screen bg-gray-950 text-white flex flex-col items-center justify-center gap-8 px-4">
        <h2 className="text-2xl font-semibold">
          {status === "waiting" ? "Waiting for participant…" : "Connecting…"}
        </h2>

        <div className="bg-gray-900 border border-gray-800 rounded-xl p-6 flex flex-col gap-4 w-full max-w-md">
          <p className="text-sm text-gray-400">Share this link with the participant:</p>
          <div className="flex items-center gap-2">
            <input
              readOnly
              value={joinUrl}
              className="flex-1 bg-gray-800 text-sm text-gray-200 px-3 py-2 rounded-lg outline-none"
            />
            <button
              onClick={handleCopy}
              className="bg-blue-600 hover:bg-blue-500 text-white text-sm font-medium px-4 py-2 rounded-lg transition-colors"
            >
              {copied ? "Copied!" : "Copy"}
            </button>
          </div>
        </div>

        <button
          onClick={handleTerminate}
          className="text-red-400 hover:text-red-300 text-sm transition-colors"
        >
          End Connection
        </button>
      </div>
    );
  }

  // ── Connected state ────────────────────────────────────────────────────────
  return (
    <div
      className="min-h-screen bg-gray-950 text-white flex flex-col outline-none"
      tabIndex={0}
      onKeyDown={handleKeyDown}
      onKeyUp={handleKeyUp}
    >
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 bg-gray-900 border-b border-gray-800">
        <span className="font-semibold">Remota</span>
        <div className="flex items-center gap-4">
          <span className="flex items-center gap-2 text-sm text-green-400">
            <span className="w-2 h-2 bg-green-400 rounded-full inline-block animate-pulse" />
            Connected
          </span>
          <button
            onClick={handleTerminate}
            className="bg-red-600 hover:bg-red-500 text-white text-sm font-medium px-4 py-1.5 rounded-lg transition-colors"
          >
            End Connection
          </button>
        </div>
      </div>

      {/* Remote screen */}
      <div className="flex-1 flex items-center justify-center bg-black">
        <video
          ref={videoRef}
          autoPlay
          playsInline
          className="w-full h-full object-contain cursor-none select-none"
          onPointerMove={handlePointerMove}
          onPointerDown={handlePointerDown}
          onPointerUp={handlePointerUp}
          onDoubleClick={handleDoubleClick}
          onWheel={handleWheel}
          onContextMenu={(e) => e.preventDefault()}
        />
      </div>
    </div>
  );
}
