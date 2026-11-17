//! frost-engine: Servo embedding wrapper for the Frost browser.
//!
//! This crate owns everything Servo-shaped: creating the engine, owning one
//! [`FrostWebView`] per tab, pumping the event loop, turning Servo
//! notifications into [`EngineEvent`]s on a channel, and reading rendered
//! frames out of the rendering context as RGBA pixels. Round 16: the
//! context is hardware-first — [`HardwareRenderingContext`] (fork patch
//! F-009) when a hardware GL adapter exists, falling back to the
//! software context with a warning when it does not, so pages rasterize
//! on the GPU instead of llvmpipe where possible.
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

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use dpi::PhysicalSize;
use servo::{
    Code, DeviceIntPoint, DeviceIntRect, DeviceIntSize, DevicePoint, EventLoopWaker,
    HardwareRenderingContext, InputEvent, JSValue, JavaScriptEvaluationError, Key, KeyState,
    KeyboardEvent, LoadStatus, Location, Modifiers, MouseButton, MouseButtonAction,
    MouseButtonEvent, MouseMoveEvent, PrefValue, RenderingContext, Servo, ServoBuilder,
    SoftwareRenderingContext, Theme, WebView, WebViewBuilder, WebViewDelegate, WheelDelta,
    WheelEvent, WheelMode,
};
use url::Url;

pub mod snowplow;

use crate::snowplow::Snowplow;

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

/// Largest single image iced 0.14 uploads synchronously. iced's wgpu
/// image cache (`upload_raster`) uploads a raster image inline only
/// when its byte length is under `MAX_SYNC_SIZE` (2 MiB); anything
/// larger is handed to a worker thread and is NOT drawn on the frame
/// it first appears — the widget draws nothing for it that frame.
/// Frost's full frame (1280x684x4 = 3.5 MB) is over that line, so
/// every new frame cost one blank draw: the content area showed the
/// chrome background while loading and scrolling, then popped back.
/// That is the flashing Joel reported (Round 6). Frames are therefore
/// split into strips under this cap (see [`Frame::split_strips`]).
/// The cap sits below iced's 2 MiB line with margin, so a strip is
/// always on the synchronous path.
pub const MAX_SYNC_UPLOAD_BYTES: usize = 1_900_000;

/// iced 0.14's synchronous-upload threshold, in bytes. A raster image
/// whose RGBA byte length is at or above this is uploaded by iced's
/// worker thread and skipped on its first draw. Kept here so the
/// strip invariant is tested against the real number, not the cap.
pub const ICED_SYNC_UPLOAD_LIMIT_BYTES: usize = 2 * 1024 * 1024;

impl Frame {
    /// True when every pixel is the same color (used by the smoke test to
    /// catch a blank/never-painted frame).
    pub fn is_solid_color(&self) -> bool {
        self.rgba.chunks_exact(4).all(|px| px == &self.rgba[0..4])
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

    /// Byte length of the RGBA pixel data (`width * height * 4`).
    pub fn byte_len(&self) -> usize {
        self.rgba.len()
    }

    /// Split the frame into horizontal strips, each no larger than
    /// `max_bytes` (see [`MAX_SYNC_UPLOAD_BYTES`]). The strips tile
    /// the frame exactly: same width, heights summing to the frame
    /// height, rows in order, no gaps or overlaps. A frame already
    /// under the cap comes back as a single strip. A degenerate
    /// (zero-size) frame splits into no strips.
    pub fn split_strips(&self, max_bytes: usize) -> Vec<Frame> {
        if self.width == 0 || self.height == 0 || self.rgba.is_empty() {
            return Vec::new();
        }
        let row_bytes = self.width as usize * 4;
        // Rows per strip, floored so a strip never exceeds the cap.
        // A single row wider than the cap still fits one row per strip
        // (absurd widths aside, that is the tightest possible split).
        let rows_per_strip = (max_bytes / row_bytes).max(1);
        let mut strips = Vec::new();
        let mut y = 0u32;
        while y < self.height {
            let rows = rows_per_strip.min((self.height - y) as usize) as u32;
            let start = (y as usize) * row_bytes;
            let end = start + (rows as usize) * row_bytes;
            strips.push(Frame {
                width: self.width,
                height: rows,
                rgba: self.rgba[start..end].to_vec(),
            });
            y += rows;
        }
        strips
    }
}

/// Events the engine reports to the UI. Delivered over a channel so the UI
/// never blocks inside a Servo callback. Every variant carries the
/// [`WebViewId`] of the webview it came from (Round 14: split view
/// runs two webviews, so the UI must know which pane an event
/// belongs to — an untagged event could not be routed).
#[derive(Debug, Clone)]
pub enum EngineEvent {
    TitleChanged(WebViewId, Option<String>),
    UrlChanged(WebViewId, Url),
    LoadStarted(WebViewId),
    LoadFinished(WebViewId),
    /// The load was stopped before completing (0.8.0's `LoadStatus::Stopped`).
    LoadStopped(WebViewId),
    FrameReady(WebViewId),
    /// A tracker/ad subresource request was blocked by Snowplow
    /// (Round 13): the blocked URL, so the UI can count it.
    ResourceBlocked(WebViewId, String),
    Crashed(WebViewId, String),
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
/// Re-exported so the UI can name the id the delegate tags every
/// [`EngineEvent`] with (Round 14: split view routes events to panes).
pub use servo::WebViewId;
pub use servo::{
    Code as FrostCode, Key as FrostKey, KeyState as FrostKeyState, Location as FrostLocation,
    Modifiers as FrostModifiers, NamedKey as FrostNamedKey,
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
    /// Shared Snowplow state (Round 13): the same `Rc` the [`Engine`]
    /// keeps, so the Settings toggle flips what the delegate enforces.
    snowplow: Rc<RefCell<Snowplow>>,
}

impl WebViewDelegate for FrostDelegate {
    fn notify_page_title_changed(&self, webview: WebView, _title: Option<String>) {
        let _ = self.tx.send(EngineEvent::TitleChanged(
            webview.id(),
            webview.page_title(),
        ));
    }

