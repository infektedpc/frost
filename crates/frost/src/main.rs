//! Frost browser — libcosmic application shell.
//!
//! Round 2 wires the engine in for real:
//!
//! * The content area shows the live Servo frame, uploaded into an iced
//!   texture whenever the engine reports `FrameReady` (or while loading).
//! * The toolbar navigates for real: back / forward (disabled when the
//!   engine says there is nothing to go to), reload, stop (live since fork
//!   patch F-001), address bar (Enter navigates; bare hostnames get
//!   `https://`).
//! * Input forwards into the page: the app listens to all iced events and
//!   translates the ones landing in the content area into
//!   [`frost_engine::FrostInputEvent`]s. Keyboard goes to the page only
//!   while the content area has focus (a click in the content, or Enter in
//!   the address bar); typing in the address bar stays in the bar.
//! * The hamburger menu opens as a dropdown; Settings / Print load
//!   placeholder pages through the engine (real `frost://` pages are
//!   Round 3); Open is a real file browser — the listing is generated
//!   app-side, folder clicks re-list, file clicks open the file in the
//!   browser; Exit quits the process.
//! * Frost draws its own titlebar (the built-in libcosmic headerbar is
//!   hidden — it renders the title but no window-control buttons on this
//!   setup): "Frost — <page title>" with real minimize, maximize/restore,
//!   and close buttons (text glyphs, real iced window commands).
//!
//! Chrome geometry is fixed by construction (Frost titlebar 40, toolbar
//! 48, menu 136 when open, status 28), so the webview size and the click
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
mod app {
    use std::path::Path;
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
        data_url_page, Engine, EngineEvent, FrostCode, FrostInputEvent, FrostKeyState,
        FrostMouseButtonAction, FrostWebView,
    };
    use url::Url;

    use crate::file_browser;
    use crate::keymap::{map_button, map_code, map_key, map_location, map_modifiers};

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
    const MENU_HEIGHT: f32 = 136.0;
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

    /// Phase of the post-load forced repaint (see `start_repaint_jiggle`).
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum RepaintPhase {
        /// No forced repaint in flight.
        Idle,
        /// Resized 1 px wider; waiting for the frame that resize produces.
        WaitFirstFrame,
        /// Resized back to the content size; waiting for that frame.
        WaitSecondFrame,
    }

    pub struct Frost {
        core: Core,
        engine: Engine,
        webview: FrostWebView,
        /// Latest uploaded frame, if one has painted yet.
        frame: Option<Handle>,
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
        /// The first paint of a load is the one that decides the frame:
        /// rendering while the display list is still being built leaves
        /// the frame partially painted (~1024x512 of a 1280x676 frame)
        /// and no later paint repairs it, because the background damage
        /// is already spent. So the app renders nothing until
        /// LoadFinished (the smoke test's pattern: one render after
        /// load + FrameReady always paints fully). The old frame stays
        /// up during a navigation until the new one is ready.
        awaiting_load: bool,
        /// When the current load started; bounds the wait so a page that
        /// never fires LoadFinished cannot leave a blank content area
        /// forever (after 15 s renders are allowed anyway).
        loading_since: Option<std::time::Instant>,
        /// A post-load forced repaint is in flight (see
        /// `start_repaint_jiggle`); no frame may be uploaded until it
        /// completes.
        repaint_pending: bool,
        /// Which step of the forced repaint is running.
        repaint_phase: RepaintPhase,
        /// Ticks waited for the current repaint step's frame; bounds the
        /// wait so a missing FrameReady can never wedge the display.
        repaint_wait_ticks: u32,
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
        /// While a forced repaint is running the jiggle owns the webview
        /// size, so a window resize during it is deferred (the jiggle's
        /// resize-back uses the current content size anyway).
        fn resize_webview(&mut self) {
            if self.repaint_pending {
                return;
            }
            let (w, h) = self.content_size();
            if self.webview.size() != (w, h) {
                self.webview.resize(w, h);
                self.frame_dirty = true;
            }
        }

        /// Force a full repaint after a load (two-phase resize jiggle).
        ///
        /// A load's first paint is partial — only a ~1024x512 region of
        /// a 1280x684 frame — and no later paint repairs it, even a
        /// render after LoadFinished (the load gate alone does not fix
        /// it). What DOES repair it is a real resize to a different
        /// size: maximizing to 1600x900 painted the whole 1600x784
        /// frame, restoring to 1280x800 painted the whole 1280x684
        /// frame, and even a 1 px window resize paints fully. So the
        /// webview itself must be resized: a same-size resize is a
        /// no-op in Servo (the painter dedupes it). This jiggle
        /// therefore does it in two steps, one per timed wait: resize
        /// 1 px wider, wait ~0.25 s, resize back to the content size,
        /// wait ~0.25 s, then upload. The intermediate size is never
        /// uploaded, so the user never sees it. The waits are timed,
        /// not FrameReady-driven: advancing on a FrameReady resized
        /// straight back on a stale (pre-resize) event, coalescing the
        /// two resizes into a net no-op. This works around a
        /// damage/initialization gap in the engine (same family as the
        /// WayServo ghost finding that dirty rects under-report); a
        /// proper full-repaint trigger in the fork is the follow-up.
        fn start_repaint_jiggle(&mut self) {
            let (w, h) = self.content_size();
            self.webview.resize(w + 1, h);
            self.repaint_phase = RepaintPhase::WaitFirstFrame;
            self.repaint_pending = true;
            self.repaint_wait_ticks = 0;
            // Discard any pre-jiggle frame flag: the frame it names is
            // the stale partial one.
            self.frame_dirty = false;
        }

        /// Advance the forced repaint one step (called by the Tick
        /// once the current step's timed wait has elapsed). The Tick's
        /// render gate uploads the final frame once the jiggle
        /// completes.
        fn advance_repaint_jiggle(&mut self) {
            match self.repaint_phase {
                RepaintPhase::WaitFirstFrame => {
                    // The wider frame arrived: resize back to the
                    // content size (which may have changed if the
                    // window was resized mid-jiggle) and wait for it.
                    let (w, h) = self.content_size();
                    self.webview.resize(w, h);
                    self.repaint_phase = RepaintPhase::WaitSecondFrame;
                    self.repaint_wait_ticks = 0;
                    self.frame_dirty = false;
                },
                RepaintPhase::WaitSecondFrame => {
                    // The back-to-size frame arrived: the jiggle is
                    // done; the Tick uploads it.
                    self.repaint_phase = RepaintPhase::Idle;
                    self.repaint_pending = false;
                    self.frame_dirty = true;
                },
                RepaintPhase::Idle => {},
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
                    self.title = title;
                    self.title_dirty = true;
                    if !self.loading {
                        self.status = self
                            .title
                            .clone()
                            .unwrap_or_else(|| "Untitled page".to_string());
                    }
                },
                EngineEvent::UrlChanged(url) => {
                    if !self.address_editing {
                        self.address = url.to_string();
                    }
                    self.title_dirty = true;
                    self.can_back = self.webview.can_go_back();
                    self.can_forward = self.webview.can_go_forward();
                    // A folder click in the file browser navigates to
                    // frost://open?path=... — re-list that folder instead
                    // of leaving the engine on an unloadable URL.
                    if let Some(path) = file_browser::parse_open_url(&url) {
                        self.open_file_browser(&path);
                    }
                },
                EngineEvent::LoadStarted => {
                    self.loading = true;
                    self.awaiting_load = true;
                    self.loading_since = Some(std::time::Instant::now());
                    self.status = format!("Loading {} …", self.address);
                },
                EngineEvent::LoadFinished => {
                    self.loading = false;
                    self.awaiting_load = false;
                    self.loading_since = None;
                    self.address_editing = false;
                    self.status = self.title.clone().unwrap_or_else(|| "Done".to_string());
                    // The load's first paint is partial; force a full
                    // repaint before any frame is uploaded.
                    self.start_repaint_jiggle();
                },
                EngineEvent::LoadStopped => {
                    self.loading = false;
                    self.awaiting_load = false;
                    self.loading_since = None;
                    self.status = "Stopped.".to_string();
                    self.start_repaint_jiggle();
                },
                EngineEvent::FrameReady => {
                    if self.repaint_pending {
                        // The jiggle is time-driven (see the Tick): a
                        // FrameReady arriving during it may be stale
                        // (queued before the jiggle's resize), so it must
                        // not advance the jiggle or flag a frame — the
                        // frame it names is the stale partial one.
                    } else {
                        self.frame_dirty = true;
                    }
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
                    repeat,
                    ..
                }) => {
                    if self.content_focused {
                        self.send_key(FrostKeyState::Down, &key, physical_key, location, modifiers, repeat);
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
        ) {
            let code = match physical_key {
                keyboard::key::Physical::Code(code) => map_code(code),
                keyboard::key::Physical::Unidentified(_) => FrostCode::Unidentified,
            };
            self.webview.send_input(FrostInputEvent::Key {
                state,
                key: map_key(key),
                code,
                location: map_location(location),
                modifiers: map_modifiers(modifiers),
                repeat,
            });
        }

        /// Load a placeholder page for a menu entry (Settings / Print;
        /// Round 3 replaces these with the real `frost://` internal
        /// pages. Open is a real file browser — see `open_file_browser`).
        fn open_placeholder(&mut self, title: &str, body_html: &str) {
            self.menu_open = false;
            self.resize_webview();
            let url = data_url_page(title, body_html);
            match self.webview.load(&url) {
                Ok(()) => {
                    self.awaiting_load = true;
                    self.loading_since = Some(std::time::Instant::now());
                    self.address_editing = false;
                    self.content_focused = true;
                    self.status = format!("{title} (placeholder — real page lands in Round 3)");
                },
                Err(e) => self.status = format!("Cannot open {title}: {e}"),
            }
        }

        /// Load the file-browser listing for `path` through the engine
        /// (Round 2: the listing is generated app-side by
        /// `file_browser::listing_page` and loaded as a data: page.
        /// Folder clicks come back as `frost://open` navigations,
        /// intercepted in `handle_engine_event` and the Tick's URL
        /// poll; file clicks are plain `file://` URLs, which Servo
        /// loads natively).
        fn open_file_browser(&mut self, path: &Path) {
            self.menu_open = false;
            self.resize_webview();
            self.last_open_path = Some(path.to_path_buf());
            let (title, body) = file_browser::listing_page(path);
            let url = data_url_page(&title, &body);
            match self.webview.load(&url) {
                Ok(()) => {
                    self.awaiting_load = true;
                    self.loading_since = Some(std::time::Instant::now());
                    self.address_editing = false;
                    self.content_focused = true;
                    self.status = format!("Files: {}", path.display());
                },
                Err(e) => self.status = format!("Cannot open {}: {e}", path.display()),
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
            let engine = Engine::new().expect("frost: could not create the Servo engine");
            let window_size = Size::new(INITIAL_WINDOW.0, INITIAL_WINDOW.1);
            let content_height =
                INITIAL_WINDOW.1 - TITLEBAR_HEIGHT - TOOLBAR_HEIGHT - STATUS_HEIGHT;
            let webview = engine
                .spawn_webview(HOME_URL, INITIAL_WINDOW.0 as u32, content_height as u32)
                .expect("frost: could not spawn the first webview");
            (
                Self {
                    core,
                    engine,
                    webview,
                    frame: None,
                    frame_dirty: false,
                    loading: true,
                    address: HOME_URL.to_string(),
                    address_editing: false,
                    status: format!("Loading {HOME_URL} …"),
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
                    repaint_pending: false,
                    repaint_phase: RepaintPhase::Idle,
                    repaint_wait_ticks: 0,
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
                    // Poll the webview URL for a frost://open folder
                    // navigation: if the engine did not emit a
                    // UrlChanged event for it (an unknown-scheme load may
                    // not produce one), the poll still catches it and
                    // re-lists the folder. Guarded by last_open_path so
                    // one navigation is intercepted exactly once; cleared
                    // whenever the webview is on a non-frost://open URL,
                    // so going Back onto a frost://open URL re-lists it
                    // instead of showing the engine's error page.
                    if let Some(url) = self.webview.url() {
                        match file_browser::parse_open_url(&url) {
                            Some(path)
                                if self.last_open_path.as_ref() != Some(&path) =>
                            {
                                self.open_file_browser(&path);
                            },
                            None => self.last_open_path = None,
                            _ => {},
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
                            }
                        }
                    }
                    // The forced repaint is time-driven, not event-driven:
                    // a FrameReady arriving during the jiggle may be stale
                    // (queued before the jiggle's resize — advancing on it
                    // resized straight back before the constellation had
                    // processed the wider resize, coalescing the two into
                    // a net no-op, which is why the event-driven jiggle
                    // still left partial frames). Each step therefore
                    // waits ~15 ticks (~0.25 s, far more than the 1–2
                    // spins the constellation needs to process a resize)
                    // before advancing, so the resize is certainly
                    // processed before the next step.
                    if self.repaint_pending {
                        self.repaint_wait_ticks += 1;
                        if self.repaint_wait_ticks >= 15 {
                            self.advance_repaint_jiggle();
                        }
                    }
                    // Render ONLY when the engine flagged a new frame AND
                    // no load is in flight (see awaiting_load) AND no
                    // forced repaint is running: the jiggle's intermediate
                    // size must never be uploaded, and a pre-jiggle
                    // partial frame must never reach the screen.
                    if self.frame_dirty && !self.awaiting_load && !self.repaint_pending {
                        if let Some(frame) = self.webview.render() {
                            self.frame = Some(Handle::from_rgba(
                                frame.width,
                                frame.height,
                                frame.rgba,
                            ));
                            self.frame_dirty = false;
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
                    self.awaiting_load = true;
                    self.loading_since = Some(std::time::Instant::now());
                    self.webview.go_back();
                    self.status = "Going back…".to_string();
                    Task::none()
                },
                Message::Forward => {
                    self.awaiting_load = true;
                    self.loading_since = Some(std::time::Instant::now());
                    self.webview.go_forward();
                    self.status = "Going forward…".to_string();
                    Task::none()
                },
                Message::Reload => {
                    self.awaiting_load = true;
                    self.loading_since = Some(std::time::Instant::now());
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
                    let url = normalize_url(&self.address, HOME_URL);
                    match self.webview.load(&url) {
                        Ok(()) => {
                            self.awaiting_load = true;
                            self.loading_since = Some(std::time::Instant::now());
                            self.address = url;
                            self.address_editing = false;
                            self.content_focused = true;
                        },
                        Err(e) => self.status = format!("Cannot load {url}: {e}"),
                    }
                    Task::none()
                },
                Message::ToggleMenu => {
                    self.menu_open = !self.menu_open;
                    self.resize_webview();
                    Task::none()
                },
                Message::OpenSettings => {
                    self.open_placeholder(
                        "Settings",
                        "<p>Settings arrive in Round 3 — theme, homepage, search engine, privacy.</p>\
                         <p><a href=\"https://example.org\">Back to example.org</a></p>",
                    );
                    Task::none()
                },
                Message::OpenPrint => {
                    self.open_placeholder(
                        "Print page",
                        "<p>Print preview arrives in Round 3 (fork patch F-003 or paginated-image export).</p>\
                         <p><a href=\"https://example.org\">Back to example.org</a></p>",
                    );
                    Task::none()
                },
                Message::OpenFile => {
                    self.open_file_browser(&file_browser::home_dir());
                    Task::none()
                },
                Message::Exit => std::process::exit(0),
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
            let titlebar = widget::row::with_children(vec![
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
            let toolbar = widget::row::with_children(vec![
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

            // Hamburger menu: fixed height when open so the content area
            // below it has an exact, known rectangle.
            let menu: Element<Message> = if self.menu_open {
                container(
                    widget::column::with_children(vec![
                        button::text("Settings").on_press(Message::OpenSettings).into(),
                        button::text("Print page…").on_press(Message::OpenPrint).into(),
                        button::text("Open file…").on_press(Message::OpenFile).into(),
                        button::text("Exit").on_press(Message::Exit).into(),
                    ])
                    .spacing(4),
                )
                .padding(8)
                .width(Length::Fill)
                .height(Length::Fixed(MENU_HEIGHT))
                .into()
            } else {
                widget::column::with_children(vec![]).into()
            };

            // Web content area: the live Servo frame, or a loading note
            // until the first frame paints.
            let content: Element<Message> = match &self.frame {
                Some(handle) => Image::new(handle.clone())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .content_fit(ContentFit::Fill)
                    .into(),
                None => container(text("Loading…").size(16))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .into(),
            };

            let status = container(text(&self.status).size(13))
                .width(Length::Fill)
                .height(Length::Fixed(STATUS_HEIGHT))
                .padding(6);

            widget::column::with_children(vec![
                titlebar.into(),
                toolbar.into(),
                menu,
                content,
                status.into(),
            ])
            .into()
        }
    }

    /// Address-bar normalization: a URL with a loadable scheme is used
    /// as-is; anything else is treated as a bare hostname and gets
    /// `https://`. An empty bar keeps the current page.
    fn normalize_url(input: &str, fallback: &str) -> String {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return fallback.to_string();
        }
        if let Ok(url) = Url::parse(trimmed) {
            if matches!(url.scheme(), "http" | "https" | "data" | "file" | "frost") {
                return url.to_string();
            }
        }
        format!("https://{trimmed}")
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
