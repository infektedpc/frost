//! Frost browser — libcosmic application shell.
//!
//! Round 2 wires the engine in for real:
//!
//! * The content area shows the live Servo frame, uploaded into iced
//!   textures whenever the engine reports `FrameReady` (or while loading).
//!   Round 6: the frame goes up as horizontal strips, each under iced's
//!   2 MiB synchronous-upload line — one whole-frame image per render
//!   flashed dark for a frame on every load/scroll repaint (see
//!   `Frame::split_strips`).
//! * The toolbar navigates for real: back / forward (disabled when the
//!   engine says there is nothing to go to), reload, stop (live since fork
//!   patch F-001), address bar (Enter navigates; bare hostnames get
//!   `https://`, anything else is a search with the default engine).
//! * Input forwards into the page: the app listens to all iced events and
//!   translates the ones landing in the content area into
//!   [`frost_engine::FrostInputEvent`]s. Keyboard goes to the page only
//!   while the content area has focus (a click in the content, or Enter in
//!   the address bar); typing in the address bar stays in the bar.
//! * The hamburger menu opens as a dropdown; Settings / Print / Exit /
//!   Open load `frost://` internal pages served by the app's own
//!   loopback server (Round 3, `server.rs`) — the address bar shows
//!   `frost://settings`, never a raw `data:` URL. Open is a real file
//!   browser — the listing is generated app-side and served by the
//!   server, folder clicks re-list, file clicks open the file in the
//!   browser, `.md` files render as formatted HTML; Exit quits the
//!   process (via the confirmation page when enabled). Inspector
//!   (Round 5) captures the current page's DOM before opening
//!   `frost://inspector` on the snapshot — the single webview cannot
//!   show the inspector and the inspected page at once.
//! * Settings (`settings.rs`) are a working page: theme (light/dark,
//!   applied live to Frost's own chrome and to the pages), homepage
//!   (next launch), default search engine, exit confirmation. Settings
//!   persist as JSON under the config dir.
//! * Extension host v1 (`frost-extensions`): extensions load from
//!   directories (manifest + content script), the host matches content
//!   scripts against page URLs, and the engine evaluates them in the
//!   page. A bad manifest is logged and skipped, never a crash.
//! * Frost draws its own titlebar (the built-in libcosmic headerbar is
//!   hidden — it renders the title but no window-control buttons on this
//!   setup): "Frost — <page title>" with real minimize, maximize/restore,
//!   and close buttons (text glyphs, real iced window commands).
//! * Round 4, F-007: the Round 2 resize "jiggle" is RETIRED. Its root
//!   cause was the fork's damage-rects patch setting WebRender
//!   `max_partial_present_rects: 1` (partial present) while Frost's
//!   engine reads the GL back buffer wholesale — the full scene
//!   rendered once and was lost, and only dirty regions repainted.
//!   The fork now uses full present (`max_partial_present_rects: 0`,
//!   the upstream default), so the first paint is complete and no
//!   post-load jiggle is needed. F-005's `refresh_viewport` stays
//!   (harmless viewport re-send).
//!
//! Chrome geometry is fixed by construction (Frost titlebar 40, toolbar
//! 48, menu 148 when open, status 28), so the webview size and the click
//! mapping are exact: the content area is the window minus the chrome,
//! and mouse coordinates translate 1:1 into page coordinates.
//!
//! Launch with:
//!   cargo run -p frost
//! Without the `ui-libcosmic` feature this binary prints a short status
//! note and exits 0, so the workspace always compiles even where
//! libcosmic cannot build.

#[cfg(feature = "ui-libcosmic")]
mod file_browser;

#[cfg(feature = "ui-libcosmic")]
mod keymap;

#[cfg(feature = "ui-libcosmic")]
mod server;

#[cfg(feature = "ui-libcosmic")]
mod settings;

#[cfg(feature = "ui-libcosmic")]
mod app {
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use cosmic::app::{Core, Settings, Task};
    use cosmic::iced::keyboard;
    use cosmic::iced::mouse;
    use cosmic::iced::widget::image::{Handle, Image};
    use cosmic::iced::widget::mouse_area;
    use cosmic::iced::{
        event, window, ContentFit, Length, Point, Rectangle, Size, Subscription,
    };
    use cosmic::widget::{self, button, container, text, text_input};
    use cosmic::{executor, iced, ApplicationExt, Element};
    use frost_engine::{
        Engine, EngineEvent, FrostCode, FrostInputEvent, FrostKeyState,
        FrostMouseButtonAction, FrostWebView, MAX_SYNC_UPLOAD_BYTES,
    };
    use frost_extensions::{ExtensionHost, ExtensionManifest, StubHost};
    use frost_inspector::{snapshot_from_json, DOM_WALK_SCRIPT};
    use url::Url;

    use crate::file_browser;
    use crate::keymap::{map_button, map_code, map_key_with_text, map_location, map_modifiers};
    use crate::server::{InspectorSnapshot, InternalServer, Shared};
    use crate::settings::{ChromeColors, Settings as FrostSettings};

    /// Chrome geometry, fixed by construction (see the module docs).
    /// Frost draws its OWN titlebar above the toolbar (40 px): the
    /// built-in libcosmic headerbar is hidden, because on this setup it
    /// renders the title but no window-control buttons at all (its
    /// min/max/close are icon-based and never appear — verified on
    /// Joel's box and in the sandbox). Frost's titlebar uses plain text
    /// glyphs, so the buttons are always visible.
    const TITLEBAR_HEIGHT: f32 = 40.0;
    const TOOLBAR_HEIGHT: f32 = 48.0;
    const STATUS_HEIGHT: f32 = 28.0;
    /// Height of the hamburger menu (5 entries since Round 5). Round 2
    /// used 136 and the 4th entry (Exit) was clipped ~4 px at the
    /// bottom edge; 148 fits four fully, 185 fits five.
    const MENU_HEIGHT: f32 = 185.0;
    const INITIAL_WINDOW: (f32, f32) = (1280.0, 800.0);
    const HOME_URL: &str = "https://example.org";
    /// Engine pump rate. The tick spins Servo, drains its events, and
    /// re-renders only while loading or when a frame was flagged ready.
    const TICK: Duration = Duration::from_millis(16);

    #[derive(Clone, Debug)]
    pub enum Message {
        /// Pump the engine (spin + drain events + maybe re-render).
        Tick,
        /// A raw iced event (mouse / keyboard / window) for input forwarding.
        IcedEvent(iced::Event),
        Back,
        Forward,
        Reload,
        Stop,
        AddressChanged(String),
        AddressSubmitted,
        ToggleMenu,
        OpenSettings,
        OpenPrint,
        OpenInspector,
        OpenFile,
        Exit,
        /// Titlebar window controls (real iced window commands).
        TitlebarMinimize,
        TitlebarMaximize,
        TitlebarClose,
        /// Press on the titlebar (not on a button) starts a window drag.
        TitlebarDrag,
        /// Double-press on the titlebar toggles maximize/restore.
        TitlebarDoubleClick,
    }