    fn notify_url_changed(&self, webview: WebView, url: Url) {
        let _ = self.tx.send(EngineEvent::UrlChanged(webview.id(), url));
    }

    fn notify_load_status_changed(&self, webview: WebView, status: LoadStatus) {
        let id = webview.id();
        let event = match status {
            LoadStatus::Started => EngineEvent::LoadStarted(id),
            LoadStatus::Complete => EngineEvent::LoadFinished(id),
            // 0.8.0 added Stopped (the load ended early, e.g. via
            // window.stop() inside the page). Surfaced as its own event
            // so the UI can tell "stopped" from "finished".
            LoadStatus::Stopped => EngineEvent::LoadStopped(id),
            // HeadParsed is not surfaced: the UI only cares about
            // started/finished/stopped.
            LoadStatus::HeadParsed => return,
        };
        let _ = self.tx.send(event);
    }

    fn notify_new_frame_ready(&self, webview: WebView) {
        let _ = self.tx.send(EngineEvent::FrameReady(webview.id()));
    }

    fn notify_crashed(&self, webview: WebView, reason: String, _backtrace: Option<String>) {
        let _ = self.tx.send(EngineEvent::Crashed(webview.id(), reason));
    }

    /// Round 13 (Snowplow): every HTTP fetch reaches the embedder
    /// here first. Blocked tracker/ad subresources are cancelled
    /// (a fast network error); everything else is dropped untouched,
    /// which the responder translates to "do not intercept".
    fn load_web_resource(&self, webview: WebView, load: servo::WebResourceLoad) {
        snowplow::handle_load(load, &self.snowplow.borrow(), &self.tx, webview.id());
    }
}

/// The engine: owns Servo, spawns webviews, pumps the event loop.
pub struct Engine {
    servo: Servo,
    tx: mpsc::Sender<EngineEvent>,
    rx: mpsc::Receiver<EngineEvent>,
    /// Shared Snowplow state (Round 13): the delegate enforces it,
    /// the app toggles it. Same `Rc` in both places.
    snowplow: Rc<RefCell<Snowplow>>,
}

/// Which rendering context a new webview prefers (Round 16).
///
/// The default is hardware-first: try [`HardwareRenderingContext`]
/// (fork patch F-009) and fall back to the software context on failure.
/// `FROST_RENDERER=software` forces the software context (the escape
/// hatch if a hardware driver misbehaves on a given machine).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererPreference {
    HardwareFirst,
    SoftwareOnly,
}

/// Pure preference parsing, kept separate from the environment read so
/// tests can drive it without touching process state.
pub fn renderer_preference_from_env_value(value: Option<&str>) -> RendererPreference {
    match value {
        Some(v) if v.eq_ignore_ascii_case("software") => RendererPreference::SoftwareOnly,
        _ => RendererPreference::HardwareFirst,
    }
}

/// Read the renderer preference from the process environment.
pub fn renderer_preference() -> RendererPreference {
    renderer_preference_from_env_value(std::env::var("FROST_RENDERER").ok().as_deref())
}

/// Create a rendering context for a new webview (Round 16).
///
/// Hardware-first by default: [`HardwareRenderingContext`] when a
/// hardware GL adapter exists, falling back to
/// [`SoftwareRenderingContext`] (llvmpipe) with a warning when it does
/// not — Frost must still launch on machines with no hardware GL.
/// `FROST_RENDERER=software` skips the hardware attempt entirely.
/// Which adapter was actually picked is visible in the log: WebRender
/// logs its GL renderer at device creation
/// (`webrender::device::gl` — `Renderer: ...`).
pub fn create_rendering_context(
    width: u32,
    height: u32,
) -> Result<Rc<dyn RenderingContext>, EngineError> {
    let size = PhysicalSize::new(width, height);
    if renderer_preference() == RendererPreference::SoftwareOnly {
        return Ok(Rc::new(SoftwareRenderingContext::new(size).map_err(
            |e| EngineError::Servo(format!("rendering context: {e:?}")),
        )?));
    }
    match HardwareRenderingContext::new(size) {
        Ok(ctx) => Ok(Rc::new(ctx)),
        Err(e) => {
            log::warn!(
                "frost-engine: hardware rendering context failed ({e:?}); falling back to software rendering"
            );
            Ok(Rc::new(SoftwareRenderingContext::new(size).map_err(
                |e| EngineError::Servo(format!("rendering context: {e:?}")),
            )?))
        }
    }
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
        Ok(Self {
            servo,
            tx,
            rx,
            snowplow: Rc::new(RefCell::new(Snowplow::new())),
        })
    }

