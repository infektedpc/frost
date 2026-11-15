//! Round 6 diagnostic probe: does the ENGINE produce blank frames
//! during load and scroll, or is every `render()` a full frame?
//!
//! Loads a URL, renders DURING the load (like the app would if the
//! `awaiting_load` gate were not there), then scrolls with wheel events
//! and renders after each step, logging per-frame stats:
//! solid color? distinct colors? mean brightness? center pixel?
//! A solid-white frame mid-load or mid-scroll is the flash, engine-side.
//! All-full frames point the finger at the app/iced upload path.
//!
//! Usage:
//!   cargo run -p frost-engine --example frameprobe -- [URL] [STEPS]
//!
//! Defaults: file://.../r6-scroll.html in the harness dir, 12 scroll steps.

use std::time::{Duration, Instant};

use frost_engine::{Engine, EngineEvent, Frame, FrostInputEvent};

fn stats(tag: &str, frame: &Frame) {
    let n = (frame.width as usize) * (frame.height as usize);
    let mut sum_r: u64 = 0;
    let mut sum_g: u64 = 0;
    let mut sum_b: u64 = 0;
    for px in frame.rgba.chunks_exact(4) {
        sum_r += px[0] as u64;
        sum_g += px[1] as u64;
        sum_b += px[2] as u64;
    }
    let cx = (frame.width / 2) as usize;
    let cy = (frame.height / 2) as usize;
    let center = &frame.rgba[(cy * frame.width as usize + cx) * 4..][..4];
    let top = &frame.rgba[(10 * frame.width as usize + 10) * 4..][..4];
    println!(
        "[probe] {tag}: {}x{} solid={} distinct={} mean=({},{},{}) center=rgba({},{},{},{}) topleft=rgba({},{},{},{})",
        frame.width,
        frame.height,
        frame.is_solid_color(),
        frame.distinct_colors(),
        sum_r / n as u64,
        sum_g / n as u64,
        sum_b / n as u64,
        center[0],
        center[1],
        center[2],
        center[3],
        top[0],
        top[1],
        top[2],
        top[3],
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::args().nth(1).unwrap_or_else(|| {
        format!(
            "file://{}/workspace/harness-tmp/frost/r6-scroll.html",
            std::env::var("HOME").unwrap()
        )
    });
    let steps: usize = std::env::args()
        .nth(2)
        .and_then(|v| v.parse().ok())
        .unwrap_or(12);

    let engine = Engine::new()?;
    let webview = engine.spawn_webview(&url, 1280, 684)?;
    println!("[probe] loading {url} at 1280x684");

    // Phase 1: render DURING the load, every ~200 ms, until LoadFinished.
    // This is what the app would show without its awaiting_load gate.
    let load_deadline = Instant::now() + Duration::from_secs(120);
    let mut finished = false;
    let mut tick = 0;
    while Instant::now() < load_deadline && !finished {
        engine.spin();
        while let Some(event) = engine.try_recv_event() {
            if matches!(event, EngineEvent::LoadFinished | EngineEvent::LoadStopped) {
                finished = true;
            }
        }
        tick += 1;
        if tick % 20 == 0 {
            match webview.render() {
                Some(frame) => stats(&format!("load-tick{tick}"), &frame),
                None => println!("[probe] load-tick{tick}: render() -> None"),
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    println!("[probe] load finished={finished} after {tick} ticks");
    // Settle, then one post-load frame.
    for _ in 0..10 {
        engine.spin();
        std::thread::sleep(Duration::from_millis(50));
    }
    match webview.render() {
        Some(frame) => stats("post-load", &frame),
        None => println!("[probe] post-load: render() -> None"),
    }

    // Phase 2: scroll down in steps; render after each wheel event and
    // a short settle. Alternate the sign if the first direction does not
    // move the page (center pixel stays the same color).
    for step in 0..steps {
        webview.send_input(FrostInputEvent::Wheel {
            dx: 0.0,
            dy: -480.0,
            x: 640.0,
            y: 300.0,
        });
        for _ in 0..20 {
            engine.spin();
            std::thread::sleep(Duration::from_millis(50));
        }
        match webview.render() {
            Some(frame) => stats(&format!("scroll-down-{step}"), &frame),
            None => println!("[probe] scroll-down-{step}: render() -> None"),
        }
    }
    // Phase 3: scroll back up the same way.
    for step in 0..steps {
        webview.send_input(FrostInputEvent::Wheel {
            dx: 0.0,
            dy: 480.0,
            x: 640.0,
            y: 300.0,
        });
        for _ in 0..20 {
            engine.spin();
            std::thread::sleep(Duration::from_millis(50));
        }
        match webview.render() {
            Some(frame) => stats(&format!("scroll-up-{step}"), &frame),
            None => println!("[probe] scroll-up-{step}: render() -> None"),
        }
    }
    println!("[probe] done");
    Ok(())
}
