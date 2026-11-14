//! frost-engine: Servo embedding wrapper for the Frost browser.
//!
//! This crate owns everything Servo-shaped: creating the engine, owning one
//! [`FrostWebView`] per tab, pumping the event loop, turning Servo
//! notifications into [`EngineEvent`]s on a channel, and reading rendered
//! frames out of the software rendering context as RGBA pixels.
//!
//! What it deliberately does NOT do (Round 1):
//!
//! * No window, no toolkit — the libcosmic UI lives in the `frost` crate
//!   and consumes frames through [`FrostWebView::render`]. The engine is
//!   headless by design, which is what makes the smoke test possible.
//! * No input mapping — [`FrostInputEvent`] defines the event shapes the UI
//!   will send (winit/libcosmic keys in Round 2). Mouse and wheel events
//!   are already forwarded; keyboard events go through
//!   [`KeyboardEvent::new_without_event`], which needs the `+8` evdev
//!   adjustment only for Wayland seat input — Frost keys come from
//!   winit/libcosmic, so that fix does not apply here.
//! * `stop()` landed in Round 2 as fork patch F-001 (see `FORK.md`):
//!   [`FrostWebView::stop`] sends the stop request to the script thread
//!   of the active pipeline.
//!
//! Threading: everything here runs on ONE thread (the thread that creates
//! the [`Engine`]). Servo's embedder API expects `spin_event_loop` on the
//! creating thread; the UI drives it from its own loop in Round 2.

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use dpi::PhysicalSize;
use servo::{
    Code, DeviceIntPoint, DeviceIntRect, DeviceIntSize, DevicePoint, EventLoopWaker, InputEvent,
    Key, KeyState, KeyboardEvent, LoadStatus, Location, Modifiers, MouseButton,
    MouseButtonAction, MouseButtonEvent, MouseMoveEvent, PrefValue, RenderingContext,
    Servo, ServoBuilder, SoftwareRenderingContext, Theme, WebView, WebViewBuilder,
    WebViewDelegate, WheelDelta, WheelEvent, WheelMode,
};
use url::Url;

/// Errors from the engine.
#[derive(Debug)]
pub enum EngineError {
    /// Servo itself failed (context creation, bad preference, …).
    Servo(String),
    /// The URL did not parse.
    InvalidUrl(String),
    /// The operation is not available in this Servo version yet
    /// (see the `stop()` note above).
    Unsupported(String),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::Servo(msg) => write!(f, "servo error: {msg}"),
            EngineError::InvalidUrl(msg) => write!(f, "invalid URL: {msg}"),
            EngineError::Unsupported(msg) => write!(f, "unsupported: {msg}"),
        }
    }
}

impl std::error::Error for EngineError {}

/// A rendered frame: RGBA8 pixels, row-major, top-left origin.
/// `render()` returns `None` when no frame is available yet.
#[derive(Debug, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Frame {
    /// True when every pixel is the same color (used by the smoke test to
    /// catch a blank/never-painted frame).
    pub fn is_solid_color(&self) -> bool {
        self.rgba
            .chunks_exact(4)
            .all(|px| px == &self.rgba[0..4])
    }

    /// Number of distinct RGBA colors in the frame.
    pub fn distinct_colors(&self) -> usize {
        use std::collections::HashSet;
        self.rgba
            .chunks_exact(4)
            .map(|px| [px[0], px[1], px[2], px[3]])
            .collect::<HashSet<_>>()
            .len()
    }
}

/// Events the engine reports to the UI. Delivered over a channel so the UI
/// never blocks inside a Servo callback.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    TitleChanged(Option<String>),
    UrlChanged(Url),
    LoadStarted,
    LoadFinished,
    /// The load was stopped before completing (0.8.0's `LoadStatus::Stopped`).
    LoadStopped,
    FrameReady,
    Crashed(String),
}