    pub struct Frost {
        core: Core,
        engine: Engine,
        webview: FrostWebView,
        /// Latest uploaded frame, as horizontal strips, if one has
        /// painted yet. Each entry is an iced image handle plus the
        /// strip's height in pixels; the strips tile the content area
        /// top to bottom (see `Frame::split_strips`).
        ///
        /// Round 6: a frame is NEVER uploaded as one image. iced 0.14
        /// uploads a raster image synchronously only when it is under
        /// 2 MiB; a larger image is handed to a worker thread and is
        /// NOT drawn on the frame it first appears — the widget draws
        /// nothing for it, so the content area showed the chrome
        /// background for one frame per new handle. Frost's full frame
        /// (1280x684x4 = 3.5 MB) is over that line, so every frame
        /// during load and scroll flashed dark, then popped back. The
        /// strips are each under the line, so every strip takes the
        /// synchronous path and is drawn the same frame it arrives.
        frame_strips: Vec<(Handle, f32)>,
        /// A new frame is waiting to be uploaded.
        frame_dirty: bool,
        /// A load is in progress (Stop enabled, status shows it).
        loading: bool,
        /// What the address bar shows.
        address: String,
        /// The user is mid-edit in the address bar: engine URL updates
        /// must not clobber what they are typing.
        address_editing: bool,
        /// One-line status under the toolbar.
        status: String,
        /// The page title, for the status line once loaded.
        title: Option<String>,
        /// Whether the hamburger menu is open.
        menu_open: bool,
        /// Last `frost://open` path intercepted, so the Tick's URL poll
        /// does not re-intercept the same folder navigation every tick
        /// (cleared whenever the webview is on a non-`frost://open` URL).
        last_open_path: Option<std::path::PathBuf>,
        can_back: bool,
        can_forward: bool,
        window_size: Size,
        /// Last cursor position, in absolute window coordinates.
        last_cursor: Option<Point>,
        /// Keyboard events go to the page only while this is set.
        content_focused: bool,
        /// The titlebar text changed; the OS window title needs the
        /// same update (via set_window_title, issued from the Tick).
        title_dirty: bool,
        /// A load is in flight and no frame from it may be rendered yet.
        /// The first paint of a load may be mid-build (the display list
        /// is still arriving); rendering it would show a half-loaded
        /// page, so the app renders nothing until LoadFinished (the
        /// smoke test's pattern: one render after load + FrameReady
        /// always paints fully). The old frame stays up during a
        /// navigation until the new one is ready. (Round 4, F-007: with
        /// full present a later paint DOES repair a partial frame now —
        /// the old "damage is spent" claim is gone — but waiting for
        /// LoadFinished still avoids showing a half-loaded page.)
        awaiting_load: bool,
        /// When the current load started; bounds the wait so a page that
        /// never fires LoadFinished cannot leave a blank content area
        /// forever (after 15 s renders are allowed anyway).
        loading_since: Option<std::time::Instant>,
        /// A print preview capture is in flight (Round 4, F-003): the
        /// channel the engine delivers the PNG bytes on, polled by the
        /// Tick. Set by `Message::OpenPrint`; cleared when the bytes
        /// arrive (or the capture errors) and frost://print opens.
        print_shot_rx: Option<std::sync::mpsc::Receiver<Result<Vec<u8>, String>>>,
        /// A print preview capture is in flight; the status line says
        /// so until the bytes arrive.
        print_shot_pending: bool,
        /// An inspector DOM capture is in flight (Round 5): the channel
        /// the walker JSON arrives on, polled by the Tick. Set by
        /// `open_inspector`; cleared when the JSON arrives (or the
        /// capture errors) and frost://inspector opens.
        inspector_shot_rx: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
        /// An inspector capture is in flight; the status line says so
        /// until the snapshot arrives.
        inspector_shot_pending: bool,
        /// The display URL the in-flight inspector capture is for
        /// (shown in the snapshot header once it arrives).
        inspector_capture_url: Option<String>,
        /// A re-capture was requested from the inspector page (Round 5):
        /// once the inspected page (this URL) loads again, capture it.
        /// Set by the Tick when it sees the server's
        /// `inspector_recapture_requested` flag; cleared when the
        /// capture starts.
        pending_inspector_capture: Option<String>,
        /// Current settings (theme, homepage, search engine, exit
        /// confirmation). Loaded at startup; the Tick polls the shared
        /// version for changes made through the settings page.
        settings: FrostSettings,
        /// Settings version last applied to the app (see
        /// `Shared::settings_version`).
        settings_version: u64,
        /// The internal-page loopback server (owns the shared state).
        server: InternalServer,
        /// Content-script host (StubHost: in-memory, real disk loading
        /// + real injection through the engine — see
        /// `inject_content_scripts`).
        host: StubHost,
        /// URL content scripts were last injected for; a new navigation
        /// clears it so the new page gets its scripts.
        injected_for: Option<String>,
    }

    impl Frost {
        /// The content area's rectangle in window coordinates.
        fn content_rect(&self) -> Rectangle {
            let menu = if self.menu_open { MENU_HEIGHT } else { 0.0 };
            Rectangle {
                x: 0.0,
                y: TITLEBAR_HEIGHT + TOOLBAR_HEIGHT + menu,
                width: self.window_size.width,
                height: (self.window_size.height
                    - TITLEBAR_HEIGHT
                    - TOOLBAR_HEIGHT
                    - menu
                    - STATUS_HEIGHT)
                    .max(1.0),
            }
        }

        /// The webview viewport size for the current window/menu state.
        fn content_size(&self) -> (u32, u32) {
            let rect = self.content_rect();
            (
                rect.width.max(1.0) as u32,
                rect.height.max(1.0) as u32,
            )
        }

        /// Resize the webview to the content area, if it changed.
        fn resize_webview(&mut self) {
            let (w, h) = self.content_size();
            if self.webview.size() != (w, h) {
                self.webview.resize(w, h);
                self.frame_dirty = true;
            }
        }

        /// Translate an absolute window point into content-area coordinates,
        /// or `None` when the point is outside the content area.
        fn content_point(&self, point: Point) -> Option<Point> {
            let rect = self.content_rect();
            if rect.contains(point) {
                Some(Point::new(point.x - rect.x, point.y - rect.y))
            } else {
                None
            }
        }

