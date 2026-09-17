import { useEffect, useRef, useState, useCallback } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { SignalingClient } from "../lib/signaling";
import { buildIceServers } from "../lib/ice";
import { normalizePointer } from "../lib/control";
import { saveSession, clearSession } from "../lib/session";
import ConfirmDialog from "../components/ConfirmDialog";

// ── Touch gesture constants ───────────────────────────────────────────────────
const LONG_PRESS_MS = 600;
const DOUBLE_TAP_MS = 300;
const DRAG_THRESHOLD_PX = 8;

export default function ControllerRoom() {
  const { token } = useParams<{ token: string }>();
  const navigate = useNavigate();

  const sigRef = useRef<SignalingClient | null>(null);
  const pcRef = useRef<RTCPeerConnection | null>(null);
  const dcRef = useRef<RTCDataChannel | null>(null);
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const keyInputRef = useRef<HTMLInputElement>(null);
  const micTrackRef = useRef<MediaStreamTrack | null>(null);
  const viewerPCsRef = useRef<Map<string, RTCPeerConnection>>(new Map());

  const [status, setStatus] = useState<"waiting" | "connecting" | "connected" | "ended">("waiting");
  const [copied, setCopied] = useState(false);
  const [showKeyboard, setShowKeyboard] = useState(false);
  const [viewOnly, setViewOnly] = useState(false);
  const [showConfirm, setShowConfirm] = useState(false);
  const [micMuted, setMicMuted] = useState(true); // always start muted
  const [hasMic, setHasMic] = useState(false);
  const [debugLog, setDebugLog] = useState<string[]>([]);

  function log(msg: string) {
    setDebugLog((p) => [...p.slice(-8), `${new Date().toLocaleTimeString()}: ${msg}`]);
  }

  const joinUrl = `${window.location.origin}/join/${token ?? ""}`;

  // ── Touch gesture state (refs so handlers don't stale-close over them) ──────
  const touchState = useRef({
    startX: 0,
    startY: 0,
    startTime: 0,
    dragging: false,
    longPressTimer: null as ReturnType<typeof setTimeout> | null,
    lastTapTime: 0,
  });

  // ── WebRTC + signaling setup ──────────────────────────────────────────────
  useEffect(() => {
    if (!token) return;

    const sig = new SignalingClient();
    sigRef.current = sig;
    let pc: RTCPeerConnection;

    async function start() {
      await sig.connect();
      log("signaling connected");
      sig.send({ type: "join", token, role: "controller" });

      // Save session so refresh can offer to resume
      saveSession({ token: token!, role: "controller", path: `/wait/${token}` });

      sig.onMessage(async (msg) => {
        if (msg.type === "participant_joined") {
          log("participant joined");
          setStatus("connecting");
          await startOffer();
        }
        if (msg.type === "answer") {
          const fromId = msg.fromId as string | undefined;
          if (fromId) {
            const vpc = viewerPCsRef.current.get(fromId);
            if (vpc) {
              await vpc.setRemoteDescription(
                new RTCSessionDescription(msg.sdp as RTCSessionDescriptionInit)
              );
            }
          } else {
            await pcRef.current?.setRemoteDescription(
              new RTCSessionDescription(msg.sdp as RTCSessionDescriptionInit)
            );
          }
        }
        if (msg.type === "ice_candidate" && msg.candidate) {
          const fromId = msg.fromId as string | undefined;
          if (fromId) {
            const vpc = viewerPCsRef.current.get(fromId);
            if (vpc) {
              await vpc.addIceCandidate(
                new RTCIceCandidate(msg.candidate as RTCIceCandidateInit)
              );
            }
          } else {
            try {
              await pcRef.current?.addIceCandidate(
                new RTCIceCandidate(msg.candidate as RTCIceCandidateInit)
              );
            } catch { /* ignore stale candidates */ }
          }
        }
        if (msg.type === "active") {
          setStatus("connected");
        }
        if (msg.type === "viewer_joined") {
          const viewerId = msg.viewerId as string;
          log(`viewer joined: ${viewerId}`);
          await startViewerOffer(viewerId);
        }
        if (msg.type === "viewer_left") {
          const viewerId = msg.viewerId as string;
          const vpc = viewerPCsRef.current.get(viewerId);
          vpc?.close();
          viewerPCsRef.current.delete(viewerId);
        }
        // Only end on explicit terminate — participant_left is a transient WS drop,
        // handled by the server's 15s grace period
        if (msg.type === "terminate") {
          cleanup();
          navigate("/");
        }
      });
    }

    async function startOffer() {
      pc = new RTCPeerConnection({ iceServers: buildIceServers() });
      pcRef.current = pc;

      const dc = pc.createDataChannel("control");
      dcRef.current = dc;
      dc.onopen = () => setViewOnly(false);

      // Receive screen (video) and participant mic (audio)
      pc.addTransceiver("video", { direction: "recvonly" });
      pc.addTransceiver("audio", { direction: "sendrecv" });

      pc.ontrack = (e) => {
        if (e.track.kind === "video") {
          const v = videoRef.current;
          if (v) {
            v.srcObject = new MediaStream([e.track]);
            v.muted = true;
            v.play().catch(() => {});
          }
          setStatus("connected");
          setTimeout(() => {
            if (dcRef.current?.readyState !== "open") setViewOnly(true);
          }, 3000);
        }
        if (e.track.kind === "audio") {
          const a = audioRef.current;
          if (a) {
            a.srcObject = new MediaStream([e.track]);
            a.play().catch(() => {});
          }
        }
      };

      pc.onicecandidate = (e) => {
        if (e.candidate) {
          sig.send({ type: "ice_candidate", candidate: e.candidate.toJSON() });
        }
      };

      pc.onconnectionstatechange = () => {
        const state = pc.connectionState;
        log(`conn: ${state}`);
        if (state === "connected") setStatus("connected");
        if (state === "failed") {
          // Check pcRef status to avoid stale closure
          const currentStatus = pcRef.current === pc ? "check" : "gone";
          if (currentStatus === "gone") return;
          cleanup();
          navigate("/");
        }
      };

      const offer = await pc.createOffer();
      await pc.setLocalDescription(offer);
      sig.send({ type: "offer", sdp: pc.localDescription });
    }

    // ── Viewer offer ──────────────────────────────────────────────────────────
    // When a viewer joins we create a separate PC just for them.
    // They receive the same stream the participant is sending us (re-broadcast).
    // We also send our mic to them.
    async function startViewerOffer(viewerId: string) {
      const vpc = new RTCPeerConnection({ iceServers: buildIceServers() });
      viewerPCsRef.current.set(viewerId, vpc);

      // Re-broadcast the participant's stream to the viewer
      const participantStream = videoRef.current?.srcObject as MediaStream | null;
      if (participantStream) {
        for (const track of participantStream.getTracks()) {
          vpc.addTrack(track, participantStream);
        }
      }

      // Also send our mic to the viewer
      if (micTrackRef.current) {
        const micStream = new MediaStream([micTrackRef.current]);
        vpc.addTrack(micTrackRef.current, micStream);
      }

      vpc.onicecandidate = (e) => {
        if (e.candidate) {
          sig.send({
            type: "ice_candidate",
            candidate: e.candidate.toJSON(),
            targetId: viewerId,
          });
        }
      };

      vpc.onconnectionstatechange = () => {
        if (
          vpc.connectionState === "failed" ||
          vpc.connectionState === "closed"
        ) {
          viewerPCsRef.current.delete(viewerId);
        }
      };

      const offer = await vpc.createOffer();
      await vpc.setLocalDescription(offer);
      sig.send({ type: "offer", sdp: vpc.localDescription, viewerId });
    }

    void start();
    return () => cleanup();

    function cleanup() {
      clearSession();
      micTrackRef.current?.stop();
      micTrackRef.current = null;
      // Close all viewer PCs
      viewerPCsRef.current.forEach((vpc) => vpc.close());
      viewerPCsRef.current.clear();
      sig.close();
      pc?.close();
      setStatus("ended");
    }
  }, [token, navigate]);

  // ── Mic toggle ────────────────────────────────────────────────────────────
  const handleMicToggle = async () => {
    if (!micTrackRef.current) {
      try {
        const micStream = await navigator.mediaDevices.getUserMedia({ audio: true, video: false });
        const track = micStream.getAudioTracks()[0];
        if (track && pcRef.current) {
          track.enabled = false; // start muted
          pcRef.current.addTrack(track, micStream);
          micTrackRef.current = track;
          setHasMic(true);
          setMicMuted(true);
        }
      } catch { /* denied */ }
      return;
    }
    const track = micTrackRef.current;
    track.enabled = !track.enabled;
    setMicMuted(!track.enabled);
  };

  // -- Control message sender ────────────────────────────────────────────────
  const sendControl = useCallback((msg: object) => {
    const dc = dcRef.current;
    if (dc?.readyState === "open") {
      dc.send(JSON.stringify(msg));
    }
  }, []);

  // ── Normalise a touch position relative to the video element ─────────────
  function normTouch(touch: React.Touch, el: HTMLElement) {
    const rect = el.getBoundingClientRect();
    return {
      x: (touch.clientX - rect.left) / rect.width,
      y: (touch.clientY - rect.top) / rect.height,
    };
  }

  // ── Touch handlers ────────────────────────────────────────────────────────

  function handleTouchStart(e: React.TouchEvent<HTMLVideoElement>) {
    e.preventDefault(); // prevent ghost mouse events on Android

    const ts = touchState.current;

    if (e.touches.length === 1) {
      const t = e.touches[0]!;
      const { x, y } = normTouch(t, e.currentTarget);

      ts.startX = t.clientX;
      ts.startY = t.clientY;
      ts.startTime = Date.now();
      ts.dragging = false;

      // Schedule long-press → right click
      ts.longPressTimer = setTimeout(() => {
        ts.longPressTimer = null;
        sendControl({ type: "mouse_button", action: "down", button: "right", x, y });
        sendControl({ type: "mouse_button", action: "up", button: "right", x, y });
      }, LONG_PRESS_MS);
    }

    if (e.touches.length === 2) {
      // Two fingers starting — cancel any single-finger long press
      cancelLongPress();
    }
  }

  function handleTouchMove(e: React.TouchEvent<HTMLVideoElement>) {
    e.preventDefault();
    const ts = touchState.current;

    if (e.touches.length === 1) {
      const t = e.touches[0]!;
      const dx = t.clientX - ts.startX;
      const dy = t.clientY - ts.startY;

      if (!ts.dragging && Math.hypot(dx, dy) > DRAG_THRESHOLD_PX) {
        // Crossed drag threshold — cancel long press and begin drag
        cancelLongPress();
        ts.dragging = true;
        const { x, y } = normTouch(t, e.currentTarget);
        sendControl({ type: "mouse_button", action: "down", button: "left", x, y });
      }

      if (ts.dragging) {
        const { x, y } = normTouch(t, e.currentTarget);
        sendControl({ type: "mouse_move", x, y });
      }
    }

    if (e.touches.length === 2) {
      // Two-finger scroll
      // We track the midpoint delta between move events via a stored prev position
      const prev = touchState.current as typeof ts & { prevMidY?: number; prevMidX?: number };
      const t0 = e.touches[0]!;
      const t1 = e.touches[1]!;
      const midX = (t0.clientX + t1.clientX) / 2;
      const midY = (t0.clientY + t1.clientY) / 2;

      if (prev.prevMidX !== undefined && prev.prevMidY !== undefined) {
        const deltaX = (prev.prevMidX - midX) * 2;
        const deltaY = (prev.prevMidY - midY) * 2;
        sendControl({ type: "scroll", deltaX, deltaY });
      }

      prev.prevMidX = midX;
      prev.prevMidY = midY;
    }
  }

  function handleTouchEnd(e: React.TouchEvent<HTMLVideoElement>) {
    e.preventDefault();
    const ts = touchState.current;
    const prev = ts as typeof ts & { prevMidX?: number; prevMidY?: number };

    // Clear two-finger scroll state
    if (e.touches.length < 2) {
      prev.prevMidX = undefined;
      prev.prevMidY = undefined;
    }

    if (e.changedTouches.length === 1 && e.touches.length === 0) {
      const t = e.changedTouches[0]!;
      const { x, y } = normTouch(t, e.currentTarget);

      if (ts.dragging) {
        // End drag
        sendControl({ type: "mouse_button", action: "up", button: "left", x, y });
        ts.dragging = false;
        cancelLongPress();
        return;
      }

      cancelLongPress();

      const elapsed = Date.now() - ts.startTime;
      const dx = t.clientX - ts.startX;
      const dy = t.clientY - ts.startY;
      const moved = Math.hypot(dx, dy) > DRAG_THRESHOLD_PX;

      if (!moved && elapsed < LONG_PRESS_MS) {
        // It's a tap — check for double tap
        const now = Date.now();
        const sinceLast = now - ts.lastTapTime;

        if (sinceLast < DOUBLE_TAP_MS) {
          // Double tap → double click
          sendControl({ type: "mouse_dblclick", x, y });
          ts.lastTapTime = 0;
        } else {
          // Single tap → left click
          sendControl({ type: "mouse_button", action: "down", button: "left", x, y });
          sendControl({ type: "mouse_button", action: "up", button: "left", x, y });
          ts.lastTapTime = now;
        }
      }
    }
  }

  function cancelLongPress() {
    const ts = touchState.current;
    if (ts.longPressTimer !== null) {
      clearTimeout(ts.longPressTimer);
      ts.longPressTimer = null;
    }
  }

  // ── Desktop mouse handlers ────────────────────────────────────────────────

  function handlePointerMove(e: React.PointerEvent<HTMLVideoElement>) {
    if (e.pointerType === "touch") return; // handled by touch events
    const { x, y } = normalizePointer(e);
    sendControl({ type: "mouse_move", x, y });
  }

  function handlePointerDown(e: React.PointerEvent<HTMLVideoElement>) {
    if (e.pointerType === "touch") return;
    e.currentTarget.setPointerCapture(e.pointerId);
    const { x, y } = normalizePointer(e);
    const button = e.button === 2 ? "right" : e.button === 1 ? "middle" : "left";
    sendControl({ type: "mouse_button", action: "down", button, x, y });
  }

  function handlePointerUp(e: React.PointerEvent<HTMLVideoElement>) {
    if (e.pointerType === "touch") return;
    const { x, y } = normalizePointer(e);
    const button = e.button === 2 ? "right" : e.button === 1 ? "middle" : "left";
    sendControl({ type: "mouse_button", action: "up", button, x, y });
  }

  function handleDoubleClick(e: React.MouseEvent<HTMLVideoElement>) {
    if ("ontouchstart" in window) return; // handled by touch double-tap
    const rect = e.currentTarget.getBoundingClientRect();
    sendControl({
      type: "mouse_dblclick",
      x: (e.clientX - rect.left) / rect.width,
      y: (e.clientY - rect.top) / rect.height,
    });
  }

  function handleWheel(e: React.WheelEvent<HTMLVideoElement>) {
    sendControl({ type: "scroll", deltaX: e.deltaX, deltaY: e.deltaY });
  }

  // ── Keyboard handlers ─────────────────────────────────────────────────────

  function handleKeyDown(e: React.KeyboardEvent<HTMLDivElement>) {
    e.preventDefault();
    sendControl({ type: "keyboard", action: "down", key: e.key });
  }

  function handleKeyUp(e: React.KeyboardEvent<HTMLDivElement>) {
    sendControl({ type: "keyboard", action: "up", key: e.key });
  }

  // Soft keyboard: forward printable characters typed via Android IME
  function handleSoftInput(e: React.FormEvent<HTMLInputElement>) {
    const input = e.currentTarget;
    const val = input.value;
    if (!val) return;
    for (const char of val) {
      sendControl({ type: "keyboard", action: "down", key: char });
      sendControl({ type: "keyboard", action: "up", key: char });
    }
    input.value = "";
  }

  function handleSoftKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    // Forward special keys that don't produce input events
    const special = [
      "Backspace", "Delete", "Enter", "Tab", "Escape",
      "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight",
      "Home", "End", "PageUp", "PageDown",
    ];
    if (special.includes(e.key)) {
      e.preventDefault();
      sendControl({ type: "keyboard", action: "down", key: e.key });
      sendControl({ type: "keyboard", action: "up", key: e.key });
    }
  }

  function toggleKeyboard() {
    setShowKeyboard((v) => {
      const next = !v;
      if (next) {
        setTimeout(() => keyInputRef.current?.focus(), 50);
      }
      return next;
    });
  }

  // ── Session control ───────────────────────────────────────────────────────

  function handleTerminate() {
    setShowConfirm(true);
  }

  function confirmTerminate() {
    clearSession();
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

  const isWaiting = status === "waiting" || status === "connecting";

  return (
    <>
      {isWaiting ? (
        <div className="min-h-screen bg-gray-950 text-white flex flex-col items-center justify-center gap-8 px-4">
          <h2 className="text-2xl font-semibold">
            {status === "waiting" ? "Waiting for participant..." : "Connecting..."}
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
      ) : (
        <div
          className="min-h-screen bg-gray-950 text-white flex flex-col outline-none"
          tabIndex={0}
          onKeyDown={handleKeyDown}
          onKeyUp={handleKeyUp}
        >
          <div className="flex items-center justify-between px-4 py-3 bg-gray-900 border-b border-gray-800">
            <span className="font-semibold">Remota</span>
            <div className="flex items-center gap-3">
              {viewOnly ? (
                <span className="flex items-center gap-2 text-sm text-blue-400">
                  <span className="w-2 h-2 bg-blue-400 rounded-full inline-block animate-pulse" />
                  View only
                </span>
              ) : (
                <span className="flex items-center gap-2 text-sm text-green-400">
                  <span className="w-2 h-2 bg-green-400 rounded-full inline-block animate-pulse" />
                  Connected
                </span>
              )}
              {!viewOnly && (
                <button
                  onClick={toggleKeyboard}
                  className={`text-sm font-medium px-3 py-1.5 rounded-lg transition-colors ${
                    showKeyboard ? "bg-blue-600 text-white" : "bg-gray-800 text-gray-300 hover:bg-gray-700"
                  }`}
                  aria-label="Toggle keyboard"
                >
                  Keyboard
                </button>
              )}
              <button
                onClick={handleMicToggle}
                className={`text-sm font-medium px-3 py-1.5 rounded-lg transition-colors ${
                  !hasMic
                    ? "bg-gray-800 text-gray-500 hover:bg-gray-700"
                    : micMuted
                    ? "bg-red-900/60 text-red-400 hover:bg-red-900"
                    : "bg-gray-800 text-gray-300 hover:bg-gray-700"
                }`}
              >
                {micMuted ? "Mic Off" : "Mic On"}
              </button>
              <button
                onClick={handleTerminate}
                className="bg-red-600 hover:bg-red-500 text-white text-sm font-medium px-4 py-1.5 rounded-lg transition-colors"
              >
                End
              </button>
            </div>
          </div>

          <div className="flex-1 relative flex items-center justify-center bg-black">
            <video
              ref={(el) => { videoRef.current = el; }}
              autoPlay
              playsInline
              muted
              className="w-full h-full object-contain select-none touch-none bg-black"
              style={{ cursor: viewOnly ? "default" : "none" }}
              onPointerMove={viewOnly ? undefined : handlePointerMove}
              onPointerDown={viewOnly ? undefined : handlePointerDown}
              onPointerUp={viewOnly ? undefined : handlePointerUp}
              onDoubleClick={viewOnly ? undefined : handleDoubleClick}
              onWheel={viewOnly ? undefined : handleWheel}
              onContextMenu={(e) => e.preventDefault()}
              onTouchStart={viewOnly ? undefined : handleTouchStart}
              onTouchMove={viewOnly ? undefined : handleTouchMove}
              onTouchEnd={viewOnly ? undefined : handleTouchEnd}
              onTouchCancel={viewOnly ? undefined : handleTouchEnd}
            />
            <audio
              ref={(el) => { audioRef.current = el; }}
              autoPlay
              playsInline
              className="hidden"
            />
          </div>

          <input
            ref={keyInputRef}
            type="text"
            inputMode="text"
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
            spellCheck={false}
            className="absolute opacity-0 w-0 h-0 pointer-events-none"
            aria-hidden="true"
            onInput={handleSoftInput}
            onKeyDown={handleSoftKeyDown}
            onBlur={() => setShowKeyboard(false)}
          />
        </div>
      )}

      {showConfirm && (
        <ConfirmDialog
          message="Are you sure you want to end this connection? The session will be terminated for both parties."
          confirmLabel="End Connection"
          onConfirm={confirmTerminate}
          onCancel={() => setShowConfirm(false)}
        />
      )}

      {debugLog.length > 0 && (
        <div className="fixed bottom-0 left-0 right-0 z-50 bg-black/90 text-green-400 text-xs font-mono p-2 pointer-events-none">
          {debugLog.map((l, i) => <div key={i}>{l}</div>)}
        </div>
      )}
    </>
  );
}