/// Input events the UI sends into a webview. These are Frost's own shapes;
/// [`FrostWebView::send_input`] translates them into Servo's [`InputEvent`].
/// Round 2 fills in the winit/libcosmic → `FrostInputEvent` mapping.
#[derive(Debug, Clone)]
pub enum FrostInputEvent {
    MouseMove {
        x: f32,
        y: f32,
    },
    MouseButton {
        button: MouseButton,
        action: MouseButtonAction,
        x: f32,
        y: f32,
    },
    Wheel {
        dx: f64,
        dy: f64,
        x: f32,
        y: f32,
    },
    Key {
        state: KeyState,
        key: Key,
        code: Code,
        location: Location,
        modifiers: Modifiers,
        repeat: bool,
    },
}

impl From<FrostInputEvent> for InputEvent {
    fn from(event: FrostInputEvent) -> InputEvent {
        match event {
            FrostInputEvent::MouseMove { x, y } => {
                InputEvent::MouseMove(MouseMoveEvent::new(DevicePoint::new(x, y).into()))
            }
            FrostInputEvent::MouseButton {
                button,
                action,
                x,
                y,
            } => InputEvent::MouseButton(MouseButtonEvent::new(
                action,
                button,
                DevicePoint::new(x, y).into(),
            )),
            FrostInputEvent::Wheel { dx, dy, x, y } => InputEvent::Wheel(WheelEvent::new(
                WheelDelta {
                    x: dx,
                    y: dy,
                    z: 0.0,
                    mode: WheelMode::DeltaPixel,
                },
                DevicePoint::new(x, y).into(),
            )),
            FrostInputEvent::Key {
                state,
                key,
                code,
                location,
                modifiers,
                repeat,
            } => InputEvent::Keyboard(KeyboardEvent::new_without_event(
                state, key, code, location, modifiers, repeat, false,
            )),
        }
    }
}

// Re-export the input types the UI needs to build `FrostInputEvent`s,
// so Round 2 does not have to depend on the `servo` crate directly.
pub use servo::{
    Code as FrostCode, Key as FrostKey, KeyState as FrostKeyState,
    Location as FrostLocation, Modifiers as FrostModifiers, NamedKey as FrostNamedKey,
};
pub use servo::{MouseButton as FrostMouseButton, MouseButtonAction as FrostMouseButtonAction};

/// Minimal [`EventLoopWaker`]: Servo calls `wake()` when it wants the
/// embedder to spin soon. The smoke test and (later) the UI poll on a short
/// timeout anyway, so the flag is a hint, not a mechanism.
struct FrostWaker {
    flag: Arc<AtomicBool>,
}

impl EventLoopWaker for FrostWaker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(FrostWaker {
            flag: self.flag.clone(),
        })
    }

    fn wake(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }
}

/// Forwards Servo delegate callbacks into an [`EngineEvent`] channel.
#[derive(Clone)]
struct FrostDelegate {
    tx: mpsc::Sender<EngineEvent>,
}

impl WebViewDelegate for FrostDelegate {
    fn notify_page_title_changed(&self, webview: WebView, _title: Option<String>) {
        let _ = self
            .tx
            .send(EngineEvent::TitleChanged(webview.page_title()));
    }

    fn notify_url_changed(&self, _webview: WebView, url: Url) {
        let _ = self.tx.send(EngineEvent::UrlChanged(url));
    }

    fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
        let event = match status {
            LoadStatus::Started => EngineEvent::LoadStarted,
            LoadStatus::Complete => EngineEvent::LoadFinished,
            // 0.8.0 added Stopped (the load ended early, e.g. via
            // window.stop() inside the page). Surfaced as its own event
            // so the UI can tell "stopped" from "finished".
            LoadStatus::Stopped => EngineEvent::LoadStopped,
            // HeadParsed is not surfaced: the UI only cares about
            // started/finished/stopped.
            LoadStatus::HeadParsed => return,
        };
        let _ = self.tx.send(event);
    }

    fn notify_new_frame_ready(&self, _webview: WebView) {
        let _ = self.tx.send(EngineEvent::FrameReady);
    }

    fn notify_crashed(&self, _webview: WebView, reason: String, _backtrace: Option<String>) {
        let _ = self.tx.send(EngineEvent::Crashed(reason));
    }
}