        /// Titlebar text: the browser name, then the current site's
        /// title. Before any title arrives, the site's host stands in;
        /// internal (data:) pages show just the browser name.
        fn titlebar_text(&self) -> String {
            let label = match &self.title {
                Some(t) if !t.is_empty() => t.clone(),
                _ => match Url::parse(&self.address) {
                    Ok(u) if matches!(u.scheme(), "http" | "https") => u
                        .host_str()
                        .unwrap_or(&self.address)
                        .to_string(),
                    _ => return "Frost".to_string(),
                },
            };
            format!("Frost — {label}")
        }

        /// One engine event: update status, address bar, and history state.
        fn handle_engine_event(&mut self, event: EngineEvent) {
            match event {
                EngineEvent::TitleChanged(title) => {
                    self.title = title.clone();
                    self.title_dirty = true;
                    if !self.loading {
                        self.status = self
                            .title
                            .clone()
                            .unwrap_or_else(|| "Untitled page".to_string());
                    }
                    if let Ok(mut shared) = self.server.shared.lock() {
                        shared.current_page_title = title.unwrap_or_default();
                    }
                },
                EngineEvent::UrlChanged(url) => {
                    // The engine sees the loopback URL for internal
                    // pages; the address bar shows the `frost://` form.
                    let display = crate::server::loopback_url_to_frost(
                        &url.to_string(),
                        &self.server.base_url,
                    )
                    .unwrap_or_else(|| url.to_string());
                    if !self.address_editing {
                        self.address = display.clone();
                    }
                    self.title_dirty = true;
                    self.can_back = self.webview.can_go_back();
                    self.can_forward = self.webview.can_go_forward();
                    // The exit page's Quit link lands here: quitting is
                    // the action, not a page to show.
                    if display == "frost://exit/confirm" {
                        std::process::exit(0);
                    }
                    // Track the live page for the print/exit pages.
                    if let Ok(mut shared) = self.server.shared.lock() {
                        shared.current_page_url = display.clone();
                        if !display.starts_with("frost://") {
                            shared.last_external_url = Some(display.clone());
                        }
                    }
                    // A folder click in the file browser navigates to
                    // `frost://open?path=...` — but only the `frost://`
                    // form needs intercepting: the engine cannot load
                    // the `frost:` scheme, so the app translates it to
                    // the loopback form. The loopback `/open` form
                    // loads fine on its own (Round 5 wart fix:
                    // re-loading it duplicated the history entry, so
                    // Back across folder clicks stayed on the same
                    // folder). Guarded by last_open_path so one
                    // navigation is intercepted exactly once.
                    if file_browser::needs_open_intercept(&url) {
                        if let Some(path) = file_browser::parse_open_url(&url) {
                            if self.last_open_path.as_ref() != Some(&path) {
                                self.open_file_browser(&path);
                            }
                        }
                    }
                },
                EngineEvent::LoadStarted => {
                    self.loading = true;
                    self.awaiting_load = true;
                    self.loading_since = Some(std::time::Instant::now());
                    self.injected_for = None;
                    self.status = format!("Loading {} …", self.address);
                },
                EngineEvent::LoadFinished => {
                    self.loading = false;
                    self.awaiting_load = false;
                    self.loading_since = None;
                    self.address_editing = false;
                    self.status = self.title.clone().unwrap_or_else(|| "Done".to_string());
                    self.inject_content_scripts();
                    // Round 5: a re-capture was requested from the
                    // inspector page — capture this page now that it
                    // loaded, if it is the page asked for.
                    self.maybe_capture_inspector();
                    // Round 4, F-007: no post-load jiggle any more — the
                    // fork uses full present, so the first paint after
                    // LoadFinished is complete as-is.
                },
                EngineEvent::LoadStopped => {
                    self.loading = false;
                    self.awaiting_load = false;
                    self.loading_since = None;
                    self.status = "Stopped.".to_string();
                },
                EngineEvent::FrameReady => {
                    self.frame_dirty = true;
                },
                EngineEvent::Crashed(reason) => {
                    self.loading = false;
                    self.awaiting_load = false;
                    self.loading_since = None;
                    self.status = format!("Page crashed: {reason}");
                },
            }
        }

