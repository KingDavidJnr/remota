// ── Screen capture ────────────────────────────────────────────────────────────
// Platform-gated implementations:
//   Windows — windows-capture (WinRT Graphics Capture API)
//   macOS   — screencapturekit (ScreenCaptureKit, macOS 13+)
//
// Both produce CapturedFrame { data: Vec<u8> BGRA packed, width, height }.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use anyhow::Result;
use tokio::sync::mpsc;

/// A single captured video frame.
#[derive(Debug)]
pub struct CapturedFrame {
    /// Raw BGRA pixel data, packed (no row padding).
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Start screen capture on a background thread.
/// Frames are sent into `frame_tx`.
/// Set `stop` to `true` to request a clean shutdown.
pub fn start(frame_tx: mpsc::Sender<CapturedFrame>, stop: Arc<AtomicBool>) -> Result<()> {
    platform::start(frame_tx, stop)
}

// ══════════════════════════════════════════════════════════════════════════════
// Windows implementation
// ══════════════════════════════════════════════════════════════════════════════

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
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

    struct Flags {
        frame_tx: mpsc::Sender<CapturedFrame>,
        stop: Arc<AtomicBool>,
    }

    struct Handler {
        frame_tx: mpsc::Sender<CapturedFrame>,
        stop: Arc<AtomicBool>,
    }

    impl GraphicsCaptureApiHandler for Handler {
        type Flags = Flags;
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
            ctl: InternalCaptureControl,
        ) -> std::result::Result<(), Self::Error> {
            if self.stop.load(Ordering::Relaxed) {
                ctl.stop();
                return Ok(());
            }

            let width = frame.width();
            let height = frame.height();
            let mut packed = Vec::with_capacity((width * height * 4) as usize);
            let buf = frame.buffer()?;
            buf.as_nopadding_buffer(&mut packed);

            if self.frame_tx.try_send(CapturedFrame { data: packed, width, height }).is_err()
                && self.frame_tx.is_closed()
            {
                ctl.stop();
            }
            Ok(())
        }

        fn on_closed(&mut self) -> std::result::Result<(), Self::Error> {
            info!("[capture] monitor closed");
            Ok(())
        }
    }

    pub fn start(frame_tx: mpsc::Sender<CapturedFrame>, stop: Arc<AtomicBool>) -> Result<()> {
        let monitor = Monitor::primary()?;
        let settings = Settings::new(
            monitor,
            CursorCaptureSettings::WithCursor,
            DrawBorderSettings::WithoutBorder,
            SecondaryWindowSettings::Default,
            MinimumUpdateIntervalSettings::Default,
            DirtyRegionSettings::Default,
            ColorFormat::Bgra8,
            Flags { frame_tx, stop },
        );

        std::thread::spawn(move || {
            if let Err(e) = Handler::start(settings) {
                error!("[capture] fatal: {e}");
            }
        });

        info!("[capture] Windows screen capture started");
        Ok(())
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// macOS implementation
// ══════════════════════════════════════════════════════════════════════════════

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use screencapturekit::{
        cm::CMSampleBuffer,
        prelude::*,
        stream::{
            configuration::pixel_format::PixelFormat, sc_stream::SCStream,
            SCStreamOutputTrait, SCStreamOutputType,
        },
    };
    use tracing::{error, info, warn};

    struct FrameHandler {
        frame_tx: mpsc::Sender<CapturedFrame>,
        stop: Arc<AtomicBool>,
    }

    impl SCStreamOutputTrait for FrameHandler {
        fn did_output_sample_buffer(
            &self,
            sample: CMSampleBuffer,
            of_type: SCStreamOutputType,
        ) {
            if of_type != SCStreamOutputType::Screen {
                return;
            }

            if self.stop.load(Ordering::Relaxed) {
                return;
            }

            let Some(pixel_buf) = sample.pixel_buffer() else { return };
            let Ok(guard) = pixel_buf.lock(screencapturekit::cv::CVPixelBufferLockFlags::READ_ONLY)
            else {
                return;
            };

            let width = guard.width() as u32;
            let height = guard.height() as u32;
            let bytes_per_row = guard.bytes_per_row();

            // SAFETY: guard keeps the pixel buffer locked for the duration of this block.
            let Some(raw) = (unsafe { guard.as_slice() }) else { return };

            // Strip row padding to produce a packed BGRA buffer
            let row_bytes = (width * 4) as usize;
            let mut packed = Vec::with_capacity(row_bytes * height as usize);
            for row in 0..height as usize {
                let start = row * bytes_per_row;
                packed.extend_from_slice(&raw[start..start + row_bytes]);
            }

            let frame = CapturedFrame { data: packed, width, height };

            if self.frame_tx.try_send(frame).is_err() && self.frame_tx.is_closed() {
                // Receiver gone — nothing we can do from a sync callback
            }
        }
    }

    pub fn start(frame_tx: mpsc::Sender<CapturedFrame>, stop: Arc<AtomicBool>) -> Result<()> {
        // Check and request screen recording permission
        if !screencapturekit::has_permission() {
            warn!("[capture] Screen Recording permission not granted — requesting…");
            if !screencapturekit::request_permission() {
                anyhow::bail!(
                    "Screen Recording permission denied. \
                     Go to System Settings → Privacy & Security → Screen & System Audio Recording \
                     and enable Remota Desktop."
                );
            }
        }

        let content = SCShareableContent::get()
            .map_err(|e| anyhow::anyhow!("SCShareableContent::get failed: {e:?}"))?;

        let display = content
            .displays()
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No display found"))?;

        let filter = SCContentFilter::create()
            .with_display(&display)
            .with_excluding_windows(&[])
            .build();

        // Use native resolution of the primary display
        let width = display.width() as u32;
        let height = display.height() as u32;

        let config = SCStreamConfiguration::new()
            .with_width(width as usize)
            .with_height(height as usize)
            .with_pixel_format(PixelFormat::BGRA)
            .with_shows_cursor(true);

        let handler = FrameHandler { frame_tx, stop };

        let mut stream = SCStream::new(&filter, &config);
        stream.add_output_handler(handler, SCStreamOutputType::Screen);

        stream
            .start_capture()
            .map_err(|e| anyhow::anyhow!("start_capture failed: {e:?}"))?;

        info!("[capture] macOS ScreenCaptureKit capture started ({width}×{height})");

        // Keep the stream alive on a background thread until the process exits
        // or `stop` is set. We park the thread rather than busy-loop.
        std::thread::spawn(move || {
            // Park with a periodic wake to check the stop flag
            loop {
                std::thread::sleep(std::time::Duration::from_millis(500));
                if stop.load(Ordering::Relaxed) {
                    if let Err(e) = stream.stop_capture() {
                        error!("[capture] stop_capture: {e:?}");
                    }
                    info!("[capture] macOS capture stopped");
                    break;
                }
            }
        });

        Ok(())
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Unsupported platform stub
// ══════════════════════════════════════════════════════════════════════════════

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod platform {
    use super::*;

    pub fn start(_tx: mpsc::Sender<CapturedFrame>, _stop: Arc<AtomicBool>) -> Result<()> {
        anyhow::bail!("Screen capture is not supported on this platform")
    }
}
