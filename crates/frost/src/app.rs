mod windows;

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
use crate::settings::{is_loadable_homepage, ChromeColors, Settings as FrostSettings};
use windows::{InspectorWindow, OpenWindow, PopupMessage, PrintWindow, SettingsWindow};

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
/// Hamburger menu dropdown panel (Round 8): a compact Firefox-style
/// panel anchored top-right under the ☰ button, floating over the
/// page — NOT the full-width bar of Rounds 2–7, which pushed the
/// content down and spanned the whole window.
const MENU_PANEL_WIDTH: f32 = 300.0;
/// Right-edge margin of the dropdown panel.
const MENU_PANEL_MARGIN: f32 = 8.0;
/// Gap between the toolbar and the top of the dropdown panel.
const MENU_PANEL_GAP: f32 = 4.0;
const INITIAL_WINDOW: (f32, f32) = (1280.0, 800.0);
const HOME_URL: &str = "https://example.org";
/// Engine pump rate. The tick spins Servo, drains its events, and
/// re-renders only while loading or when a frame was flagged ready.
const TICK: Duration = Duration::from_millis(16);

/// The hamburger menu's entries, in order: (label, shortcut, message).
/// Firefox shows one shortcut (F12 for Inspector); the other entries
/// have no shortcut — do not invent any. The view builds the panel
/// from this table, so the spec and the rendering cannot drift.
/// Separators fall after "Settings" and after "Inspector" (groups:
/// Settings | Print page… / Open file… / Inspector | Exit).
const MENU_ITEMS: [(&str, Option<&str>, Message); 5] = [
    ("Settings", None, Message::OpenSettings),
    ("Print page…", None, Message::OpenPrint),
    ("Open file…", None, Message::OpenFile),
    ("Inspector", Some("F12"), Message::OpenInspector),
    ("Exit", None, Message::Exit),
];

/// (x, width) of the dropdown panel for a window of the given width:
/// fixed MENU_PANEL_WIDTH wide, MENU_PANEL_MARGIN from the right
/// edge. A window narrower than the panel shrinks the panel instead
/// of letting it run off-screen; x never goes negative.
fn menu_panel_geometry(window_width: f32) -> (f32, f32) {
    let width =
        MENU_PANEL_WIDTH.min((window_width - 2.0 * MENU_PANEL_MARGIN).max(1.0));
    let x = (window_width - width - MENU_PANEL_MARGIN).max(0.0);
    (x, width)
}

