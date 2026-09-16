// ── Signaling client ──────────────────────────────────────────────────────────
// Connects to the backend WSS /ws endpoint and handles the Remota signaling
// protocol on behalf of the Participant (desktop endpoint).

use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

// ── Wire types ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct JoinMsg<'a> {
    #[serde(rename = "type")]
    pub kind: &'static str, // "join"
    pub token: &'a str,
    pub role: &'static str, // "participant"
}

#[derive(Debug, Serialize)]
pub struct AnswerMsg {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub sdp: Value,
}

#[derive(Debug, Serialize)]
pub struct IceMsg {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub candidate: Value,
}

#[derive(Debug, Serialize)]
pub struct TerminateMsg {
    #[serde(rename = "type")]
    pub kind: &'static str,
}

/// Messages the signaling loop sends back to the main task.
#[derive(Debug)]
pub enum SignalEvent {
    Offer(String),                    // SDP string
    IceCandidate(String),             // JSON-serialised candidate object
    Terminate,
    Disconnected,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Spawns the signaling loop and returns:
/// - `rx`  — channel the caller reads SignalEvents from
/// - `tx`  — channel the caller uses to send outbound JSON strings
pub async fn connect(
    ws_url: &str,
    token: &str,
) -> Result<(mpsc::Receiver<SignalEvent>, mpsc::Sender<String>)> {
    let (ws_stream, _) = connect_async(ws_url).await?;
    info!("[signaling] connected to {ws_url}");

    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // Join as participant
    let join_msg = serde_json::to_string(&JoinMsg {
        kind: "join",
        token,
        role: "participant",
    })?;
    ws_tx.send(Message::Text(join_msg.into())).await?;
    info!("[signaling] sent join for token={token}");

    let (event_tx, event_rx) = mpsc::channel::<SignalEvent>(32);
    let (out_tx, mut out_rx) = mpsc::channel::<String>(32);

    tokio::spawn(async move {
        loop {
            tokio::select! {
                // Inbound from server
                msg = ws_rx.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            handle_inbound(text.as_str(), &event_tx).await;
                        }
                        Some(Ok(_)) => {} // ignore binary / ping frames
                        Some(Err(e)) => {
                            error!("[signaling] recv error: {e}");
                            let _ = event_tx.send(SignalEvent::Disconnected).await;
                            break;
                        }
                        None => {
                            info!("[signaling] server closed connection");
                            let _ = event_tx.send(SignalEvent::Disconnected).await;
                            break;
                        }
                    }
                }
                // Outbound from main task
                Some(text) = out_rx.recv() => {
                    if let Err(e) = ws_tx.send(Message::Text(text.into())).await {
                        error!("[signaling] send error: {e}");
                        break;
                    }
                }
            }
        }
    });

    Ok((event_rx, out_tx))
}

async fn handle_inbound(text: &str, tx: &mpsc::Sender<SignalEvent>) {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            warn!("[signaling] invalid JSON: {e}");
            return;
        }
    };

    let kind = v["type"].as_str().unwrap_or("");
    debug!("[signaling] ← {kind}");

    match kind {
        "offer" => {
            if let Some(sdp_val) = v.get("sdp") {
                // sdp may be a nested object or a plain string
                let sdp_str = if sdp_val.is_object() {
                    sdp_val["sdp"].as_str().unwrap_or("").to_owned()
                } else {
                    sdp_val.as_str().unwrap_or("").to_owned()
                };
                let _ = tx.send(SignalEvent::Offer(sdp_str)).await;
            }
        }
        "ice_candidate" => {
            if let Some(c) = v.get("candidate") {
                let s = c.to_string();
                let _ = tx.send(SignalEvent::IceCandidate(s)).await;
            }
        }
        "terminate" => {
            let _ = tx.send(SignalEvent::Terminate).await;
        }
        "error" => {
            error!("[signaling] server error: {}", v["message"]);
        }
        other => {
            debug!("[signaling] unhandled message type: {other}");
        }
    }
}

// ── Helper: send answer back via out_tx ───────────────────────────────────────

pub fn make_answer_msg(sdp_obj: &webrtc::peer_connection::sdp::session_description::RTCSessionDescription) -> Result<String> {
    let msg = serde_json::json!({
        "type": "answer",
        "sdp": {
            "type": "answer",
            "sdp": sdp_obj.sdp
        }
    });
    Ok(msg.to_string())
}

pub fn make_ice_msg(candidate_json: &str) -> Result<String> {
    let c: Value = serde_json::from_str(candidate_json)
        .unwrap_or(Value::String(candidate_json.to_owned()));
    let msg = serde_json::json!({
        "type": "ice_candidate",
        "candidate": c
    });
    Ok(msg.to_string())
}

pub fn make_terminate_msg() -> String {
    r#"{"type":"terminate"}"#.to_owned()
}
