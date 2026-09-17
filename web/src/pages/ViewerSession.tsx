import { useEffect, useRef, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { SignalingClient } from "../lib/signaling";
import { buildIceServers } from "../lib/ice";
import { saveSession, clearSession } from "../lib/session";
import ConfirmDialog from "../components/ConfirmDialog";

// Viewer session — joins a room to watch and hear without sharing anything.
// The controller receives viewer_joined and creates an offer containing
// the participant's stream and the controller's mic.
// The viewer can optionally send mic audio back.

type Status = "connecting" | "active" | "ended";

export default function ViewerSession() {
  const { token } = useParams<{ token: string }>();
  const navigate = useNavigate();

  const sigRef = useRef<SignalingClient | null>(null);
  const pcRef = useRef<RTCPeerConnection | null>(null);
  const videoRef = useRef<HTMLVideoElement>(null);
  const audioRef = useRef<HTMLAudioElement>(null);
  const micTrackRef = useRef<MediaStreamTrack | null>(null);

  const [status, setStatus] = useState<Status>("connecting");
  const [showConfirm, setShowConfirm] = useState(false);
  const [micMuted, setMicMuted] = useState(false);
  const [hasMic, setHasMic] = useState(false);

  useEffect(() => {
    if (!token) return;

    const sig = new SignalingClient();
    sigRef.current = sig;

    saveSession({ token: token!, role: "participant-browser", path: `/viewer-session/${token}` });

    async function start() {
      await sig.connect();

      // Optionally request mic before joining so it's ready to add to the PC
      let micTrack: MediaStreamTrack | null = null;
      try {
        const micStream = await navigator.mediaDevices.getUserMedia({ audio: true, video: false });
        micTrack = micStream.getAudioTracks()[0] ?? null;
        if (micTrack) {
          micTrackRef.current = micTrack;
          setHasMic(true);
        }
      } catch {
        // Mic denied — join as listen-only
      }

      // Join as viewer
      sig.send({ type: "join", token, role: "viewer" });

      sig.onMessage(async (msg) => {
        if (msg.type === "offer") {
          const pc = new RTCPeerConnection({ iceServers: buildIceServers() });
          pcRef.current = pc;

          // Add mic track if available
          if (micTrack) {
            const micStream = new MediaStream([micTrack]);
            pc.addTrack(micTrack, micStream);
          }

          pc.ontrack = (e) => {
            if (e.track.kind === "video") {
              const v = videoRef.current;
              if (!v) return;
              v.srcObject = new MediaStream([e.track]);
              // Video element stays muted — audio is played via a separate element.
              // Muted video autoplay is always allowed by browsers.
              v.play().catch(() => {});
              setStatus("active");
            }

            if (e.track.kind === "audio") {
              const a = audioRef.current;
              if (!a) return;
              a.srcObject = new MediaStream([e.track]);
              // Start muted so autoplay succeeds, then unmute immediately.
              a.muted = true;
              a.volume = 1;
              a.play()
                .then(() => {
                  a.muted = false;
                  a.play().catch(() => {
                    // Unmuted play blocked — wait for gesture
                    a.muted = true;
                    const unlock = () => {
                      a.muted = false;
                      a.play().catch(() => {});
                      document.removeEventListener("click", unlock);
                      document.removeEventListener("touchend", unlock);
                    };
                    document.addEventListener("click", unlock, { once: true });
                    document.addEventListener("touchend", unlock, { once: true });
                  });
                })
                .catch(() => {
                  const unlock = () => {
                    a.muted = false;
                    a.play().catch(() => {});
                    document.removeEventListener("click", unlock);
                    document.removeEventListener("touchend", unlock);
                  };
                  document.addEventListener("click", unlock, { once: true });
                  document.addEventListener("touchend", unlock, { once: true });
                });
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
            }
          };

          await pc.setRemoteDescription(
            new RTCSessionDescription(msg.sdp as RTCSessionDescriptionInit)
          );
          const answer = await pc.createAnswer();
          await pc.setLocalDescription(answer);
          sig.send({ type: "answer", sdp: pc.localDescription });
        }

        if (msg.type === "ice_candidate" && msg.candidate) {
          try {
            await pcRef.current?.addIceCandidate(
              new RTCIceCandidate(msg.candidate as RTCIceCandidateInit)
            );
          } catch { /* ignore */ }
        }

        if (msg.type === "terminate") {
          cleanup();
        }
      });

      sig.onClose(() => cleanup());
    }

    void start();

    return () => {
      cleanup();
    };

    function cleanup() {
      clearSession();
      micTrackRef.current?.stop();
      micTrackRef.current = null;
      sig.close();
      pcRef.current?.close();
      setStatus("ended");
    }
  }, [token]);

  function handleMicToggle() {
    const track = micTrackRef.current;
    if (!track) return;
    track.enabled = !track.enabled;
    setMicMuted(!track.enabled);
  }

  function handleTerminate() {
    setShowConfirm(true);
  }

  function confirmTerminate() {
    clearSession();
    micTrackRef.current?.stop();
    sigRef.current?.close();
    pcRef.current?.close();
    navigate("/");
  }

  if (status === "ended") {
    return (
      <div className="min-h-screen bg-gray-950 text-white flex items-center justify-center">
        <p className="text-gray-400">Session ended.</p>
      </div>
    );
  }

  return (
    <div className="min-h-screen bg-gray-950 text-white flex flex-col">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 bg-gray-900 border-b border-gray-800">
        <span className="font-semibold">Remota</span>
        <div className="flex items-center gap-3">
          <span className="flex items-center gap-2 text-sm text-purple-400">
            <span className={`w-2 h-2 rounded-full inline-block ${
              status === "active" ? "bg-purple-400 animate-pulse" : "bg-yellow-400 animate-pulse"
            }`} />
            {status === "active" ? "Viewing" : "Connecting…"}
          </span>
          {hasMic && (
            <button
              onClick={handleMicToggle}
              className={`text-sm font-medium px-3 py-1.5 rounded-lg transition-colors ${
                micMuted
                  ? "bg-red-900/60 text-red-400 hover:bg-red-900"
                  : "bg-gray-800 text-gray-300 hover:bg-gray-700"
              }`}
              aria-label={micMuted ? "Unmute mic" : "Mute mic"}
            >
              {micMuted ? "🔇" : "🎙️"}
            </button>
          )}
          <button
            onClick={handleTerminate}
            className="bg-red-600 hover:bg-red-500 text-white text-sm font-medium px-4 py-1.5 rounded-lg transition-colors"
          >
            Leave
          </button>
        </div>
      </div>

      {/* Stream */}
      <div className="flex-1 relative flex items-center justify-center bg-black">
        <video
          ref={videoRef}
          autoPlay
          playsInline
          muted
          className="w-full h-full object-contain bg-black"
        />
        <audio
          ref={audioRef}
          autoPlay
          playsInline
          style={{ position: "absolute", width: 0, height: 0, opacity: 0 }}
        />

        {status === "connecting" && (
          <div className="absolute inset-0 flex items-center justify-center bg-gray-950">
            <p className="text-gray-400">Waiting for stream…</p>
          </div>
        )}
      </div>

      {showConfirm && (
        <ConfirmDialog
          message="Are you sure you want to leave this session?"
          confirmLabel="Leave Session"
          onConfirm={confirmTerminate}
          onCancel={() => setShowConfirm(false)}
        />
      )}
    </div>
  );
}