/// The engine: owns Servo, spawns webviews, pumps the event loop.
pub struct Engine {
    servo: Servo,
    tx: mpsc::Sender<EngineEvent>,
    rx: mpsc::Receiver<EngineEvent>,
}

impl Engine {
    /// Create the engine. One per process; call [`Engine::spin`] regularly
    /// afterwards or nothing ever loads or paints.
    pub fn new() -> Result<Self, EngineError> {
        let (tx, rx) = mpsc::channel();
        let waker = FrostWaker {
            flag: Arc::new(AtomicBool::new(false)),
        };
        let servo = ServoBuilder::default()
            .event_loop_waker(Box::new(waker))
            .build();
        servo.setup_logging();
        Ok(Self { servo, tx, rx })
    }

    /// Spawn a webview loading `url` at `width` × `height` CSS pixels.
    pub fn spawn_webview(
        &self,
        url: &str,
        width: u32,
        height: u32,
    ) -> Result<FrostWebView, EngineError> {
        let url = Url::parse(url).map_err(|e| EngineError::InvalidUrl(e.to_string()))?;
        let ctx = Rc::new(
            SoftwareRenderingContext::new(PhysicalSize::new(width, height))
                .map_err(|e| EngineError::Servo(format!("rendering context: {e:?}")))?,
        );
        let _ = ctx.make_current();
        let delegate = FrostDelegate {
            tx: self.tx.clone(),
        };
        let webview = WebViewBuilder::new(&self.servo, ctx.clone())
            .url(url)
            .delegate(Rc::new(delegate))
            .build();
        webview.set_focused(true);
        Ok(FrostWebView {
            inner: webview,
            ctx,
            width,
            height,
        })
    }

    /// Pump Servo's event loop once. Must be called regularly (every few
    /// milliseconds while a page is loading or animating) on the thread
    /// that created the engine.
    pub fn spin(&self) {
        self.servo.spin_event_loop();
    }

    /// Non-blocking drain of one pending event, if any.
    pub fn try_recv_event(&self) -> Option<EngineEvent> {
        self.rx.try_recv().ok()
    }

    /// Set a Servo preference by name (e.g. `dom_async_clipboard_enabled`).
    pub fn set_preference(&self, name: &str, value: PrefValue) {
        self.servo.set_preference(name, value);
    }
}

/// One tab's worth of Servo: a webview plus its rendering context.
pub struct FrostWebView {
    inner: WebView,
    ctx: Rc<SoftwareRenderingContext>,
    width: u32,
    height: u32,
}

impl FrostWebView {
    /// Navigate the main frame to `url`.
    pub fn load(&self, url: &str) -> Result<(), EngineError> {
        let url = Url::parse(url).map_err(|e| EngineError::InvalidUrl(e.to_string()))?;
        self.inner.load(url);
        Ok(())
    }

    /// Whether the webview has history to go back to.
    pub fn can_go_back(&self) -> bool {
        self.inner.can_go_back()
    }

    /// Whether the webview has history to go forward to.
    pub fn can_go_forward(&self) -> bool {
        self.inner.can_go_forward()
    }

    /// Navigate back one history entry.
    pub fn go_back(&self) {
        self.inner.go_back(1);
    }

    /// Navigate forward one history entry.
    pub fn go_forward(&self) {
        self.inner.go_forward(1);
    }

    /// Reload the current page.
    pub fn reload(&self) {
        self.inner.reload();
    }