#[derive(Clone, Debug)]
pub enum Message {
    /// Pump the engine (spin + drain events + maybe re-render).
    Tick,
    /// A raw iced event (mouse / keyboard / window) for input forwarding.
    /// Carries the window it came from: only main-window events
    /// reach the page (Round 7 — popup windows have their own
    /// widgets, and their coordinates are not page coordinates).
    IcedEvent(window::Id, iced::Event),
    /// A window opened (reported by `window::open`'s task).
    WindowOpened(window::Id),
    /// A window closed (the OS/window manager closed it).
    WindowClosed(window::Id),
    /// A window's close was requested (OS close button): popups
    /// close, the main window quits.
    CloseWindow(window::Id),
    /// A message from one of the popup windows (Round 7).
    Popup(PopupMessage),
    Back,
    Forward,
    Reload,
    Stop,
    AddressChanged(String),
    AddressSubmitted,
    ToggleMenu,
    /// Close the hamburger dropdown without picking anything
    /// (Escape, or a press outside the panel).
    CloseMenu,
    /// The pointer entered (Some(i)) or left (None) a menu entry:
    /// drives the hover highlight.
    MenuHover(Option<usize>),
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
    /// Index of the menu entry under the pointer, for the hover
    /// highlight (Round 8). None when the pointer is not over an
    /// entry; cleared whenever the menu closes.
    menu_hovered: Option<usize>,
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
    /// arrive (or the capture errors) and the Print window opens.
    print_shot_rx: Option<std::sync::mpsc::Receiver<Result<Vec<u8>, String>>>,
    /// A print preview capture is in flight; the status line says
    /// so until the bytes arrive.
    print_shot_pending: bool,
    /// The printed page's identity, frozen at print-open time
    /// (label, title, URL) — the Print window renders from these.
    print_pending_label: String,
    print_pending_title: String,
    print_pending_url: String,
    /// An inspector DOM capture is in flight (Round 5): the channel
    /// the walker JSON arrives on, polled by the Tick. Set by
    /// `capture_inspector`; cleared when the JSON arrives (or the
    /// capture errors) and the Inspector window opens (or, on a
    /// re-capture, its snapshot is replaced in place).
    inspector_shot_rx: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    /// An inspector capture is in flight; the status line says so
    /// until the snapshot arrives.
    inspector_shot_pending: bool,
    /// A re-capture was requested from the Inspector window (Round
    /// 7): when the walker JSON arrives, replace the open
    /// window's snapshot instead of opening a new window. The
    /// inspected page never left the main window, so no navigation
    /// is needed.
    inspector_recapture: bool,
    /// Current settings (theme, homepage, search engine, exit
    /// confirmation). Loaded at startup; the native Settings
    /// window edits them and saves (Round 7 — no more server
    /// polling).
    settings: FrostSettings,
    /// (name, version, enabled) for every loaded extension, and
    /// (directory, error) for every directory that failed — shown
    /// in the Settings window's Extensions section (Round 7).
    extensions: Vec<(String, String, bool)>,
    failed_extensions: Vec<(String, String)>,
    /// The popup windows (Round 7), one per kind at most.
    settings_win: Option<SettingsWindow>,
    print_win: Option<PrintWindow>,
    open_win: Option<OpenWindow>,
    inspector_win: Option<InspectorWindow>,
    exit_win: Option<window::Id>,
    /// Content-script host (StubHost: in-memory, real disk loading
    /// + real injection through the engine — see
    /// `inject_content_scripts`).
    host: StubHost,
    /// URL content scripts were last injected for; a new navigation
    /// clears it so the new page gets its scripts.
    injected_for: Option<String>,
}