    /// Turn Snowplow blocking on or off (Round 13): the Settings
    /// toggle. Takes effect immediately for subsequent requests.
    pub fn set_snowplow_enabled(&self, enabled: bool) {
        self.snowplow.borrow_mut().set_enabled(enabled);
    }

    /// Whether Snowplow blocking is currently on.
    pub fn snowplow_enabled(&self) -> bool {
        self.snowplow.borrow().is_enabled()
    }

    /// Spawn a webview loading `url` at `width` × `height` CSS pixels.
    pub fn spawn_webview(
        &self,
        url: &str,
        width: u32,
        height: u32,
    ) -> Result<FrostWebView, EngineError> {
        let url = Url::parse(url).map_err(|e| EngineError::InvalidUrl(e.to_string()))?;
        // Round 16: hardware-first rendering context (F-009), with a
        // software fallback inside `create_rendering_context`.
        let ctx = create_rendering_context(width, height)?;
        let _ = ctx.make_current();
        let delegate = FrostDelegate {
            tx: self.tx.clone(),
            snowplow: self.snowplow.clone(),
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
    /// The rendering context this webview paints into. Round 16: the
    /// concrete type is either the hardware context (F-009) or the
    /// software one — only trait methods are used, so the field is a
    /// trait object.
    ctx: Rc<dyn RenderingContext>,
    width: u32,
    height: u32,
}

impl FrostWebView {
    /// This webview's id (Round 14): the delegate tags every
    /// [`EngineEvent`] with it, so the UI can route events to
    /// the right split-view pane.
    pub fn id(&self) -> WebViewId {
        self.inner.id()
    }

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

    /// Evaluate a JavaScript snippet and deliver its return value as a
    /// string (Round 5: the inspector walker returns a JSON string).
    /// Async like [`FrostWebView::request_screenshot_png`]: the result
    /// arrives on the caller's channel, polled by the app's Tick. The
    /// callback runs on the app thread — Servo delivers
    /// `FinishJavaScriptEvaluation` while `Engine::spin` pumps the
    /// event loop — so sending into the channel from it is safe. A
    /// non-string return value is an error, never a silent empty result.
    pub fn evaluate_javascript_string(
        &self,
        script: &str,
        tx: std::sync::mpsc::Sender<Result<String, String>>,
    ) {
        self.inner
            .evaluate_javascript(script.to_string(), move |result| {
                let _ = tx.send(js_string_result(result));
            });
    }

    /// Capture the current page as PNG bytes (Round 4, F-003 print
    /// preview). Servo 0.8.0 has no print pipeline — no `WebView::print`,
    /// no PDF path, only debug tree dumps — so a screenshot of the
    /// current viewport IS the preview. Async: the callback fires once
    /// rendering is up to date; the result is PNG bytes or an error
    /// string, delivered on the caller's channel (polled by the app's
    /// Tick). `None` rect means the whole webview.
    pub fn request_screenshot_png(&self, tx: std::sync::mpsc::Sender<Result<Vec<u8>, String>>) {
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
                }
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

/// Convert a script-evaluation result into the string the caller asked
/// for (Round 5). Pure, so it is unit-tested here: the inspector walker
/// returns a JSON string, so any other value — a number, an object, a
/// script error — is an error, never a silent empty tree.
pub fn js_string_result(
    result: Result<JSValue, JavaScriptEvaluationError>,
) -> Result<String, String> {
    match result {
        Ok(JSValue::String(s)) => Ok(s),
        Ok(other) => Err(format!("script returned a non-string value: {other:?}")),
        Err(e) => Err(format!("script evaluation failed: {e:?}")),
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
            }
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
            rgba: vec![
                255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
            ],
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

    /// A frame whose rows are painted with their row index, so a strip
    /// can be checked for exact row coverage.
    fn row_painted_frame(width: u32, height: u32) -> Frame {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for _ in 0..width {
                rgba.extend_from_slice(&[y as u8, 0, 0, 255]);
            }
        }
        Frame {
            width,
            height,
            rgba,
        }
    }

    fn assert_strips_tile(frame: &Frame, strips: &[Frame]) {
        assert_eq!(
            strips.iter().map(|s| s.height).sum::<u32>(),
            frame.height,
            "strip heights must sum to the frame height"
        );
        assert!(strips.iter().all(|s| s.width == frame.width));
        // Rows in order, no gaps or overlaps: strip i starts at the
        // row the previous strips covered.
        let mut y = 0u32;
        for strip in strips {
            for row in 0..strip.height {
                let px = &strip.rgba[(row as usize) * (strip.width as usize) * 4..][..4];
                assert_eq!(px[0], y as u8, "strip row {row} must carry source row {y}");
                y += 1;
            }
        }
        assert_eq!(y, frame.height);
    }

    #[test]
    fn split_strips_tiles_and_stays_under_iced_sync_limit() {
        // The sizes Frost actually renders at, plus odd shapes.
        for (w, h) in [
            (1280, 684),
            (1920, 984),
            (800, 600),
            (1280, 800),
            (100, 100),
            (1, 1),
            (640, 341),
            (1280, 1),
            (3, 7),
        ] {
            let frame = row_painted_frame(w, h);
            let strips = frame.split_strips(MAX_SYNC_UPLOAD_BYTES);
            assert!(!strips.is_empty(), "{w}x{h}: no strips");
            assert_strips_tile(&frame, &strips);
            for strip in &strips {
                assert!(
                    strip.byte_len() < ICED_SYNC_UPLOAD_LIMIT_BYTES,
                    "{w}x{h}: strip {}x{} is {} bytes, over iced's sync limit",
                    strip.width,
                    strip.height,
                    strip.byte_len()
                );
            }
        }
    }

    #[test]
    fn split_strips_is_one_strip_when_small() {
        let frame = row_painted_frame(100, 50);
        let strips = frame.split_strips(MAX_SYNC_UPLOAD_BYTES);
        assert_eq!(strips.len(), 1);
        assert_eq!(strips[0].width, 100);
        assert_eq!(strips[0].height, 50);
        assert_eq!(strips[0].rgba, frame.rgba);
    }

    #[test]
    fn split_strips_handles_degenerate_frames() {
        let empty = Frame {
            width: 0,
            height: 0,
            rgba: Vec::new(),
        };
        assert!(empty.split_strips(MAX_SYNC_UPLOAD_BYTES).is_empty());
        let no_rows = Frame {
            width: 10,
            height: 0,
            rgba: Vec::new(),
        };
        assert!(no_rows.split_strips(MAX_SYNC_UPLOAD_BYTES).is_empty());
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
    fn js_string_result_keeps_strings_and_rejects_the_rest() {
        assert_eq!(
            js_string_result(Ok(JSValue::String("{\"a\":1}".to_string()))).unwrap(),
            "{\"a\":1}"
        );
        assert!(js_string_result(Ok(JSValue::Number(42.0))).is_err());
        assert!(js_string_result(Ok(JSValue::Undefined)).is_err());
        assert!(js_string_result(Err(JavaScriptEvaluationError::DocumentNotFound)).is_err());
        assert!(js_string_result(Err(JavaScriptEvaluationError::CompilationFailure)).is_err());
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

    #[test]
    fn renderer_preference_defaults_to_hardware_first() {
        assert_eq!(
            renderer_preference_from_env_value(None),
            RendererPreference::HardwareFirst
        );
        assert_eq!(
            renderer_preference_from_env_value(Some("")),
            RendererPreference::HardwareFirst
        );
        assert_eq!(
            renderer_preference_from_env_value(Some("hardware")),
            RendererPreference::HardwareFirst
        );
        // Unknown values do not change the default — only "software" opts out.
        assert_eq!(
            renderer_preference_from_env_value(Some("bogus")),
            RendererPreference::HardwareFirst
        );
    }

    #[test]
    fn renderer_preference_software_is_case_insensitive() {
        assert_eq!(
            renderer_preference_from_env_value(Some("software")),
            RendererPreference::SoftwareOnly
        );
        assert_eq!(
            renderer_preference_from_env_value(Some("Software")),
            RendererPreference::SoftwareOnly
        );
        assert_eq!(
            renderer_preference_from_env_value(Some("SOFTWARE")),
            RendererPreference::SoftwareOnly
        );
    }
}