    /// Stop the current load.
    ///
    /// Round 2: live, via fork patch F-001 (`WebView::stop` in the fork,
    /// which asks the script thread of the active pipeline to abort the
    /// document load — the same path as `window.stop()` in the page).
    /// The engine reports [`EngineEvent::LoadStopped`] when the load
    /// actually stops. A no-op when nothing is loading.
    pub fn stop(&self) -> Result<(), EngineError> {
        self.inner.stop();
        Ok(())
    }

    /// Evaluate a JavaScript snippet in the page (Round 3: content
    /// scripts). The fork already exposes `WebView::evaluate_javascript`,
    /// so no fork patch was needed — this is a thin wrapper. The result
    /// is discarded (content scripts are fire-and-forget); evaluation
    /// errors surface only in the engine log, never as a crash.
    pub fn evaluate_javascript(&self, script: &str) {
        self.inner.evaluate_javascript(script.to_string(), |_| {});
    }

    /// Capture the current page as PNG bytes (Round 4, F-003 print
    /// preview). Servo 0.8.0 has no print pipeline — no `WebView::print`,
    /// no PDF path, only debug tree dumps — so a screenshot of the
    /// current viewport IS the preview. Async: the callback fires once
    /// rendering is up to date; the result is PNG bytes or an error
    /// string, delivered on the caller's channel (polled by the app's
    /// Tick). `None` rect means the whole webview.
    pub fn request_screenshot_png(
        &self,
        tx: std::sync::mpsc::Sender<Result<Vec<u8>, String>>,
    ) {
        self.inner.take_screenshot(None, move |result| {
            let out = match result {
                Ok(img) => {
                    let (w, h) = (img.width(), img.height());
                    let mut bytes = Vec::new();
                    let mut encoder = png::Encoder::new(&mut bytes, w, h);
                    encoder.set_color(png::ColorType::Rgba);
                    encoder.set_depth(png::BitDepth::Eight);
                    match encoder
                        .write_header()
                        .and_then(|mut writer| writer.write_image_data(&img.into_raw()))
                    {
                        Ok(()) => Ok(bytes),
                        Err(e) => Err(format!("PNG encode failed: {e}")),
                    }
                },
                Err(e) => Err(format!("screenshot failed: {e:?}")),
            };
            let _ = tx.send(out);
        });
    }

    /// Re-send the current viewport and rebuild the root display list,
    /// without resizing (Round 3, fork patch F-005). This is the proper
    /// full-repaint trigger: it replaces the Round 2 resize jiggle (see
    /// the app's `start_repaint_refresh`).
    pub fn refresh_viewport(&self) {
        self.inner.refresh_viewport();
    }

    /// Tell the page the system theme changed (Round 3: the settings
    /// theme). Servo maps this onto `prefers-color-scheme`, so themed
    /// pages (including Frost's own internal pages) restyle.
    pub fn set_theme(&self, dark: bool) {
        self.inner
            .notify_theme_change(if dark { Theme::Dark } else { Theme::Light });
    }

    /// Resize the viewport. Follows the servo-de Round 8 finding: in
    /// 0.8.0, `WebView::resize` alone is enough — the rendering context
    /// follows the webview.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.inner.resize(PhysicalSize::new(width, height));
    }

    /// Read the latest painted frame as RGBA pixels, or `None` if nothing
    /// has painted yet. The caller owns the pixels; the UI uploads them
    /// into a texture each frame (Round 2).
    ///
    /// The embedder drives the paint itself, servoshell-style:
    /// `notify_new_frame_ready` only signals that a paint is *wanted*.
    /// Without `paint()` here the context keeps its cleared buffer and
    /// every frame comes back a single solid color. The read happens
    /// before `present()` on purpose: the trait documents that
    /// `read_to_image` reads the back buffer, so a post-`present` read
    /// would see the swapped (empty) buffer.
    pub fn render(&self) -> Option<Frame> {
        let _ = self.ctx.make_current();
        self.inner.paint();
        let rect = DeviceIntRect::from_origin_and_size(
            DeviceIntPoint::new(0, 0),
            DeviceIntSize::new(self.width as i32, self.height as i32),
        );
        let img = self.ctx.read_to_image(rect)?;
        let (w, h) = (img.width(), img.height());
        let frame = Frame {
            width: w,
            height: h,
            rgba: img.into_raw(),
        };
        self.ctx.present();
        Some(frame)
    }

    /// Forward a UI input event into the page.
    pub fn send_input(&self, event: FrostInputEvent) {
        self.inner.notify_input_event(event.into());
    }

    /// Current main-frame URL, if any.
    pub fn url(&self) -> Option<Url> {
        self.inner.url()
    }

    /// Current page title, if any.
    pub fn title(&self) -> Option<String> {
        self.inner.page_title()
    }

    /// Current load status.
    pub fn load_status(&self) -> LoadStatus {
        self.inner.load_status()
    }

    /// Current viewport size.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

