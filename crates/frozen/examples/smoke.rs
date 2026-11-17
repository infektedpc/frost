//! Headless smoke test: prove the engine loop works end to end.
//!
//! Creates an [`Engine`], loads a URL, spins until the load finishes (or a
//! timeout), waits for a painted frame, and writes it as a PNG.
//!
//! Usage:
//!   cargo run -p frozen --example smoke -- [URL] [OUT.png] [W] [H]
//!
//! Defaults: `https://example.org`, `smoke.png` in the current directory,
//! 1280x800. Exit code 0 means a non-blank PNG was written; anything else is a
//! failure. The PNG is the gate: it must exist, be a valid PNG, and not be
//! a single solid color.

use std::time::{Duration, Instant};

use frozen::{Engine, EngineEvent, Frame};

fn write_png(path: &str, frame: &Frame) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&frame.rgba)?;
    writer.finish()?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://example.org".to_string());
    let out = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "smoke.png".to_string());
    let width: u32 = std::env::args()
        .nth(3)
        .and_then(|v| v.parse().ok())
        .unwrap_or(1280);
    let height: u32 = std::env::args()
        .nth(4)
        .and_then(|v| v.parse().ok())
        .unwrap_or(800);

    eprintln!("[smoke] creating engine");
    let engine = Engine::new()?;
    let webview = engine.spawn_webview(&url, width, height)?;
    eprintln!("[smoke] loading {url} at {width}x{height}");

    // Spin until LoadFinished or the load timeout.
    let load_deadline = Instant::now() + Duration::from_secs(120);
    let mut finished = false;
    while Instant::now() < load_deadline {
        engine.spin();
        while let Some(event) = engine.try_recv_event() {
            eprintln!("[smoke] event: {event:?}");
            if matches!(
                event,
                EngineEvent::LoadFinished(_) | EngineEvent::LoadStopped(_)
            ) {
                finished = true;
            }
        }
        if finished {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if !finished {
        eprintln!("[smoke] WARNING: load did not finish within 120s; screenshotting anyway");
    }

    // Wait for a painted frame (FrameReady) so we screenshot content,
    // not the pre-paint blank.
    let frame_deadline = Instant::now() + Duration::from_secs(15);
    let mut saw_frame = false;
    while Instant::now() < frame_deadline {
        engine.spin();
        while let Some(event) = engine.try_recv_event() {
            if matches!(event, EngineEvent::FrameReady(_)) {
                saw_frame = true;
            }
        }
        if saw_frame {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if !saw_frame {
        eprintln!("[smoke] WARNING: no FrameReady within 15s; screenshotting anyway");
    }
    // One last settle pass: a frame can be ready but the newest paint a
    // spin behind.
    for _ in 0..10 {
        engine.spin();
        std::thread::sleep(Duration::from_millis(50));
    }

    let frame = webview
        .render()
        .ok_or("render() returned no frame — nothing painted yet")?;
    eprintln!(
        "[smoke] frame {}x{}, {} distinct colors, solid={}",
        frame.width,
        frame.height,
        frame.distinct_colors(),
        frame.is_solid_color()
    );
    if frame.is_solid_color() {
        return Err("frame is a single solid color — page did not paint".into());
    }
    write_png(&out, &frame)?;
    eprintln!("[smoke] wrote {out}");
    Ok(())
}