        /// A raw iced event: forward the content-area ones into the page.
        fn handle_iced_event(&mut self, event: iced::Event) {
            match event {
                iced::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                    self.last_cursor = Some(position);
                    if let Some(p) = self.content_point(position) {
                        self.webview
                            .send_input(FrostInputEvent::MouseMove { x: p.x, y: p.y });
                    }
                },
                iced::Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                    if let Some(pos) = self.last_cursor {
                        if let Some(p) = self.content_point(pos) {
                            self.content_focused = true;
                            self.webview.send_input(FrostInputEvent::MouseButton {
                                button: map_button(button),
                                action: FrostMouseButtonAction::Down,
                                x: p.x,
                                y: p.y,
                            });
                        }
                    }
                },
                iced::Event::Mouse(mouse::Event::ButtonReleased(button)) => {
                    if let Some(pos) = self.last_cursor {
                        if let Some(p) = self.content_point(pos) {
                            self.webview.send_input(FrostInputEvent::MouseButton {
                                button: map_button(button),
                                action: FrostMouseButtonAction::Up,
                                x: p.x,
                                y: p.y,
                            });
                        }
                    }
                },
                iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                    if let Some(pos) = self.last_cursor {
                        if let Some(p) = self.content_point(pos) {
                            // Servo takes pixel deltas; a "line" is ~40 px.
                            let (dx, dy) = match delta {
                                mouse::ScrollDelta::Lines { x, y } => {
                                    (x as f64 * 40.0, y as f64 * 40.0)
                                },
                                mouse::ScrollDelta::Pixels { x, y } => {
                                    (x as f64, y as f64)
                                },
                            };
                            self.webview.send_input(FrostInputEvent::Wheel {
                                dx,
                                dy,
                                x: p.x,
                                y: p.y,
                            });
                        }
                    }
                },
                iced::Event::Keyboard(keyboard::Event::KeyPressed {
                    key,
                    physical_key,
                    location,
                    modifiers,
                    text,
                    repeat,
                    ..
                }) => {
                    // Round 5: F12 opens the Inspector — a browser
                    // shortcut, not a page key. Checked before the
                    // content-focus gate, so it works from the address
                    // bar too; repeats are ignored (one capture per
                    // press). The key never reaches the page.
                    if !repeat
                        && matches!(key, keyboard::Key::Named(keyboard::key::Named::F12))
                    {
                        self.open_inspector();
                    } else if self.content_focused {
                        self.send_key(
                            FrostKeyState::Down,
                            &key,
                            physical_key,
                            location,
                            modifiers,
                            repeat,
                            text.as_deref(),
                        );
                    }
                },
                iced::Event::Keyboard(keyboard::Event::KeyReleased {
                    key,
                    physical_key,
                    location,
                    modifiers,
                    ..
                }) => {
                    if self.content_focused {
                        self.send_key(
                            FrostKeyState::Up,
                            &key,
                            physical_key,
                            location,
                            modifiers,
                            false,
                            None,
                        );
                    }
                },
                _ => {},
            }
        }

        fn send_key(
            &self,
            state: FrostKeyState,
            key: &keyboard::Key,
            physical_key: keyboard::key::Physical,
            location: keyboard::Location,
            modifiers: keyboard::Modifiers,
            repeat: bool,
            text: Option<&str>,
        ) {
            let code = match physical_key {
                keyboard::key::Physical::Code(code) => map_code(code),
                keyboard::key::Physical::Unidentified(_) => FrostCode::Unidentified,
            };
            self.webview.send_input(FrostInputEvent::Key {
                state,
                // `text` is the character the keypress actually
                // produced; see `map_key_with_text`. Control/Meta
                // chords ignore it (they are shortcuts, and winit
                // reports e.g. "\x01" as the text for Ctrl+A).
                key: map_key_with_text(key, text, modifiers),
                code,
                location: map_location(location),
                modifiers: map_modifiers(modifiers),
                repeat,
            });
        }

        /// Load a loopback URL for an internal page, showing
        /// `display_url` (the `frost://` form) in the address bar.
        fn load_loopback(&mut self, loopback_url: &str, display_url: &str) {
            match self.webview.load(loopback_url) {
                Ok(()) => {
                    // Round 4, F-008: mark the load in flight NOW, not
                    // when LoadStarted fires — a navigation whose first
                    // response never arrives (hung server) never fires
                    // LoadStarted at all (Servo only reports Started
                    // when a Document is initialized), so without this
                    // the Stop button would stay dead for exactly the
                    // hang Stop is meant to escape.
                    self.loading = true;
                    self.awaiting_load = true;
                    self.loading_since = Some(std::time::Instant::now());
                    self.address = display_url.to_string();
                    self.address_editing = false;
                    self.content_focused = true;
                    self.status = format!("Loading {display_url} …");
                },
                Err(e) => self.status = format!("Cannot open {display_url}: {e}"),
            }
        }

        /// Load an internal page by its `frost://` URL (menu entries,
        /// address bar). The engine cannot load the `frost:` scheme,
        /// so the URL is translated to the loopback server's address
        /// before loading; the address bar keeps the `frost://` form.
        fn open_internal(&mut self, frost_url: &str) {
            self.menu_open = false;
            self.resize_webview();
            match crate::server::frost_url_to_loopback(frost_url, &self.server.base_url) {
                Some(loopback) => self.load_loopback(&loopback, frost_url),
                None => self.status = format!("Cannot open {frost_url}: unknown internal page"),
            }
        }

        /// Load the file-browser listing for `path` through the
        /// internal server (Round 3: the listing is served at
        /// `{base}/open?path=...`, so the address bar shows
        /// `frost://open?path=...`. Folder clicks are plain server
        /// navigations; file clicks are `file://` URLs, which Servo
        /// loads natively; `.md` files render as HTML).
        fn open_file_browser(&mut self, path: &Path) {
            self.menu_open = false;
            self.resize_webview();
            self.last_open_path = Some(path.to_path_buf());
            let loopback = file_browser::listing_url(path, &self.server.base_url);
            let display = crate::server::loopback_url_to_frost(&loopback, &self.server.base_url)
                .unwrap_or(loopback.clone());
            self.load_loopback(&loopback, &display);
            self.status = format!("Files: {}", path.display());
        }

        /// Open the inspector for the current page (Round 5): capture
        /// the page's DOM first — navigating to frost://inspector
        /// would replace the page being inspected, so the snapshot is
        /// taken BEFORE the navigation, and the inspector page renders
        /// that snapshot (labeled with its URL and capture time). The
        /// inspector opens from the Tick once the walker JSON arrives
        /// (or the capture errors, and the page stays put).
        fn open_inspector(&mut self) {
            if self.inspector_shot_pending {
                return;
            }
            let Some(current) = self.webview.url().map(|u| u.to_string()) else {
                self.status = "Inspector: no page loaded yet.".to_string();
                return;
            };
            let display = crate::server::loopback_url_to_frost(&current, &self.server.base_url)
                .unwrap_or(current);
            if display == "frost://inspector" {
                return; // Already looking at the inspector.
            }
            let (tx, rx) = std::sync::mpsc::channel();
            self.webview
                .evaluate_javascript_string(DOM_WALK_SCRIPT, tx);
            self.inspector_shot_rx = Some(rx);
            self.inspector_shot_pending = true;
            self.inspector_capture_url = Some(display);
            self.status = "Capturing page structure…".to_string();
        }

        /// Run a pending inspector re-capture, if the page that just
        /// loaded is the one asked for (Round 5). The flag is cleared
        /// either way: a capture for a different URL is dropped, not
        /// queued — the snapshot header always names the page it
        /// actually captured.
        fn maybe_capture_inspector(&mut self) {
            let Some(expected) = self.pending_inspector_capture.take() else {
                return;
            };
            let current = self
                .webview
                .url()
                .map(|u| u.to_string())
                .unwrap_or_default();
            let display = crate::server::loopback_url_to_frost(&current, &self.server.base_url)
                .unwrap_or(current);
            if display == expected {
                self.open_inspector();
            }
        }

        /// Inject the content scripts that match the current page URL.
        /// Runs once per URL, on LoadFinished (and on the 15 s safety
        /// net, for history traversals that never fire LoadFinished).
        /// Internal pages (`frost://` / loopback) never get scripts —
        /// they are Frost's own UI. `DocumentStart` scripts cannot run
        /// at document start (no navigation hook in the engine yet), so
        /// v1 runs every script at load finish; the sample extension
        /// uses `DocumentIdle` and is unaffected.
        fn inject_content_scripts(&mut self) {
            let Some(url) = self.webview.url().map(|u| u.to_string()) else {
                return;
            };
            if self.injected_for.as_deref() == Some(url.as_str()) {
                return;
            }
            self.injected_for = Some(url.clone());
            if url.starts_with("frost://")
                || crate::server::is_loopback_url(&url, &self.server.base_url)
            {
                return;
            }
            let scripts = self.host.content_scripts_for(&url);
            for (_, script) in scripts {
                self.webview.evaluate_javascript(&script.js);
            }
        }
    }

    impl cosmic::Application for Frost {
        type Executor = executor::Default;
        type Flags = ();
        type Message = Message;

        const APP_ID: &'static str = "dev.frost.Frost";

        fn core(&self) -> &Core {
            &self.core
        }

        fn core_mut(&mut self) -> &mut Core {
            &mut self.core
        }

        fn init(mut core: Core, _flags: ()) -> (Self, Task<Message>) {
            // Frost draws its own titlebar (see TITLEBAR_HEIGHT): the
            // built-in libcosmic headerbar renders the title but no
            // window-control buttons on this setup, so it is hidden
            // rather than left as an empty strip.
            core.window.show_headerbar = false;

            // Settings first: they decide the homepage and the theme.
            let settings = FrostSettings::load();
            let homepage = if crate::server::is_loadable_homepage(&settings.homepage) {
                settings.homepage.clone()
            } else {
                eprintln!(
                    "frost: homepage {:?} is not a loadable URL; using {HOME_URL}",
                    settings.homepage
                );
                HOME_URL.to_string()
            };

            // The internal-page server (frost:// settings/print/exit/
            // open/extensions). The bind is on an ephemeral loopback
            // port and cannot realistically fail; if it does, the
            // browser cannot serve its own pages, so fail loudly.
            let shared = Arc::new(Mutex::new(Shared::new(settings.clone())));
            let server = InternalServer::start(
                Arc::clone(&shared),
                FrostSettings::config_path(),
            )
            .expect("frost: could not start the internal-page server");
            if let Ok(mut shared) = shared.lock() {
                shared.base_url = server.base_url.clone();
            }

            // Extensions: load from every candidate directory, log and
            // skip the bad ones (never crash on a bad manifest), and
            // publish the snapshot for the frost://extensions page.
            let (host, loaded, failed) = load_extensions();
            if let Ok(mut shared) = shared.lock() {
                shared.extensions = loaded;
                shared.failed_extensions = failed;
            }

            let engine = Engine::new().expect("frost: could not create the Servo engine");
            let window_size = Size::new(INITIAL_WINDOW.0, INITIAL_WINDOW.1);
            let content_height =
                INITIAL_WINDOW.1 - TITLEBAR_HEIGHT - TOOLBAR_HEIGHT - STATUS_HEIGHT;
            let webview = engine
                .spawn_webview(&homepage, INITIAL_WINDOW.0 as u32, content_height as u32)
                .expect("frost: could not spawn the first webview");
            webview.set_theme(settings.theme.is_dark());
            (
                Self {
                    core,
                    engine,
                    webview,
                    frame_strips: Vec::new(),
                    frame_dirty: false,
                    loading: true,
                    address: homepage.clone(),
                    address_editing: false,
                    status: format!("Loading {homepage} …"),
                    title: None,
                    menu_open: false,
                    last_open_path: None,
                    can_back: false,
                    can_forward: false,
                    window_size,
                    last_cursor: None,
                    content_focused: true,
                    title_dirty: true,
                    awaiting_load: true,
                    loading_since: Some(std::time::Instant::now()),
                    print_shot_rx: None,
                    print_shot_pending: false,
                    inspector_shot_rx: None,
                    inspector_shot_pending: false,
                    inspector_capture_url: None,
                    pending_inspector_capture: None,
                    settings,
                    settings_version: 0,
                    server,
                    host,
                    injected_for: None,
                },
                Task::none(),
            )
        }

        fn subscription(&self) -> Subscription<Message> {
            Subscription::batch(vec![
                iced::time::every(TICK).map(|_| Message::Tick),
                event::listen().map(Message::IcedEvent),
            ])
        }

        fn on_window_resize(&mut self, _id: window::Id, width: f32, height: f32) {
            self.window_size = Size::new(width, height);
            self.resize_webview();
        }

        fn update(&mut self, message: Message) -> Task<Message> {
            match message {
                Message::Tick => {
                    self.engine.spin();
                    while let Some(event) = self.engine.try_recv_event() {
                        self.handle_engine_event(event);
                    }
                    // Settings changed through the settings page (the
                    // server bumps the shared version): apply them.
                    // Theme and search engine apply live; the homepage
                    // applies on next launch (the page says so). The
                    // same lock read also picks up an inspector
                    // re-capture request (Round 5): the flag is cleared
                    // here, and the URL to capture is remembered until
                    // that page loads again.
                    let (new_settings, new_version, recapture_url) = {
                        match self.server.shared.lock() {
                            Ok(mut shared) => {
                                let recapture_url = if shared.inspector_recapture_requested {
                                    shared.inspector_recapture_requested = false;
                                    shared
                                        .inspector_snapshot
                                        .as_ref()
                                        .map(|snap| snap.url.clone())
                                } else {
                                    None
                                };
                                (shared.settings.clone(), shared.settings_version, recapture_url)
                            },
                            Err(e) => (
                                e.into_inner().settings.clone(),
                                self.settings_version,
                                None,
                            ),
                        }
                    };
                    if new_version != self.settings_version {
                        let theme_changed = new_settings.theme != self.settings.theme;
                        self.settings = new_settings;
                        self.settings_version = new_version;
                        if theme_changed {
                            self.webview.set_theme(self.settings.theme.is_dark());
                        }
                    }
                    if let Some(url) = recapture_url {
                        self.pending_inspector_capture = Some(url);
                    }
                    // Poll the webview URL for a frost://open folder
                    // navigation: if the engine did not emit a
                    // UrlChanged event for it (an unknown-scheme load may
                    // not produce one), the poll still catches it and
                    // re-lists the folder. Guarded by last_open_path so
                    // one navigation is intercepted exactly once; cleared
                    // whenever the webview is on a non-open URL, so going
                    // Back onto a frost://open URL re-lists it instead
                    // of showing the engine's error page. Only the
                    // frost:// form is intercepted (Round 5 wart fix):
                    // the loopback /open form loads fine on its own.
                    if let Some(url) = self.webview.url() {
                        if file_browser::needs_open_intercept(&url) {
                            if let Some(path) = file_browser::parse_open_url(&url) {
                                if self.last_open_path.as_ref() != Some(&path) {
                                    self.open_file_browser(&path);
                                }
                            }
                        } else if file_browser::parse_open_url(&url).is_none() {
                            self.last_open_path = None;
                        }
                    }
                    // Safety net: a page that never fires LoadFinished
                    // must not leave a blank content area forever.
                    if self.awaiting_load {
                        if let Some(since) = self.loading_since {
                            if since.elapsed() > Duration::from_secs(15) {
                                // A history traversal (back/forward)
                                // restores from cache and never fires
                                // LoadFinished, so without this the status
                                // would read "Going back…" forever.
                                self.awaiting_load = false;
                                self.loading = false;
                                self.status = self
                                    .title
                                    .clone()
                                    .unwrap_or_else(|| "Done".to_string());
                                self.frame_dirty = true;
                                // The traversal's page still gets its
                                // content scripts (no LoadFinished fired).
                                self.inject_content_scripts();
                                // Round 5: a re-capture was requested
                                // and this traversal is the inspected
                                // page loading again — capture it.
                                self.maybe_capture_inspector();
                            }
                        }
                    }
                    // Render ONLY when the engine flagged a new frame AND
                    // no load is in flight (see awaiting_load): a
                    // mid-load frame would show a half-loaded page.
                    // Round 6: the frame is split into strips under
                    // iced's 2 MiB synchronous-upload line (see
                    // `Frame::split_strips` and the `frame_strips`
                    // field): one whole-frame image per render flashed
                    // dark for a frame on every load/scroll repaint.
                    if self.frame_dirty && !self.awaiting_load {
                        if let Some(frame) = self.webview.render() {
                            self.frame_strips = frame
                                .split_strips(MAX_SYNC_UPLOAD_BYTES)
                                .into_iter()
                                .map(|strip| {
                                    let height = strip.height as f32;
                                    (
                                        Handle::from_rgba(
                                            strip.width,
                                            strip.height,
                                            strip.rgba,
                                        ),
                                        height,
                                    )
                                })
                                .collect();
                            self.frame_dirty = false;
                        }
                    }
                    // F-003 print preview: the screenshot bytes arrive
                    // on this channel once rendering is up to date.
                    // Store them and only then open frost://print, so
                    // the page shows the preview (or the error) at
                    // once.
                    if let Some(rx) = &self.print_shot_rx {
                        if let Ok(result) = rx.try_recv() {
                            self.print_shot_rx = None;
                            self.print_shot_pending = false;
                            if let Ok(mut shared) = self.server.shared.lock() {
                                match result {
                                    Ok(bytes) => {
                                        shared.print_preview_png = Some(bytes);
                                        shared.print_preview_error = None;
                                    },
                                    Err(e) => {
                                        shared.print_preview_png = None;
                                        shared.print_preview_error = Some(e);
                                    },
                                }
                            }
                            self.open_internal("frost://print");
                        }
                    }
                    // Round 5 inspector: the walker JSON arrives on
                    // this channel once the script runs in the page.
                    // Parse it, store it, and only then open
                    // frost://inspector, so the page shows the tree at
                    // once. A failed capture leaves the current page
                    // up and says so in the status line.
                    if let Some(rx) = &self.inspector_shot_rx {
                        if let Ok(result) = rx.try_recv() {
                            self.inspector_shot_rx = None;
                            self.inspector_shot_pending = false;
                            let url =
                                self.inspector_capture_url.take().unwrap_or_default();
                            let parsed = result.and_then(|json| {
                                snapshot_from_json(&json).map(|snap| (json, snap))
                            });
                            match parsed {
                                Ok((json, snap)) => {
                                    if let Ok(mut shared) = self.server.shared.lock() {
                                        shared.inspector_snapshot = Some(InspectorSnapshot {
                                            url,
                                            captured_at_ms: snap.captured_at_unix_ms,
                                            node_count: snap.node_count,
                                            truncated: snap.truncated,
                                            json,
                                        });
                                    }
                                    self.open_internal("frost://inspector");
                                },
                                Err(e) => {
                                    self.status = format!("Inspector failed: {e}");
                                },
                            }
                        }
                    }
                    // Keep the OS window title in step with the titlebar.
                    if self.title_dirty {
                        self.title_dirty = false;
                        self.set_window_title(self.titlebar_text())
                    } else {
                        Task::none()
                    }
                },
                Message::IcedEvent(event) => {
                    self.handle_iced_event(event);
                    Task::none()
                },
                Message::Back => {
                    // Round 4, F-008: mark loading NOW (see
                    // load_loopback) so Stop is live even if this
                    // traversal never reports a load start.
                    self.loading = true;
                    self.awaiting_load = true;
                    self.loading_since = Some(std::time::Instant::now());
                    self.webview.go_back();
                    self.status = "Going back…".to_string();
                    Task::none()
                },
                Message::Forward => {
                    // Round 4, F-008: mark loading NOW (see
                    // load_loopback).
                    self.loading = true;
                    self.awaiting_load = true;
                    self.loading_since = Some(std::time::Instant::now());
                    self.webview.go_forward();
                    self.status = "Going forward…".to_string();
                    Task::none()
                },
                Message::Reload => {
                    self.loading = true;
                    self.awaiting_load = true;
                    self.loading_since = Some(std::time::Instant::now());
                    // Round 4, F-006: the fork's `reload()` now notifies
                    // the delegate via `set_load_status`, so the engine
                    // reports `LoadStarted` for a reload — which clears
                    // the content-script guard. The app-side workaround
                    // (clearing `injected_for` here) is RETIRED; setting
                    // `loading` here is kept so Stop stays live if the
                    // reload's first response never arrives (F-008).
                    self.webview.reload();
                    self.status = "Reloading…".to_string();
                    Task::none()
                },
                Message::Stop => {
                    match self.webview.stop() {
                        Ok(()) => self.status = "Stopping…".to_string(),
                        Err(e) => self.status = format!("Stop failed: {e}"),
                    }
                    Task::none()
                },
                Message::AddressChanged(value) => {
                    self.address = value;
                    self.address_editing = true;
                    // Typing in the bar means keys go to the bar, not the page.
                    self.content_focused = false;
                    Task::none()
                },
                Message::AddressSubmitted => {
                    let url = resolve_address(&self.address, &self.settings);
                    // A frost:// URL goes through the internal server.
                    if let Some(loopback) = url
                        .strip_prefix("frost://")
                        .and_then(|_| crate::server::frost_url_to_loopback(&url, &self.server.base_url))
                    {
                        self.load_loopback(&loopback, &url);
                    } else {
                        match self.webview.load(&url) {
                            Ok(()) => {
                                // Round 4, F-008: mark the load in
                                // flight NOW (see load_loopback) so Stop
                                // is live even if the server never
                                // responds and LoadStarted never fires.
                                self.loading = true;
                                self.awaiting_load = true;
                                self.loading_since = Some(std::time::Instant::now());
                                self.address = url.clone();
                                self.address_editing = false;
                                self.content_focused = true;
                                self.status = format!("Loading {url} …");
                            },
                            Err(e) => self.status = format!("Cannot load {url}: {e}"),
                        }
                    }
                    Task::none()
                },
                Message::ToggleMenu => {
                    self.menu_open = !self.menu_open;
                    self.resize_webview();
                    Task::none()
                },
                Message::OpenSettings => {
                    self.open_internal("frost://settings");
                    Task::none()
                },
                Message::OpenPrint => {
                    // F-003: capture the CURRENT page first — opening
                    // frost://print would replace it, and the preview
                    // must be of the page the user was reading. The
                    // print page opens from the Tick once the PNG
                    // bytes arrive (or the capture errors).
                    let (tx, rx) = std::sync::mpsc::channel();
                    self.webview.request_screenshot_png(tx);
                    self.print_shot_rx = Some(rx);
                    self.print_shot_pending = true;
                    self.status = "Capturing print preview…".to_string();
                    if let Ok(mut shared) = self.server.shared.lock() {
                        shared.print_preview_label =
                            format!("Preview of {}", self.address);
                        // Freeze the printed page's identity here too:
                        // by the time /print re-renders after a Save,
                        // the live current page is frost://print.
                        shared.print_page_title =
                            self.title.clone().unwrap_or_default();
                        shared.print_page_url = self.address.clone();
                        shared.print_saved_path = None;
                    }
                    Task::none()
                },
                Message::OpenInspector => {
                    // Round 5: capture the current page's DOM, then
                    // open frost://inspector on the snapshot (see
                    // open_inspector).
                    self.open_inspector();
                    Task::none()
                },
                Message::OpenFile => {
                    self.open_file_browser(&file_browser::home_dir());
                    Task::none()
                },
                Message::Exit => {
                    if self.settings.confirm_exit {
                        self.open_internal("frost://exit");
                    } else {
                        std::process::exit(0);
                    }
                    Task::none()
                },
                // Titlebar window controls: real iced window commands,
                // never decoration. Minimize / maximize / close are the
                // same commands libcosmic's own headerbar issues.
                Message::TitlebarMinimize => self.minimize(),
                Message::TitlebarMaximize => {
                    let id = self
                        .core()
                        .main_window_id()
                        .unwrap_or(window::Id::RESERVED);
                    cosmic::command::toggle_maximize(id)
                },
                Message::TitlebarClose => {
                    // The same Close action libcosmic's own headerbar
                    // close button produces.
                    Task::done(cosmic::Action::Cosmic(cosmic::app::Action::Close))
                },
                Message::TitlebarDrag => self.drag(),
                Message::TitlebarDoubleClick => {
                    let id = self
                        .core()
                        .main_window_id()
                        .unwrap_or(window::Id::RESERVED);
                    cosmic::command::toggle_maximize(id)
                },
            }
        }

        fn view(&self) -> Element<'_, Message> {
            // Frost's own chrome is styled explicitly from the settings
            // theme (see `chrome_container_style`): libcosmic's global
            // theme is not settable at runtime, so the titlebar /
            // toolbar / menu / status carry the theme colors themselves.
            // The buttons and the text field keep the global (dark)
            // styling in both modes — readable on either chrome.
            let colors = self.settings.theme.chrome_colors();
            // Titlebar (Frost's own — the built-in libcosmic headerbar
            // is hidden): browser name + current site title, and real
            // minimize / maximize-restores / close buttons. Text glyphs,
            // not icons, so the buttons are visible on any setup.
            // Pressing the title (not a button) drags the window;
            // double-press toggles maximize. The buttons sit outside
            // the drag area, so a click on one presses the button.
            let title = mouse_area(
                container(text(self.titlebar_text()).size(15))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill),
            )
            .on_press(Message::TitlebarDrag)
            .on_double_press(Message::TitlebarDoubleClick);
            let titlebar_row = widget::row::with_children(vec![
                // Left spacer, as wide as the button group, so the title
                // stays centered in the remaining space.
                widget::Space::new()
                    .width(Length::Fixed(3.0 * 40.0 + 2.0 * 4.0))
                    .into(),
                title.into(),
                button::standard("—")
                    .on_press(Message::TitlebarMinimize)
                    .width(Length::Fixed(40.0))
                    .into(),
                button::standard("▢")
                    .on_press(Message::TitlebarMaximize)
                    .width(Length::Fixed(40.0))
                    .into(),
                button::standard("✕")
                    .on_press(Message::TitlebarClose)
                    .width(Length::Fixed(40.0))
                    .into(),
            ])
            .spacing(4)
            .padding([0, 4, 0, 4])
            .height(Length::Fixed(TITLEBAR_HEIGHT));
            let titlebar = container(titlebar_row)
                .style(move |_| chrome_container_style(colors))
                .width(Length::Fill)
                .height(Length::Fixed(TITLEBAR_HEIGHT));

            // Toolbar: back / forward / reload / stop, address bar, menu.
            // Back/forward/stop only get an on_press when they can act —
            // without one the button renders disabled, never live-looking
            // while doing nothing.
            let back = {
                let b = button::standard("‹");
                if self.can_back {
                    b.on_press(Message::Back)
                } else {
                    b
                }
            };
            let forward = {
                let b = button::standard("›");
                if self.can_forward {
                    b.on_press(Message::Forward)
                } else {
                    b
                }
            };
            let stop = {
                let b = button::standard("✕");
                if self.loading {
                    b.on_press(Message::Stop)
                } else {
                    b
                }
            };
            let toolbar_row = widget::row::with_children(vec![
                back.into(),
                forward.into(),
                button::standard("⟳").on_press(Message::Reload).into(),
                stop.into(),
                text_input("Address", &self.address)
                    .on_input(Message::AddressChanged)
                    .on_submit(|_| Message::AddressSubmitted)
                    .width(iced::Length::Fill)
                    .into(),
                button::standard("☰").on_press(Message::ToggleMenu).into(),
            ])
            .spacing(8)
            .padding(8)
            .height(Length::Fixed(TOOLBAR_HEIGHT));
            let toolbar = container(toolbar_row)
                .style(move |_| chrome_container_style(colors))
                .width(Length::Fill)
                .height(Length::Fixed(TOOLBAR_HEIGHT));

            // Hamburger menu: fixed height when open so the content area
            // below it has an exact, known rectangle.
            let menu: Element<Message> = if self.menu_open {
                container(
                    widget::column::with_children(vec![
                        button::text("Settings").on_press(Message::OpenSettings).into(),
                        button::text("Print page…").on_press(Message::OpenPrint).into(),
                        button::text("Inspector").on_press(Message::OpenInspector).into(),
                        button::text("Open file…").on_press(Message::OpenFile).into(),
                        button::text("Exit").on_press(Message::Exit).into(),
                    ])
                    .spacing(4),
                )
                .style(move |_| chrome_container_style(colors))
                .padding(8)
                .width(Length::Fill)
                .height(Length::Fixed(MENU_HEIGHT))
                .into()
            } else {
                widget::column::with_children(vec![]).into()
            };

            // Web content area: the live Servo frame as horizontal
            // strips (Round 6), or a loading note until the first
            // frame paints. The strips tile the area exactly — each is
            // the frame's full width at its own height, no scaling, no
            // seams — so the column is the whole frame at 1:1.
            let content: Element<Message> = if self.frame_strips.is_empty() {
                container(text("Loading…").size(16))
                    .style(move |_| chrome_container_style(colors))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .into()
            } else {
                widget::column::with_children(
                    self.frame_strips
                        .iter()
                        .map(|(handle, height)| {
                            Image::new(handle.clone())
                                .width(Length::Fill)
                                .height(Length::Fixed(*height))
                                .content_fit(ContentFit::Fill)
                                .into()
                        })
                        .collect::<Vec<Element<'_, Message>>>(),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
            };

            let status = container(text(&self.status).size(13))
                .style(move |_| chrome_container_style(colors))
                .width(Length::Fill)
                .height(Length::Fixed(STATUS_HEIGHT))
                .padding(6);

            container(
                widget::column::with_children(vec![
                    titlebar.into(),
                    toolbar.into(),
                    menu,
                    content,
                    status.into(),
                ]),
            )
            .style(move |_| chrome_container_style(colors))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        }
    }

    /// Explicit chrome styling for one theme (see `ThemeChoice` in
    /// `settings.rs`: libcosmic's global theme is not settable at
    /// runtime, so Frost styles its own titlebar / toolbar / menu /
    /// status containers with the theme's background and text color;
    /// text inside a container inherits the container's text color).
    fn chrome_container_style(colors: ChromeColors) -> cosmic::iced::widget::container::Style {
        cosmic::iced::widget::container::Style {
            background: Some(cosmic::iced::Background::Color(cosmic::iced::Color::from_rgb8(
                colors.background[0],
                colors.background[1],
                colors.background[2],
            ))),
            text_color: Some(cosmic::iced::Color::from_rgb8(
                colors.text[0],
                colors.text[1],
                colors.text[2],
            )),
            ..Default::default()
        }
    }

    /// Address-bar resolution: a URL with a loadable scheme is used
    /// as-is; a bare hostname (no spaces, has a dot) gets `https://`;
    /// anything else is a search query with the default search engine.
    /// An empty bar loads the homepage.
    fn resolve_address(input: &str, settings: &FrostSettings) -> String {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return settings.homepage.clone();
        }
        if let Ok(url) = Url::parse(trimmed) {
            if matches!(url.scheme(), "http" | "https" | "data" | "file" | "frost") {
                return url.to_string();
            }
        }
        if !trimmed.contains(' ') && trimmed.contains('.') {
            return format!("https://{trimmed}");
        }
        settings.search_engine.search_url(trimmed)
    }

    /// Load extensions from every candidate directory, in order:
    /// `$FROST_EXTENSIONS_DIR` (colon-separated), then each
    /// subdirectory of `~/.config/frost/extensions/`, then each
    /// subdirectory of `./extensions/` (the repo ships a sample there).
    /// Returns the host plus the (name, version, enabled) snapshot and
    /// the (directory, error) failures, for the `frost://extensions`
    /// page. A bad manifest is logged and skipped — it never stops the
    /// browser or the other extensions.
    fn load_extensions() -> (
        StubHost,
        Vec<(String, String, bool)>,
        Vec<(String, String)>,
    ) {
        let mut host = StubHost::new();
        let mut loaded = Vec::new();
        let mut failed = Vec::new();
        let mut dirs: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(env_dirs) = std::env::var("FROST_EXTENSIONS_DIR") {
            for dir in env_dirs.split(':') {
                if !dir.is_empty() {
                    dirs.push(std::path::PathBuf::from(dir));
                }
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            let config_dir =
                std::path::PathBuf::from(home).join(".config/frost/extensions");
            if let Ok(entries) = std::fs::read_dir(&config_dir) {
                for entry in entries.flatten() {
                    if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        dirs.push(entry.path());
                    }
                }
            }
        }
        if let Ok(entries) = std::fs::read_dir("extensions") {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    dirs.push(entry.path());
                }
            }
        }
        for dir in dirs {
            match ExtensionManifest::load_from_dir(&dir) {
                Ok(manifest) => {
                    let name = manifest.name.clone();
                    let version = manifest.version.clone();
                    match host.load(manifest) {
                        Ok(_) => {
                            eprintln!(
                                "frost: loaded extension {name} {version} from {}",
                                dir.display()
                            );
                            loaded.push((name, version, true));
                        },
                        Err(e) => {
                            eprintln!("frost: skipping {}: {e}", dir.display());
                            failed.push((dir.display().to_string(), e.to_string()));
                        },
                    }
                },
                Err(e) => {
                    eprintln!("frost: skipping {}: {e}", dir.display());
                    failed.push((dir.display().to_string(), e.to_string()));
                },
            }
        }
        (host, loaded, failed)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::settings::SearchEngine;

        #[test]
        fn resolve_address_keeps_urls_and_searches_the_rest() {
            let settings = FrostSettings::default();
            assert_eq!(
                resolve_address("https://example.org", &settings),
                "https://example.org/"
            );
            assert_eq!(
                resolve_address("frost://settings", &settings),
                "frost://settings"
            );
            assert_eq!(
                resolve_address("example.org", &settings),
                "https://example.org"
            );
            assert_eq!(
                resolve_address("hello world", &settings),
                "https://duckduckgo.com/?q=hello%20world"
            );
            assert_eq!(
                resolve_address("rust", &settings),
                "https://duckduckgo.com/?q=rust"
            );
            assert_eq!(resolve_address("", &settings), "https://example.org");
            let mut google = FrostSettings::default();
            google.search_engine = SearchEngine::Google;
            assert_eq!(
                resolve_address("hello world", &google),
                "https://www.google.com/search?q=hello%20world"
            );
        }
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let settings = Settings::default().size(Size::new(INITIAL_WINDOW.0, INITIAL_WINDOW.1));
        cosmic::app::run::<Frost>(settings, ())?;
        Ok(())
    }
}

#[cfg(feature = "ui-libcosmic")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::run()
}

#[cfg(not(feature = "ui-libcosmic"))]
fn main() {
    eprintln!("frost: libcosmic UI is not enabled in this build.");
    eprintln!("frost: run with `cargo run -p frost --features ui-libcosmic` for the window.");
    eprintln!("frost: the engine is headless — prove it with:");
    eprintln!("frost:   cargo run -p frost-engine --example smoke");
}