/// Build a `data:` URL for a small placeholder page (Round 2 menu entries:
/// Settings / Print / Open load these through the engine until the real
/// `frost://` internal pages land in Round 3). The HTML is percent-encoded
/// so the URL is valid as-is.
pub fn data_url_page(title: &str, body_html: &str) -> String {
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title>\
         <style>body{{background:#1e1e2e;color:#e8e8f0;font-family:sans-serif;margin:48px}}\
         h1{{font-size:28px}}p{{font-size:16px;line-height:1.5;color:#b8b8cc}}\
         a{{color:#7aa2ff}}</style></head><body><h1>{title}</h1>{body_html}</body></html>"
    );
    let mut out = String::from("data:text/html;charset=utf-8,");
    for byte in html.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            },
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use servo::NamedKey;

    #[test]
    fn frame_solid_color_detection() {
        let solid = Frame {
            width: 2,
            height: 2,
            rgba: vec![255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
        };
        assert!(solid.is_solid_color());
        assert_eq!(solid.distinct_colors(), 1);

        let mixed = Frame {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 0, 255, 255],
        };
        assert!(!mixed.is_solid_color());
        assert_eq!(mixed.distinct_colors(), 2);
    }

    #[test]
    fn data_url_page_is_a_valid_data_url() {
        let url = data_url_page("Settings", "<p>hello world</p>");
        assert!(url.starts_with("data:text/html;charset=utf-8,"));
        assert!(url.contains("Settings"));
        // No raw characters that would break a URL.
        assert!(!url.contains('<'));
        assert!(!url.contains(' '));
        assert!(!url.contains('"'));
        // The body survives percent-decoding.
        let encoded = url.trim_start_matches("data:text/html;charset=utf-8,");
        let mut decoded = String::new();
        let mut chars = encoded.chars();
        while let Some(c) = chars.next() {
            if c == '%' {
                let hex: String = chars.by_ref().take(2).collect();
                decoded.push(u8::from_str_radix(&hex, 16).unwrap() as char);
            } else {
                decoded.push(c);
            }
        }
        assert!(decoded.contains("<p>hello world</p>"));
    }

    #[test]
    fn input_event_conversion_compiles_and_maps() {
        let ev: InputEvent = FrostInputEvent::MouseMove { x: 10.0, y: 20.0 }.into();
        assert!(matches!(ev, InputEvent::MouseMove(_)));
        let ev: InputEvent = FrostInputEvent::Wheel {
            dx: 0.0,
            dy: -120.0,
            x: 5.0,
            y: 5.0,
        }
        .into();
        assert!(matches!(ev, InputEvent::Wheel(_)));
        let ev: InputEvent = FrostInputEvent::Key {
            state: KeyState::Down,
            key: Key::Named(NamedKey::Enter),
            code: Code::Enter,
            location: Location::Standard,
            modifiers: Modifiers::empty(),
            repeat: false,
        }
        .into();
        assert!(matches!(ev, InputEvent::Keyboard(_)));
    }
}