impl Frost {
    /// The content area's rectangle in window coordinates. Round 8:
    /// the hamburger menu is a floating panel over the content, so
    /// it no longer consumes layout space (Rounds 2–7 subtracted
    /// MENU_HEIGHT here).
    fn content_rect(&self) -> Rectangle {
        Rectangle {
            x: 0.0,
            y: TITLEBAR_HEIGHT + TOOLBAR_HEIGHT,
            width: self.window_size.width,
            height: (self.window_size.height
                - TITLEBAR_HEIGHT
                - TOOLBAR_HEIGHT
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

    /// Close the hamburger dropdown (and forget which entry was
    /// hovered). Every close path goes through here.
    fn close_menu(&mut self) {
        self.menu_open = false;
        self.menu_hovered = None;
    }

    /// The hamburger dropdown panel (Round 8): a compact panel
    /// anchored top-right under the ☰ button, floating over the
    /// page. Positioned by `menu_panel_geometry` from the live
    /// window width, so it stays glued to the right edge at any
    /// window size.
    fn menu_panel(&self, colors: ChromeColors) -> Element<'_, Message> {
        let (x, width) = menu_panel_geometry(self.window_size.width);
        let mut children: Vec<Element<'_, Message>> = Vec::new();
        for (i, (label, shortcut, message)) in MENU_ITEMS.iter().enumerate() {
            // Separators between groups: after Settings, after Inspector.
            if i == 1 || i == 4 {
                children.push(
                    container(widget::Space::new().height(Length::Fixed(1.0)))
                        .style(move |_| menu_separator_style(colors))
                        .width(Length::Fill)
                        .padding([4, 0])
                        .into(),
                );
            }
            children.push(menu_item(
                colors,
                i,
                self.menu_hovered == Some(i),
                label,
                *shortcut,
                message.clone(),
            ));
        }
        let panel = container(widget::column::with_children(children).spacing(2))
            .style(move |_| menu_panel_style(colors))
            .padding(6)
            .width(Length::Fixed(width));
        // The wrapper fills the window; left padding puts the
        // panel's left edge at x, so its right edge sits
        // MENU_PANEL_MARGIN from the window's right edge.
        container(panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(iced::Padding::from([
                TITLEBAR_HEIGHT + TOOLBAR_HEIGHT + MENU_PANEL_GAP,
                0.0,
                0.0,
                x,
            ]))
            .into()
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
            },
            EngineEvent::UrlChanged(url) => {
                // Round 7: no internal pages any more — the engine
                // URL is the display URL, full stop.
                let display = url.to_string();
                if !self.address_editing {
                    self.address = display;
                }
                self.title_dirty = true;
                self.can_back = self.webview.can_go_back();
                self.can_forward = self.webview.can_go_forward();
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
    /// Only events from the MAIN window reach the page (Round 7):
    /// popup windows have their own widgets, and their coordinates
    /// are in the popup's space, not the page's — forwarding them
    /// would click the page under the wrong point.
    fn handle_iced_event(
        &mut self,
        id: window::Id,
        event: iced::Event,
    ) -> Task<Message> {
        if self.core.main_window_id() != Some(id) {
            return Task::none();
        }
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
                // Round 8: Escape closes the dropdown menu, and the
                // key never reaches the page. Checked before F12
                // and before the content-focus gate.
                if !repeat
                    && self.menu_open
                    && matches!(key, keyboard::Key::Named(keyboard::key::Named::Escape))
                {
                    self.close_menu();
                    return Task::none();
                }
                // Round 5: F12 opens the Inspector — a browser
                // shortcut, not a page key. Checked before the
                // content-focus gate, so it works from the address
                // bar too; repeats are ignored (one capture per
                // press). The key never reaches the page. An
                // Inspector window that is already open is just
                // focused (Round 7).
                if !repeat
                    && matches!(key, keyboard::Key::Named(keyboard::key::Named::F12))
                {
                    if let Some(id) = self.popup_id(windows::PopupKind::Inspector) {
                        return window::gain_focus(id);
                    }
                    self.capture_inspector();
                } else if self.content_focused && !self.menu_open {
                    // Round 8: while the menu is open the page
                    // takes no keys — typing at a menu-covered page
                    // would land invisibly.
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
                if self.content_focused && !self.menu_open {
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
        Task::none()
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

    /// Capture the current page's DOM for the Inspector window
    /// (Round 5, native window since Round 7): the walker script
    /// runs in the page and its JSON arrives on the channel; the
    /// Tick opens the Inspector window on the snapshot (or replaces
    /// the open window's snapshot, on a re-capture). The page
    /// itself never leaves the main window, so there is no
    /// navigation to wait for and no "back to the page" step.
    fn capture_inspector(&mut self) {
        if self.inspector_shot_pending {
            return;
        }
        if self.webview.url().is_none() {
            self.status = "Inspector: no page loaded yet.".to_string();
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        self.webview
            .evaluate_javascript_string(DOM_WALK_SCRIPT, tx);
        self.inspector_shot_rx = Some(rx);
        self.inspector_shot_pending = true;
        self.status = "Capturing page structure…".to_string();
    }

    /// Inject the content scripts that match the current page URL.
    /// Runs once per URL, on LoadFinished (and on the 15 s safety
    /// net, for history traversals that never fire LoadFinished).
    /// `DocumentStart` scripts cannot run at document start (no
    /// navigation hook in the engine yet), so v1 runs every script
    /// at load finish; the sample extension uses `DocumentIdle`
    /// and is unaffected. Round 7: there are no internal pages
    /// any more, so every URL the engine loads is a real page —
    /// the old `frost://` / loopback guard is gone.
    fn inject_content_scripts(&mut self) {
        let Some(url) = self.webview.url().map(|u| u.to_string()) else {
            return;
        };
        if self.injected_for.as_deref() == Some(url.as_str()) {
            return;
        }
        self.injected_for = Some(url.clone());
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
        let homepage = if is_loadable_homepage(&settings.homepage) {
            settings.homepage.clone()
        } else {
            eprintln!(
                "frost: homepage {:?} is not a loadable URL; using {HOME_URL}",
                settings.homepage
            );
            HOME_URL.to_string()
        };

        // Extensions: load from every candidate directory, log and
        // skip the bad ones (never crash on a bad manifest). The
        // snapshots live on the app now (Round 7: the Settings
        // window's Extensions section shows them) — no server
        // owns browser state any more.
        let (host, loaded, failed) = load_extensions();

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
                menu_hovered: None,
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
                print_pending_label: String::new(),
                print_pending_title: String::new(),
                print_pending_url: String::new(),
                inspector_shot_rx: None,
                inspector_shot_pending: false,
                inspector_recapture: false,
                settings,
                extensions: loaded,
                failed_extensions: failed,
                settings_win: None,
                print_win: None,
                open_win: None,
                inspector_win: None,
                exit_win: None,
                host,
                injected_for: None,
            },
            Task::none(),
        )
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch(vec![
            iced::time::every(TICK).map(|_| Message::Tick),
            // Round 7: listen_with tags every event with its
            // window id. Window lifecycle events (opened / closed
            // / close requested) become their own messages; all
            // other ignored events become IcedEvent with the id,
            // so input forwarding can tell the main window from a
            // popup (same filter as the old `event::listen()` —
            // captured events stay with their widget).
            event::listen_with(|event, status, id| match event {
                iced::Event::Window(window::Event::Opened { .. }) => {
                    Some(Message::WindowOpened(id))
                },
                iced::Event::Window(window::Event::Closed) => {
                    Some(Message::WindowClosed(id))
                },
                iced::Event::Window(window::Event::CloseRequested) => {
                    Some(Message::CloseWindow(id))
                },
                _ => match status {
                    iced::event::Status::Ignored => {
                        Some(Message::IcedEvent(id, event))
                    },
                    iced::event::Status::Captured => None,
                },
            }),
        ])
    }

    fn on_window_resize(&mut self, id: window::Id, width: f32, height: f32) {
        // Round 7: only the MAIN window's resize resizes the
        // webview — a popup resizing must not touch it.
        if self.core.main_window_id() != Some(id) {
            return;
        }
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
                // Round 7: no server to poll any more — settings
                // changes happen in the native Settings window in
                // this process, and the file browser / inspector
                // never navigate the webview. Nothing to poll here.
                let mut tasks: Vec<Task<Message>> = Vec::new();
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
                // Open the Print window on them (or on the error),
                // with the printed page's identity frozen at
                // print-open time.
                if let Some(rx) = &self.print_shot_rx {
                    if let Ok(result) = rx.try_recv() {
                        self.print_shot_rx = None;
                        self.print_shot_pending = false;
                        let label = std::mem::take(&mut self.print_pending_label);
                        let title = std::mem::take(&mut self.print_pending_title);
                        let url = std::mem::take(&mut self.print_pending_url);
                        tasks.push(self.open_print_window(result, label, title, url));
                    }
                }
                // Round 5 inspector: the walker JSON arrives on
                // this channel once the script runs in the page.
                // Parse it and open the Inspector window on the
                // snapshot — or, on a re-capture, replace the open
                // window's snapshot in place. A failed capture
                // leaves the current page up and says so in the
                // status line.
                if let Some(rx) = &self.inspector_shot_rx {
                    if let Ok(result) = rx.try_recv() {
                        self.inspector_shot_rx = None;
                        self.inspector_shot_pending = false;
                        let recapture = std::mem::take(&mut self.inspector_recapture);
                        match result.and_then(|json| snapshot_from_json(&json)) {
                            Ok(snap) => {
                                if recapture && self.inspector_win.is_some() {
                                    self.replace_inspector_snapshot(snap);
                                } else {
                                    tasks.push(self.open_inspector_window(snap));
                                }
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
                    if let Some(main_id) = self.core.main_window_id() {
                        tasks.push(self.set_window_title(self.titlebar_text(), main_id));
                    }
                }
                Task::batch(tasks)
            },
            Message::WindowOpened(_id) => {
                // The window's title was set when it was opened;
                // nothing more to do when the runtime reports it.
                Task::none()
            },
            Message::WindowClosed(id) => {
                // The window is gone: drop its popup state, if it
                // had any. (The main window closing ends the app.)
                self.forget_window(id);
                Task::none()
            },
            Message::CloseWindow(id) => self.close_window_by_id(id),
            Message::Popup(msg) => self.update_popup(msg),
            Message::IcedEvent(id, event) => self.handle_iced_event(id, event),
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
                let trimmed = self.address.trim().to_string();
                // Round 7: there is no internal web server any
                // more, so a frost:// URL cannot load. Say so
                // plainly and leave the current page alone — never
                // a dead page, never a hang.
                if trimmed.to_ascii_lowercase().starts_with("frost://") {
                    self.status = "Frost no longer has internal pages — Settings, Print, Open file, and Inspector open in their own windows from the ☰ menu.".to_string();
                    return Task::none();
                }
                let url = resolve_address(&trimmed, &self.settings);
                match self.webview.load(&url) {
                    Ok(()) => {
                        // Round 4, F-008: mark the load in flight
                        // NOW (see load_url_in_main) so Stop stays
                        // live if the server never responds and
                        // LoadStarted never fires.
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
                Task::none()
            },
            Message::ToggleMenu => {
                // Round 8: the menu floats over the content, so
                // opening it does not resize the webview.
                if self.menu_open {
                    self.close_menu();
                } else {
                    self.menu_open = true;
                }
                Task::none()
            },
            Message::CloseMenu => {
                self.close_menu();
                Task::none()
            },
            Message::MenuHover(hovered) => {
                self.menu_hovered = hovered;
                Task::none()
            },
            Message::OpenSettings => {
                self.close_menu();
                self.open_settings_window()
            },
            Message::OpenPrint => {
                // F-003: capture the CURRENT page first — the
                // preview must be of the page the user was reading.
                // The Print window opens from the Tick once the PNG
                // bytes arrive (or the capture errors). A Print
                // window that is already open is just focused.
                self.close_menu();
                if let Some(id) = self.popup_id(windows::PopupKind::Print) {
                    return window::gain_focus(id);
                }
                if self.print_shot_pending {
                    return Task::none();
                }
                let (tx, rx) = std::sync::mpsc::channel();
                self.webview.request_screenshot_png(tx);
                self.print_shot_rx = Some(rx);
                self.print_shot_pending = true;
                self.status = "Capturing print preview…".to_string();
                // Freeze the printed page's identity here: the
                // Print window renders from these.
                self.print_pending_label = format!("Preview of {}", self.address);
                self.print_pending_title = self.title.clone().unwrap_or_default();
                self.print_pending_url = self.address.clone();
                Task::none()
            },
            Message::OpenInspector => {
                // Round 5: capture the current page's DOM, then
                // open the Inspector window on the snapshot (see
                // capture_inspector). An Inspector window that is
                // already open is just focused.
                self.close_menu();
                if let Some(id) = self.popup_id(windows::PopupKind::Inspector) {
                    window::gain_focus(id)
                } else {
                    self.capture_inspector();
                    Task::none()
                }
            },
            Message::OpenFile => {
                self.close_menu();
                let home = file_browser::home_dir();
                self.open_file_window(&home)
            },
            Message::Exit => {
                self.close_menu();
                if self.settings.confirm_exit {
                    self.open_exit_window()
                } else {
                    std::process::exit(0);
                }
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
        // ☰ button: the suggested (accent) style while the menu is
        // open is the pressed/active state; standard otherwise.
        let menu_button = if self.menu_open {
            button::suggested("☰").on_press(Message::ToggleMenu)
        } else {
            button::standard("☰").on_press(Message::ToggleMenu)
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
            menu_button.into(),
        ])
        .spacing(8)
        .padding(8)
        .height(Length::Fixed(TOOLBAR_HEIGHT));
        let toolbar = container(toolbar_row)
            .style(move |_| chrome_container_style(colors))
            .width(Length::Fill)
            .height(Length::Fixed(TOOLBAR_HEIGHT));

        // Round 8: the hamburger menu is a floating dropdown panel
        // (see menu_panel), not a row in this column. The column is
        // titlebar / toolbar / content / status; the panel floats
        // over the content via the Stack below.

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

        // Round 8: the main column (titlebar / toolbar / content /
        // status) is the Stack's base layer. When the menu is open,
        // two layers float over it: a transparent full-window
        // click-catcher (any press outside the panel closes the
        // menu, and — like Firefox — does not click through to the
        // page or toolbar underneath), then the dropdown panel
        // itself, anchored top-right under the ☰ button. The Stack
        // dispatches events top-first, so the panel's own buttons
        // still win where they overlap the catcher.
        let main_column = widget::column::with_children(vec![
            titlebar.into(),
            toolbar.into(),
            content,
            status.into(),
        ]);
        let mut stack = cosmic::iced::widget::Stack::new().push(main_column);
        if self.menu_open {
            let catcher = mouse_area(
                widget::Space::new()
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .on_press(Message::CloseMenu);
            stack = stack.push(catcher);
            stack = stack.push(self.menu_panel(colors));
        }

        container(stack.width(Length::Fill).height(Length::Fill))
            .style(move |_| chrome_container_style(colors))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Views for the popup windows (Round 7, libcosmic
    /// multi-window): the runtime calls this for every window
    /// that is not the main window. The main window still uses
    /// `view()`.
    fn view_window(&self, id: window::Id) -> Element<'_, Message> {
        if self.core.main_window_id() == Some(id) {
            return self.view();
        }
        self.view_popup(id)
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

/// A `[u8; 3]` theme color as an iced `Color`.
fn rgb(color: [u8; 3]) -> cosmic::iced::Color {
    cosmic::iced::Color::from_rgb8(color[0], color[1], color[2])
}

/// One dropdown entry: label left, shortcut (F12) right-aligned in
/// the theme's muted color. The whole row is the click target.
/// Round 8 note: this is a `mouse_area`, not a button — the raw
/// iced button's custom `.style()` closure cannot be used under
/// cosmic's Theme (its button class does not accept an arbitrary
/// style fn), and cosmic's own buttons are label-only, so a row
/// with a right-aligned shortcut cannot live inside one. The hover
/// highlight comes from `menu_hovered` (see `MenuHover`): the entry
/// under the pointer renders with the theme's hover fill.
fn menu_item<'a>(
    colors: ChromeColors,
    index: usize,
    hovered: bool,
    label: &'a str,
    shortcut: Option<&'a str>,
    message: Message,
) -> Element<'a, Message> {
    let mut row_items: Vec<Element<'_, Message>> =
        vec![text(label).size(14).into()];
    if let Some(shortcut) = shortcut {
        row_items.push(widget::Space::new().width(Length::Fill).into());
        row_items.push(
            container(text(shortcut).size(13))
                .style(move |_| menu_shortcut_style(colors))
                .into(),
        );
    }
    let row = widget::row::with_children(row_items)
        .align_y(iced::Alignment::Center)
        .spacing(8);
    let style = if hovered {
        menu_item_hover_style(colors)
    } else {
        menu_item_rest_style(colors)
    };
    mouse_area(
        container(row)
            .style(move |_| style)
            .width(Length::Fill)
            .padding([8, 10]),
    )
    .on_press(message)
    .on_enter(Message::MenuHover(Some(index)))
    .on_exit(Message::MenuHover(None))
    .into()
}

/// A dropdown entry at rest: transparent (the panel shows through),
/// rounded corners.
fn menu_item_rest_style(colors: ChromeColors) -> cosmic::iced::widget::container::Style {
    cosmic::iced::widget::container::Style {
        text_color: Some(rgb(colors.text)),
        border: cosmic::iced::Border {
            color: cosmic::iced::Color::TRANSPARENT,
            width: 0.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    }
}

/// A dropdown entry under the pointer: the theme's hover fill, same
/// rounded corners as at rest.
fn menu_item_hover_style(colors: ChromeColors) -> cosmic::iced::widget::container::Style {
    cosmic::iced::widget::container::Style {
        background: Some(cosmic::iced::Background::Color(rgb(colors.menu_hover))),
        text_color: Some(rgb(colors.text)),
        border: cosmic::iced::Border {
            color: cosmic::iced::Color::TRANSPARENT,
            width: 0.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    }
}

/// Style for the dropdown panel itself: the theme background, a
/// visible 1 px border, rounded corners, and a drop shadow — so it
/// reads as a floating panel over the page, not a toolbar row.
fn menu_panel_style(colors: ChromeColors) -> cosmic::iced::widget::container::Style {
    cosmic::iced::widget::container::Style {
        background: Some(cosmic::iced::Background::Color(rgb(colors.background))),
        text_color: Some(rgb(colors.text)),
        border: cosmic::iced::Border {
            color: rgb(colors.menu_border),
            width: 1.0,
            radius: 8.0.into(),
        },
        shadow: cosmic::iced::Shadow {
            color: cosmic::iced::Color::from_rgba(0.0, 0.0, 0.0, 0.45),
            offset: cosmic::iced::Vector::new(0.0, 4.0),
            blur_radius: 16.0,
        },
        ..Default::default()
    }
}

/// A 1 px separator line between menu groups, in the theme's border
/// color on a transparent background.
fn menu_separator_style(colors: ChromeColors) -> cosmic::iced::widget::container::Style {
    cosmic::iced::widget::container::Style {
        background: Some(cosmic::iced::Background::Color(rgb(colors.menu_border))),
        ..Default::default()
    }
}

/// The shortcut label (F12) in the theme's muted color. The label
/// sits in its own container because a bare text widget takes the
/// button's text color; the container's text color wins.
fn menu_shortcut_style(colors: ChromeColors) -> cosmic::iced::widget::container::Style {
    cosmic::iced::widget::container::Style {
        text_color: Some(rgb(colors.muted)),
        ..Default::default()
    }
}

/// Address-bar resolution: a URL with a loadable scheme is used
/// as-is; a bare hostname (no spaces, has a dot) gets `https://`;
/// anything else is a search query with the default search engine.
/// An empty bar loads the homepage. Round 7: `frost` is NOT a
/// loadable scheme any more — the address bar intercepts
/// `frost://` before resolving (see Message::AddressSubmitted).
fn resolve_address(input: &str, settings: &FrostSettings) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return settings.homepage.clone();
    }
    if let Ok(url) = Url::parse(trimmed) {
        if matches!(url.scheme(), "http" | "https" | "data" | "file") {
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
/// the (directory, error) failures, for the Settings window's
/// Extensions section. A bad manifest is logged and skipped — it
/// never stops the browser or the other extensions.
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
    use crate::settings::{SearchEngine, ThemeChoice};

    #[test]
    fn menu_panel_anchors_right_at_1280_and_800() {
        // 300 px panel, 8 px from the right edge.
        assert_eq!(menu_panel_geometry(1280.0), (972.0, 300.0));
        assert_eq!(menu_panel_geometry(800.0), (492.0, 300.0));
    }

    #[test]
    fn menu_panel_never_overflows_narrow_windows() {
        for width in [320.0, 250.0, 100.0, 20.0] {
            let (x, w) = menu_panel_geometry(width);
            assert!(x >= 0.0, "x {x} at window width {width}");
            assert!(w >= 1.0, "width {w} at window width {width}");
            assert!(
                x + w <= width,
                "panel ({x} + {w}) runs off-screen at window width {width}"
            );
        }
    }

    #[test]
    fn menu_items_are_five_with_f12_only_on_inspector() {
        assert_eq!(MENU_ITEMS.len(), 5);
        let labels: Vec<&str> = MENU_ITEMS.iter().map(|(label, _, _)| *label).collect();
        assert_eq!(
            labels,
            ["Settings", "Print page…", "Open file…", "Inspector", "Exit"]
        );
        for (label, shortcut, _) in MENU_ITEMS {
            if label == "Inspector" {
                assert_eq!(shortcut, Some("F12"));
            } else {
                assert_eq!(shortcut, None, "{label} must not invent a shortcut");
            }
        }
    }

    #[test]
    fn menu_panel_style_follows_the_theme() {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            let colors = theme.chrome_colors();
            let style = menu_panel_style(colors);
            assert_eq!(style.border.color, rgb(colors.menu_border));
            assert_ne!(colors.menu_hover, colors.background);
            assert_ne!(colors.menu_border, colors.background);
        }
    }

    #[test]
    fn resolve_address_keeps_urls_and_searches_the_rest() {
        let settings = FrostSettings::default();
        assert_eq!(
            resolve_address("https://example.org", &settings),
            "https://example.org/"
        );
        assert_eq!(
            resolve_address("frost://settings", &settings),
            "https://duckduckgo.com/?q=frost%3A%2F%2Fsettings"
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