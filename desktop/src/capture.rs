// ── Screen capture ────────────────────────────────────────────────────────────
// Captures the primary monitor using the Windows.Graphics.Capture WinRT API
// (via the `windows-capture` crate) and pushes raw BGRA frames into a channel
// for the WebRTC encoding pipeline.

use anyhow::Result;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::mpsc;
use tracing::{error, info};
use windows_capture::{
    capture::{Context, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    monitor::Monitor,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings,
        DrawBorderSettings, MinimumUpdateIntervalSettings,
        SecondaryWindowSettings, Settings,
    },
};

/// A single captured video frame.
#[derive(Debug)]
pub struct CapturedFrame {
    /// Raw BGRA pixel data (no padding, packed).
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Starts screen capture on a background thread.
///
/// Returns:
/// - `rx` — the receiver that yields `CapturedFrame`s
/// - `stop` — set to `true` to request capture stop
pub fn start(
    frame_tx: mpsc::Sender<CapturedFrame>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let monitor = Monitor::primary()?;

    let settings = Settings::new(
        monitor,
        CursorCaptureSettings::WithCursor,
        DrawBorderSettings::Default,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        CaptureFlags { frame_tx, stop },
    );

    // start_free_threaded() spawns an internal OS thread and returns immediately.
    // We deliberately don't await it — the thread runs until `stop` is set or
    // the monitor disappears.
    std::thread::spawn(move || {
        if let Err(e) = CaptureHandler::start(settings) {
            error!("[capture] fatal error: {e}");
        }
    });

    info!("[capture] screen capture started (primary monitor)");
    Ok(())
}

// ── Internal handler ──────────────────────────────────────────────────────────

struct CaptureFlags {
    frame_tx: mpsc::Sender<CapturedFrame>,
    stop: Arc<AtomicBool>,
}

struct CaptureHandler {
    frame_tx: mpsc::Sender<CapturedFrame>,
    stop: Arc<AtomicBool>,
}

impl GraphicsCaptureApiHandler for CaptureHandler {
    type Flags = CaptureFlags;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(ctx: Context<Self::Flags>) -> std::result::Result<Self, Self::Error> {
        Ok(Self {
            frame_tx: ctx.flags.frame_tx,
            stop: ctx.flags.stop,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        capture_control: InternalCaptureControl,
    ) -> std::result::Result<(), Self::Error> {
        // Honour external stop request
        if self.stop.load(Ordering::Relaxed) {
            capture_control.stop();
            return Ok(());
        }

        let width = frame.width();
        let height = frame.height();

        let mut packed: Vec<u8> = Vec::with_capacity((width * height * 4) as usize);
        let buf = frame.buffer()?;
        buf.as_nopadding_buffer(&mut packed);

        let cf = CapturedFrame {
            data: packed,
            width,
            height,
        };

        // Non-blocking send: drop frame if the encoding pipeline is busy.
        // This prevents a frame backlog from building up.
        if self.frame_tx.try_send(cf).is_err() {
            // receiver is full or gone — if gone, stop
            if self.frame_tx.is_closed() {
                capture_control.stop();
            }
        }

        Ok(())
    }

    fn on_closed(&mut self) -> std::result::Result<(), Self::Error> {
        info!("[capture] monitor closed");
        Ok(())
    }
}
