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
use frozen::{
    Engine, EngineEvent, FrostCode, FrostInputEvent, FrostKeyState,
    FrostMouseButtonAction, FrostWebView, WebViewId, MAX_SYNC_UPLOAD_BYTES,
};
use frost_extensions::{ExtensionHost, ExtensionManifest, RunAt, StubHost};
use frost_inspector::{snapshot_from_json, DOM_WALK_SCRIPT};
use url::Url;

use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::archive;
use crate::file_browser;
use crate::keymap::{map_button, map_code, map_key_with_text, map_location, map_modifiers};
use crate::settings::{is_loadable_homepage, ChromeColors, Settings as FrostSettings};
use windows::{
    ArchivesWindow, InspectorWindow, OpenWindow, PopupMessage, PrintWindow, SettingsWindow,
};

/// Chrome geometry, fixed by construction (see the module docs).
/// Round 10: the window manager draws the titlebar by default
/// (`client_decorations: false` — see `use_client_decorations`).
/// Frost's OWN titlebar above the toolbar (40 px) is now only the
/// GNOME fallback: GNOME/Mutter refuses server-side decorations,
/// so there the WM bar would mean no titlebar at all. The
/// built-in libcosmic headerbar stays hidden in both modes,
/// because on this setup it renders the title but no
/// window-control buttons at all (its min/max/close are
/// icon-based and never appear — verified on Joel's box and in
/// the sandbox). Frost's fallback titlebar uses plain text
/// glyphs, so the buttons are always visible.
const TITLEBAR_HEIGHT: f32 = 40.0;
const TOOLBAR_HEIGHT: f32 = 48.0;
/// Round 11: toolbar icon buttons are round, not pill-shaped —
/// square 32 px bounds, the toolbar's inner height (48 px row
/// minus 2×8 px row padding). The theme's button corner radius
/// (radius_xl = 160) is already far past half the side, so a
/// square button renders as a circle; the pill shape came only
/// from the wider-than-tall bounds (glyph + 2×space_s of
/// horizontal padding on a 32 px tall button). Same visual size
/// as before — just square, never oval.
const TOOLBAR_ICON_SIZE: f32 = 32.0;
/// Round 12: toolbar glyph size — the Round 11 buttons are round
/// but the glyphs render small (~10 px) and left-of-center.
/// libcosmic's TextButton builds its label in a left-aligned row
/// at the default 14 px font with no alignment control, so the
/// glyph sits at the button's left edge no matter the bounds.
/// A 20 px glyph in a 32 px circle reads like a real toolbar
/// icon (Firefox's are ~16 px in 32–36 px buttons) and still
/// leaves room around it — the widest glyph, "↻", is ~20 px
/// wide at this size.
const TOOLBAR_GLYPH_SIZE: f32 = 20.0;
/// Round 12: the ❮/❯ chevrons' ink sits ~2 px low in the em
/// next to ↻/☰/✕ at the same size (measured from pixels, and
/// from the font metrics — their ink center is ~2 px below the
/// reload/menu glyphs' at 20 px), so the chevron buttons nudge
/// their glyphs up by this much. The nudge is applied as bottom
/// padding on the glyph container (see `round_icon_button`):
/// padding shrinks the centered area from the bottom, so the
/// glyph sits higher by half the padding.
const TOOLBAR_CHEVRON_NUDGE_UP: f32 = 2.0;
const STATUS_HEIGHT: f32 = 28.0;
/// Split view (Round 14): the active pane's border, in px — the
/// same width on the inactive pane (transparent) so switching the
/// active pane never shifts layout. The pane's image column sits
/// inside this border + the same padding, so the webview viewport
/// is the pane rect minus 2×PANE_BORDER on each side (see
/// `pane_viewport_size`).
const PANE_BORDER: f32 = 2.0;
/// Index of the "Split view" entry in MENU_ITEMS (Round 14):
/// the menu label is dynamic ("Split view" / "Close split view"),
/// so the panel reads this index instead of the table's label.
const SPLIT_MENU_INDEX: usize = 5;
/// Hamburger menu dropdown panel (Round 8): a compact Firefox-style
/// panel anchored top-right under the ☰ button, floating over the
/// page — NOT the full-width bar of Rounds 2–7, which pushed the
/// content down and spanned the whole window.
const MENU_PANEL_WIDTH: f32 = 300.0;
/// Right-edge margin of the dropdown panel.
const MENU_PANEL_MARGIN: f32 = 8.0;
/// Gap between the toolbar and the top of the dropdown panel.
const MENU_PANEL_GAP: f32 = 4.0;
/// Command palette (Round 9): a centered VS Code-style overlay
/// panel. Fixed width, shrinking on narrow windows; the top gap
/// puts it below the toolbar, floating over the page like the
/// menu — it consumes no layout space.
const PALETTE_PANEL_WIDTH: f32 = 560.0;
const PALETTE_PANEL_MARGIN: f32 = 16.0;
const PALETTE_PANEL_TOP: f32 = 100.0;
/// Widget ids for programmatic focus (Round 9): the palette's
/// query input (focused on open) and the address bar (focused by
/// the palette's "Focus address bar" command, with select-all).
/// Plain functions, not consts: this fork's `Id::new` is not a
/// const fn. The same string always yields the same id.
fn palette_input_id() -> cosmic::iced::widget::Id {
    cosmic::iced::widget::Id::new("palette-input")
}
fn address_input_id() -> cosmic::iced::widget::Id {
    cosmic::iced::widget::Id::new("address-bar")
}
const INITIAL_WINDOW: (f32, f32) = (1280.0, 800.0);
const HOME_URL: &str = "https://example.org";
/// Engine pump rate. The tick spins Servo, drains its events, and
/// re-renders only while loading or when a frame was flagged ready.
const TICK: Duration = Duration::from_millis(16);

/// The hamburger menu's entries, in order: (label, shortcut, message).
/// Firefox shows one shortcut (F12 for Inspector); the other entries
/// have no shortcut — do not invent any. Round 9 adds "Command
/// palette…" (Ctrl+K, the palette's own shortcut) after Inspector,
/// Round 14 adds "Split view" after it, Round 15 adds "Freeze this
/// page…" and "Archived pages…" after that, so the groups stay
/// Settings | Print page… / Open file… / Inspector /
/// Command palette… | Split view / Freeze this page… /
/// Archived pages… / Exit. The view builds the panel
/// from this table, so the spec and the rendering cannot drift.
/// Separators fall after "Settings" and after "Command palette…".
/// The "Split view" label is dynamic (see SPLIT_MENU_INDEX):
/// while split it reads "Close split view".
const MENU_ITEMS: [(&str, Option<&str>, Message); 9] = [
    ("Settings", None, Message::OpenSettings),
    ("Print page…", None, Message::OpenPrint),
    ("Open file…", None, Message::OpenFile),
    ("Inspector", Some("F12"), Message::OpenInspector),
    ("Command palette…", Some("Ctrl+K"), Message::TogglePalette),
    ("Split view", None, Message::ToggleSplit),
    ("Freeze this page…", None, Message::FreezePage),
    ("Archived pages…", None, Message::OpenArchives),
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

/// Split view geometry (Round 14): the content rect's two halves,
/// side by side. The left pane takes floor(width / 2), the right
/// pane the rest, so an odd width loses no pixel and the halves
/// tile exactly with no gap and no overlap.
fn split_pane_rects(content: Rectangle) -> (Rectangle, Rectangle) {
    let left_w = (content.width / 2.0).floor();
    (
        Rectangle {
            x: content.x,
            y: content.y,
            width: left_w,
            height: content.height,
        },
        Rectangle {
            x: content.x + left_w,
            y: content.y,
            width: content.width - left_w,
            height: content.height,
        },
    )
}

/// Which pane a window point falls in (Round 14): 0 in single
/// view, or the left (0) / right (1) half in split view. The
/// caller checks the point is inside the content area first.
fn pane_at_point(content: Rectangle, split: bool, point: Point) -> usize {
    if !split {
        return 0;
    }
    let (left, _) = split_pane_rects(content);
    if point.x < left.x + left.width {
        0
    } else {
        1
    }
}

/// A pane's webview viewport size (Round 14): in split view the
/// image column sits inside the pane's 2 px border + 2 px padding,
/// so the viewport is the pane rect minus 2×PANE_BORDER on each
/// side. In single view the webview IS the content area, exactly
/// as before. Never zero.
fn pane_viewport_size(rect: Rectangle, split: bool) -> (u32, u32) {
    if split {
        let w = (rect.width - 2.0 * PANE_BORDER).max(1.0) as u32;
        let h = (rect.height - 2.0 * PANE_BORDER).max(1.0) as u32;
        (w, h)
    } else {
        (
            rect.width.max(1.0) as u32,
            rect.height.max(1.0) as u32,
        )
    }
}

/// (x, width) of the command-palette panel for a window of the
/// given width (Round 9): PALETTE_PANEL_WIDTH wide, centered, with
/// PALETTE_PANEL_MARGIN kept on both sides. A window narrower than
/// the panel shrinks the panel instead of letting it run
/// off-screen; x never goes negative.
fn palette_panel_geometry(window_width: f32) -> (f32, f32) {
    let width = PALETTE_PANEL_WIDTH
        .min((window_width - 2.0 * PALETTE_PANEL_MARGIN).max(1.0));
    let x = ((window_width - width) / 2.0).max(0.0);
    (x, width)
}

/// Round 10: which titlebar Frost gets. GNOME/Mutter refuses
/// server-side decorations, so on GNOME the WM bar would mean no
/// titlebar at all — Frost draws its own (`true`). Every other
/// desktop (KDE, COSMIC, Sway, …) and an unset/unknown value get
/// the window manager's bar (`false`). `desktop` is the resolved
/// desktop-environment string (see `detected_desktop`); the
/// check is a case-insensitive substring match, so
/// colon-separated values like "ubuntu:GNOME" match too.
fn use_client_decorations(desktop: &str) -> bool {
    desktop.to_ascii_lowercase().contains("gnome")
}

/// Round 10: resolve the desktop environment for the titlebar
/// rule — `XDG_CURRENT_DESKTOP` first, `XDG_SESSION_DESKTOP`
/// as the fallback, empty when neither is set (which means the
/// WM bar, per `use_client_decorations`).
fn detected_desktop() -> String {
    std::env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("XDG_SESSION_DESKTOP")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or_default()
}

/// Round 10: height of Frost's own titlebar row for a mode —
/// 40 px when Frost draws it (GNOME fallback), 0 when the
/// window manager draws the titlebar. Pure, so the mode logic
/// is unit-testable without a running app.
fn titlebar_height_for(client_decorations: bool) -> f32 {
    if client_decorations {
        TITLEBAR_HEIGHT
    } else {
        0.0
    }
}

/// Round 12: a toolbar icon button with its glyph big and
/// centered. Built on libcosmic's `button::custom` — the same
/// Button widget and theme classes as `button::standard`, so
/// hover/pressed/disabled look identical — with a 32×32
/// container that centers the glyph both ways. TextButton
/// cannot do this: its label lives in a left-aligned row at
/// 14 px (see `src/widget/button/text.rs` in the vendored
/// libcosmic source, pinned rev ecc5fe4 — the row is only
/// `.align_y(Center)`, and the Builder exposes no horizontal
/// alignment), which is exactly why the Round 11 glyphs sat
/// small at the left edge of their circles. The caller sets
/// the class and on_press, as before. `nudge_up` shifts the
/// glyph up by that many px (0.0 for most glyphs): bottom
/// padding shrinks the container's centered area from the
/// bottom, so the glyph sits higher by half the padding.
/// Needed for glyphs whose ink sits low in the em — the ❮/❯
/// chevrons sit ~2 px low next to ↻/☰/✕ at the same size.
fn round_icon_button<'a>(
    label: &'static str,
    class: button::ButtonClass,
    nudge_up: f32,
) -> button::Button<'a, Message> {
    let bottom_pad = (2.0 * nudge_up).max(0.0) as u16;
    let glyph = container(text(label).size(TOOLBAR_GLYPH_SIZE))
        .width(Length::Fixed(TOOLBAR_ICON_SIZE))
        .height(Length::Fixed(TOOLBAR_ICON_SIZE))
        .align_x(iced::Alignment::Center)
        .align_y(iced::Alignment::Center)
        .padding(iced::Padding::from([0u16, 0u16, bottom_pad, 0u16]));
    button::custom(glyph)
        .width(Length::Fixed(TOOLBAR_ICON_SIZE))
        .height(Length::Fixed(TOOLBAR_ICON_SIZE))
        .padding(iced::Padding::from([0u16, 0u16]))
        .class(class)
}

/// Round 11: the merged reload/stop button — which glyph and
/// message it shows for a given loading state. Pure, so the
/// mapping is unit-testable; view() just renders it. The
/// button is always live: reload when idle, stop while a load
/// is in flight. It flips back to reload in both end states —
/// load finished and load stopped — because both clear
/// `self.loading` (see EngineEvent::LoadFinished /
/// LoadStopped).
fn reload_stop_button(loading: bool) -> (&'static str, Message) {
    if loading {
        ("✕", Message::Stop)
    } else {
        ("⟳", Message::Reload)
    }
}

/// Scored fuzzy match (Round 34): nucleo-matcher's fzf-style
/// matcher scores how well `query` matches `label` — prefix and
/// contiguous matches score higher than scattered ones — so the
/// palette can rank best match first. `None` means no match. An
/// empty query matches everything with score 0, so the palette
/// opens on the full list (same as the old hand-written
/// subsequence matcher). `Config::DEFAULT` is case-insensitive.
///
/// The query is lowercased first — a nucleo-matcher 0.3.1 quirk,
/// verified firsthand: with `ignore_case: true` its ASCII
/// prefilter only folds case for LOWERCASE needle chars (an
/// uppercase needle char must match an uppercase haystack char
/// exactly), while its scorer compares the case-folded haystack
/// against the RAW needle. A mixed-case needle that passes the
/// prefilter therefore PANICS the scorer ("should have been
/// caught by prefilter", `fuzzy_optimal.rs:37`). Lowercasing the
/// query gives true case-insensitive matching (the Round 9
/// semantics) and closes the panic hole; the haystack is folded
/// by the matcher itself.
fn fuzzy_score(query: &str, label: &str) -> Option<u16> {
    let query = query.to_lowercase();
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut haystack_buf = Vec::new();
    let mut needle_buf = Vec::new();
    matcher.fuzzy_match(
        Utf32Str::new(label, &mut haystack_buf),
        Utf32Str::new(&query, &mut needle_buf),
    )
}

/// Subsequence match, case-insensitive (Round 9, now backed by
/// nucleo-matcher in Round 34): every character of the query appears
/// in the label, in order. An empty query matches everything.
/// Kept as the boolean match API — the Round 9 subsequence test
/// exercises it; the palette itself ranks via `fuzzy_score`.
#[allow(dead_code)]
fn fuzzy_match(query: &str, label: &str) -> bool {
    fuzzy_score(query, label).is_some()
}

/// Rank commands for a palette query (Round 34): best fuzzy score
/// first, table order on ties (the sort is stable); an empty query
/// returns the table unchanged. Pure, so the ranking is
/// unit-testable; `palette_filtered` just feeds it the live table.
fn rank_palette_commands(
    query: &str,
    commands: Vec<(&'static str, Message)>,
) -> Vec<(&'static str, Message)> {
    if query.is_empty() {
        return commands;
    }
    let mut scored: Vec<(u16, usize, (&'static str, Message))> = commands
        .into_iter()
        .enumerate()
        .filter_map(|(i, (label, msg))| fuzzy_score(query, label).map(|score| (score, i, (label, msg))))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, cmd)| cmd).collect()
}

/// The palette's commands (Round 9): only actions that can act
/// right now. "Go back" / "Go forward" appear only when there is
/// history to move through, "Stop loading" only while a load is in
/// flight, "Freeze this page" only when a page is loaded (Round 15)
/// — so no entry ever looks real while doing nothing. The
/// rest are always available: every one of them is wired to an
/// existing working message (no dead entries by construction).
/// Round 14: the split command's label says what it WILL do —
/// "Split view" in single view, "Close split view" while split.
fn palette_commands_for(
    can_back: bool,
    can_forward: bool,
    loading: bool,
    split: bool,
    can_freeze: bool,
) -> Vec<(&'static str, Message)> {
    let mut commands = Vec::new();
    if can_back {
        commands.push(("Go back", Message::Back));
    }
    if can_forward {
        commands.push(("Go forward", Message::Forward));
    }
    commands.push(("Reload", Message::Reload));
    if loading {
        commands.push(("Stop loading", Message::Stop));
    }
    commands.push(("Open Settings", Message::OpenSettings));
    commands.push(("Print page…", Message::OpenPrint));
    commands.push(("Open file…", Message::OpenFile));
    commands.push(("Open Inspector", Message::OpenInspector));
    if split {
        commands.push(("Close split view", Message::ToggleSplit));
    } else {
        commands.push(("Split view", Message::ToggleSplit));
    }
    if can_freeze {
        commands.push(("Freeze this page", Message::FreezePage));
    }
    commands.push(("Archived pages…", Message::OpenArchives));
    commands.push(("Focus address bar", Message::FocusAddress));
    commands.push(("Quit Frost", Message::Exit));
    commands
}

/// Move the palette highlight `delta` steps through `len` rows,
/// wrapping around both ends (Round 9). No rows → 0.
fn move_selection(selected: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    let len = len as isize;
    (selected as isize + delta).rem_euclid(len) as usize
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
    /// A keyboard event a widget CAPTURED (Round 9): only the
    /// palette input can capture keys while the palette is open,
    /// and it swallows Escape — so Escape reaches the app only
    /// through here. The handler ignores captured keys unless the
    /// palette is open, so the address bar's captured keys still
    /// never reach the page.
    IcedEventCaptured(window::Id, iced::Event),
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
    /// Toggle the command palette (Ctrl+K, or the menu entry).
    /// Opening it closes the menu; closing it leaves the page
    /// as it was (Round 9).
    TogglePalette,
    /// Split the window into two panes side by side, or close
    /// the split and keep the active pane (Round 14). In the
    /// menu and the palette the label says what it will do.
    ToggleSplit,
    /// Freeze the ACTIVE pane's page as a Deep Freeze archive
    /// (Round 15): capture the live DOM and save it as a
    /// self-contained HTML file in the archives dir.
    FreezePage,
    /// Open the Archived pages window (Round 15): the list of
    /// frozen pages, with Open and Delete per entry.
    OpenArchives,
    /// Close the command palette without running anything
    /// (Escape, or a press outside the panel).
    ClosePalette,
    /// The palette's query input changed: re-filter, highlight
    /// back at the top (Round 9).
    PaletteInputChanged(String),
    /// The pointer entered a palette row: it becomes the
    /// highlighted one (Round 9).
    PaletteSelect(usize),
    /// Run the i-th command of the CURRENT filtered list (Round 9).
    PaletteRun(usize),
    /// Run the currently highlighted command (Round 9 — the
    /// palette input's on_submit).
    PaletteRunSelected,
    /// Focus the address bar, with its URL selected (Round 9 —
    /// the palette's "Focus address bar" command).
    FocusAddress,
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

/// One split-view pane (Round 14): a webview plus everything the
/// app used to keep once, globally, for the single webview. The
/// address bar itself stays on [`Frost`] — there is one bar,
/// showing the ACTIVE pane's URL — and a pane's URL is whatever
/// its webview reports. Status, Snowplow count, and history state
/// are all per pane; the view reads the active pane's.
struct Pane {
    webview: FrostWebView,
    /// Latest uploaded frame, as horizontal strips, if one has
    /// painted yet. Each entry is an iced image handle plus the
    /// strip's height in pixels; the strips tile the pane's
    /// content area top to bottom (see `Frame::split_strips`).
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
    /// The page title, for the status line once loaded.
    title: Option<String>,
    /// One-line status for this pane.
    status: String,
    /// Round 13 (Snowplow): tracker/ad subresources blocked on the
    /// current page. Reset on each navigation; shown in the status
    /// line when greater than zero.
    snowplow_blocked: u64,
    can_back: bool,
    can_forward: bool,
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
    /// URL content scripts were last injected for; a new navigation
    /// clears it so the new page gets its scripts.
    injected_for: Option<String>,
    /// URL `DocumentStart` content scripts were last injected for
    /// (Round 28: HeadParsed pass). A new navigation clears it.
    injected_start_for: Option<String>,
}

impl Pane {
    /// A pane for a webview that is already loading, with its
    /// initial status line.
    fn new(webview: FrostWebView, status: String) -> Self {
        Self {
            webview,
            frame_strips: Vec::new(),
            frame_dirty: false,
            loading: true,
            title: None,
            status,
            snowplow_blocked: 0,
            can_back: false,
            can_forward: false,
            awaiting_load: true,
            loading_since: Some(std::time::Instant::now()),
            injected_for: None,
            injected_start_for: None,
        }
    }

    /// Mark a load in flight NOW (Round 4, F-008): Stop stays live
    /// even if the load never reports a start.
    fn mark_loading(&mut self, status: String) {
        self.loading = true;
        self.awaiting_load = true;
        self.loading_since = Some(std::time::Instant::now());
        self.status = status;
    }

    /// A load ended (finished, stopped, or crashed): clear the
    /// in-flight flags and show `status`.
    fn mark_settled(&mut self, status: String) {
        self.loading = false;
        self.awaiting_load = false;
        self.loading_since = None;
        self.status = status;
    }
}

pub struct Frost {
    core: Core,
    engine: Engine,
    /// The panes (Round 14): panes[0] always exists; panes[1]
    /// exists exactly while split. `active_pane` indexes the pane
    /// the toolbar, address bar, status line, and keyboard act on.
    panes: Vec<Pane>,
    /// Index of the active pane into `panes`.
    active_pane: usize,
    /// Whether the window shows two panes side by side.
    split: bool,
    /// What the address bar shows (the ACTIVE pane's URL, unless
    /// the user is mid-edit — see `address_editing`).
    address: String,
    /// The user is mid-edit in the address bar: engine URL updates
    /// must not clobber what they are typing.
    address_editing: bool,
    /// Whether the hamburger menu is open.
    menu_open: bool,
    /// Index of the menu entry under the pointer, for the hover
    /// highlight (Round 8). None when the pointer is not over an
    /// entry; cleared whenever the menu closes.
    menu_hovered: Option<usize>,
    /// Whether the command palette is open (Round 9).
    palette_open: bool,
    /// The palette's query text (Round 9): the filtered list is
    /// derived from it on every render, so it is the only query
    /// state the app keeps.
    palette_query: String,
    /// Index of the highlighted command in the CURRENT filtered
    /// list (Round 9). Reset to 0 whenever the query changes;
    /// clamped by the filtered length at use.
    palette_selected: usize,
    window_size: Size,
    /// Round 10: whether Frost draws its own titlebar (GNOME
    /// fallback) or the window manager does. Chosen once at
    /// startup from the desktop environment (see
    /// `use_client_decorations`); the view, content_rect, and
    /// menu geometry all read it through `titlebar_height()`.
    client_decorations: bool,
    /// Last cursor position, in absolute window coordinates.
    last_cursor: Option<Point>,
    /// Keyboard events go to the page only while this is set.
    content_focused: bool,
    /// The titlebar text changed; the OS window title needs the
    /// same update (via set_window_title, issued from the Tick).
    title_dirty: bool,
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
    /// A Deep Freeze capture is in flight (Round 15): the channel
    /// the frozen outerHTML arrives on, polled by the Tick. Set
    /// by `freeze_page`; cleared when the HTML arrives (or the
    /// capture errors) and the archive is written.
    freeze_rx: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    /// A freeze capture is in flight; the status line says so
    /// until the archive is written.
    freeze_pending: bool,
    /// The frozen page's identity, fixed at freeze time (URL,
    /// title) — the archive is written from these when the
    /// capture lands, so a navigation in between cannot
    /// mislabel it.
    freeze_pending_url: String,
    freeze_pending_title: String,
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
    /// Round 15: the Archived pages window, if open.
    archives_win: Option<ArchivesWindow>,
    exit_win: Option<window::Id>,
    /// Content-script host (StubHost: in-memory, real disk loading
    /// + real injection through the engine — see
    /// `inject_content_scripts`).
    host: StubHost,
}

impl Frost {
    /// Height of Frost's own titlebar row in the current mode
    /// (Round 10): 40 px in the GNOME fallback, 0 when the
    /// window manager draws the titlebar.
    fn titlebar_height(&self) -> f32 {
        titlebar_height_for(self.client_decorations)
    }

    /// The content area's rectangle in window coordinates. Round 8:
    /// the hamburger menu is a floating panel over the content, so
    /// it no longer consumes layout space (Rounds 2–7 subtracted
    /// MENU_HEIGHT here).
    fn content_rect(&self) -> Rectangle {
        Rectangle {
            x: 0.0,
            y: self.titlebar_height() + TOOLBAR_HEIGHT,
            width: self.window_size.width,
            height: (self.window_size.height
                - self.titlebar_height()
                - TOOLBAR_HEIGHT
                - STATUS_HEIGHT)
                .max(1.0),
        }
    }

    /// The webview viewport size for pane `index` (Round 14).
    fn pane_size(&self, index: usize) -> (u32, u32) {
        pane_viewport_size(self.pane_rect(index), self.split)
    }

    /// Resize every webview to its pane, if it changed.
    fn resize_panes(&mut self) {
        for i in 0..self.panes.len() {
            let (w, h) = self.pane_size(i);
            if self.panes[i].webview.size() != (w, h) {
                self.panes[i].webview.resize(w, h);
                self.panes[i].frame_dirty = true;
            }
        }
    }

    /// The panes' rectangles in window coordinates (Round 14):
    /// the whole content area alone, or its two halves side by
    /// side. The second element is None in single view.
    fn pane_rects(&self) -> (Rectangle, Option<Rectangle>) {
        let rect = self.content_rect();
        if self.split {
            let (left, right) = split_pane_rects(rect);
            (left, Some(right))
        } else {
            (rect, None)
        }
    }

    /// The rectangle of pane `index` (0 = left / only, 1 = right).
    fn pane_rect(&self, index: usize) -> Rectangle {
        let (left, right) = self.pane_rects();
        if index == 1 {
            right.unwrap_or(left)
        } else {
            left
        }
    }

    /// The active pane.
    fn active_pane(&self) -> &Pane {
        &self.panes[self.active_pane]
    }

    /// The active pane, mutably.
    fn active_pane_mut(&mut self) -> &mut Pane {
        &mut self.panes[self.active_pane]
    }

    /// Which pane a webview id belongs to (Round 14): the
    /// delegate tags every engine event with its webview's id,
    /// so events route to the pane that produced them. None
    /// for an id no pane owns (a closed pane's straggler).
    fn pane_index_for(&self, id: WebViewId) -> Option<usize> {
        self.panes.iter().position(|p| p.webview.id() == id)
    }

    /// Make pane `index` the active one (Round 14): the toolbar,
    /// address bar, status line, and keyboard follow it. The bar
    /// shows the new pane's URL unless the user is mid-edit.
    fn set_active_pane(&mut self, index: usize) {
        if index >= self.panes.len() || index == self.active_pane {
            return;
        }
        self.active_pane = index;
        if !self.address_editing {
            self.address = self.panes[index]
                .webview
                .url()
                .map(|u| u.to_string())
                .unwrap_or_default();
        }
        self.title_dirty = true;
    }

    /// Split the window into two panes side by side, or close the
    /// split and keep the active pane (Round 14). The second pane
    /// starts on the active pane's page — the most useful default:
    /// a comparison, a link opened beside the original, or a
    /// second view of the same page. The surviving pane on close
    /// is the ACTIVE one; dropping the other pane's webview
    /// closes it in the engine (`Drop for WebViewInner` sends
    /// CloseWebView), so no pipeline is left running.
    fn toggle_split(&mut self) -> Task<Message> {
        if !self.split {
            let url = self
                .active_pane()
                .webview
                .url()
                .map(|u| u.to_string())
                .unwrap_or_else(|| self.settings.homepage.clone());
            let (_, right) = split_pane_rects(self.content_rect());
            let (w, h) = pane_viewport_size(right, true);
            match self.engine.spawn_webview(&url, w, h) {
                Ok(webview) => {
                    webview.set_theme(self.settings.theme.is_dark());
                    self.panes
                        .push(Pane::new(webview, format!("Loading {url} …")));
                    self.split = true;
                    self.resize_panes();
                },
                Err(e) => {
                    self.active_pane_mut().status =
                        format!("Split view failed: {e}");
                },
            }
        } else {
            // Keep the active pane; drop the other (its webview's
            // Drop closes it in the engine).
            let other = if self.active_pane == 0 { 1 } else { 0 };
            self.panes.remove(other);
            self.active_pane = 0;
            self.split = false;
            if !self.address_editing {
                self.address = self.panes[0]
                    .webview
                    .url()
                    .map(|u| u.to_string())
                    .unwrap_or_default();
            }
            self.title_dirty = true;
            self.resize_panes();
        }
        Task::none()
    }

    /// Close the hamburger dropdown (and forget which entry was
    /// hovered). Every close path goes through here.
    fn close_menu(&mut self) {
        self.menu_open = false;
        self.menu_hovered = None;
    }

    /// Close the command palette (and forget its query and
    /// highlight). Every close path goes through here (Round 9).
    fn close_palette(&mut self) {
        self.palette_open = false;
        self.palette_query.clear();
        self.palette_selected = 0;
    }

    /// The palette's command list right now (Round 9): the
    /// available commands, filtered by the current query and ranked
    /// best match first (Round 34). The filtered list is derived on
    /// demand, so the view and the run path can never disagree
    /// about it.
    fn palette_filtered(&self) -> Vec<(&'static str, Message)> {
        rank_palette_commands(
            &self.palette_query,
            palette_commands_for(
                self.active_pane().can_back,
                self.active_pane().can_forward,
                self.active_pane().loading,
                self.split,
                self.active_pane().webview.url().is_some(),
            ),
        )
    }

    /// The command-palette panel (Round 9): a centered overlay
    /// panel, same pattern as the Round 8 menu (a `Stack` layer
    /// floating over the content — the caller adds the dimmed
    /// click-catcher under it). A text input on top, the filtered
    /// command list below; the highlighted row uses the same hover
    /// fill as a menu entry. The panel reuses the menu's panel
    /// style, so it follows the Dark/Light theme the same way.
    fn palette_panel(&self, colors: ChromeColors) -> Element<'_, Message> {
        let (x, width) = palette_panel_geometry(self.window_size.width);
        let filtered = self.palette_filtered();
        let mut children: Vec<Element<'_, Message>> = Vec::new();
        children.push(
            text_input("Type a command", &self.palette_query)
                .id(palette_input_id())
                .on_input(Message::PaletteInputChanged)
                .on_submit(|_| Message::PaletteRunSelected)
                .width(Length::Fill)
                .into(),
        );
        if filtered.is_empty() {
            children.push(
                container(text("No matching commands").size(14))
                    .style(move |_| menu_shortcut_style(colors))
                    .padding([8, 10])
                    .into(),
            );
        } else {
            for (i, (label, _)) in filtered.iter().enumerate() {
                children.push(palette_row(
                    colors,
                    i,
                    self.palette_selected == i,
                    label,
                ));
            }
        }
        let panel = container(widget::column::with_children(children).spacing(4))
            .style(move |_| menu_panel_style(colors))
            .padding(8)
            .width(Length::Fixed(width));
        // The wrapper fills the window; left padding puts the
        // panel's left edge at x (centered by the geometry), top
        // padding drops it below the toolbar.
        container(panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(iced::Padding::from([
                PALETTE_PANEL_TOP,
                0.0,
                0.0,
                x,
            ]))
            .into()
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
            // Round 14: the Split view entry's label says what it
            // WILL do — "Close split view" while split.
            let label: &str = if i == SPLIT_MENU_INDEX && self.split {
                "Close split view"
            } else {
                label
            };
            // Separators between groups: after Settings, after
            // "Command palette…".
            if i == 1 || i == 5 {
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
                self.titlebar_height() + TOOLBAR_HEIGHT + MENU_PANEL_GAP,
                0.0,
                0.0,
                x,
            ]))
            .into()
    }

    /// Translate an absolute window point into (pane, page)
    /// coordinates (Round 14), or `None` outside the content
    /// area. In split view the point also picks the pane it
    /// lands in; the page coordinates are relative to that
    /// pane's webview (inside its border + padding), clamped
    /// to the viewport so a press on the border still lands
    /// on the pane's edge pixel instead of a negative point.
    fn pane_content_point(&self, point: Point) -> Option<(usize, Point)> {
        let rect = self.content_rect();
        if !rect.contains(point) {
            return None;
        }
        let index = pane_at_point(rect, self.split, point);
        let pane_rect = self.pane_rect(index);
        let (w, h) = self.pane_size(index);
        let pad = if self.split { PANE_BORDER } else { 0.0 };
        let x = (point.x - pane_rect.x - pad).clamp(0.0, w as f32);
        let y = (point.y - pane_rect.y - pad).clamp(0.0, h as f32);
        Some((index, Point::new(x, y)))
    }

    /// Titlebar text: the browser name, then the ACTIVE pane's
    /// site title. Before any title arrives, the site's host
    /// stands in; internal (data:) pages show just the browser
    /// name.
    fn titlebar_text(&self) -> String {
        let pane = self.active_pane();
        let label = match &pane.title {
            Some(t) if !t.is_empty() => t.clone(),
            _ => match pane.webview.url() {
                Some(u) if matches!(u.scheme(), "http" | "https") => {
                    match u.host_str() {
                        Some(host) if !host.is_empty() => host.to_string(),
                        _ => return "Frost".to_string(),
                    }
                },
                _ => return "Frost".to_string(),
            },
        };
        format!("Frost — {label}")
    }

    /// One engine event: update the pane it came from (Round 14:
    /// the delegate tags every event with its webview's id). An
    /// event for a pane that no longer exists (a closed pane's
    /// straggler) is dropped. The status line, address bar, and
    /// OS title follow the ACTIVE pane only.
    fn handle_engine_event(&mut self, event: EngineEvent) {
        // Route first: every variant carries its webview id.
        let id = match &event {
            EngineEvent::TitleChanged(id, _)
            | EngineEvent::UrlChanged(id, _)
            | EngineEvent::LoadStarted(id)
            | EngineEvent::LoadFinished(id)
            | EngineEvent::LoadStopped(id)
            | EngineEvent::HeadParsed(id)
            | EngineEvent::FrameReady(id)
            | EngineEvent::ResourceBlocked(id, _)
            | EngineEvent::Crashed(id, _) => *id,
        };
        let Some(index) = self.pane_index_for(id) else {
            return;
        };
        let is_active = index == self.active_pane;
        // A LoadFinished also (re-)injects this pane's content
        // scripts — after the match, once the pane borrow ends.
        // A HeadParsed injects only the DocumentStart scripts.
        let mut inject_after = false;
        let mut inject_start_after = false;
        let pane = &mut self.panes[index];
        match event {
            EngineEvent::TitleChanged(_, title) => {
                pane.title = title.clone();
                if is_active {
                    self.title_dirty = true;
                    if !pane.loading {
                        pane.status = pane
                            .title
                            .clone()
                            .unwrap_or_else(|| "Untitled page".to_string());
                    }
                }
            },
            EngineEvent::UrlChanged(_, url) => {
                // Round 7: no internal pages any more — the engine
                // URL is the display URL, full stop.
                let display = url.to_string();
                if is_active && !self.address_editing {
                    self.address = display;
                }
                if is_active {
                    self.title_dirty = true;
                }
                pane.can_back = pane.webview.can_go_back();
                pane.can_forward = pane.webview.can_go_forward();
                // Round 13: history (Back/Forward) traversals commit a
                // new URL without a LoadStarted, so reset the Snowplow
                // count here too — otherwise the previous page's count
                // lingers on the new page. (Same-document fragment
                // navigations also reset it; the count is per shown URL.)
                pane.snowplow_blocked = 0;
            },
            EngineEvent::LoadStarted(_) => {
                pane.injected_for = None;
                pane.injected_start_for = None;
                // Round 13: a new page starts with a clean Snowplow count.
                pane.snowplow_blocked = 0;
                let url = pane
                    .webview
                    .url()
                    .map(|u| u.to_string())
                    .unwrap_or_default();
                pane.mark_loading(if url.is_empty() {
                    "Loading…".to_string()
                } else {
                    format!("Loading {url} …")
                });
            },
            EngineEvent::LoadFinished(_) => {
                let status = pane
                    .title
                    .clone()
                    .unwrap_or_else(|| "Done".to_string());
                pane.mark_settled(status);
                if is_active {
                    self.address_editing = false;
                }
                inject_after = true;
                // Round 4, F-007: no post-load jiggle any more — the
                // fork uses full present, so the first paint after
                // LoadFinished is complete as-is.
            },
            EngineEvent::LoadStopped(_) => {
                pane.mark_settled("Stopped.".to_string());
            },
            EngineEvent::HeadParsed(_) => {
                // Round 28: the document exists now; run DocumentStart
                // scripts before the page's own bootstrap scripts run.
                inject_start_after = true;
            },
            EngineEvent::ResourceBlocked(_, _url) => {
                // Round 13: a real count from the delegate, one event
                // per cancelled request. The status line shows it
                // (see the view); nothing is faked.
                pane.snowplow_blocked += 1;
            },
            EngineEvent::FrameReady(_) => {
                pane.frame_dirty = true;
            },
            EngineEvent::Crashed(_, reason) => {
                pane.mark_settled(format!("Page crashed: {reason}"));
            },
        }
        if inject_after {
            self.inject_content_scripts(index);
        }
        if inject_start_after {
            self.inject_start_scripts(index);
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
                if let Some((index, p)) = self.pane_content_point(position) {
                    self.panes[index]
                        .webview
                        .send_input(FrostInputEvent::MouseMove { x: p.x, y: p.y });
                }
            },
            iced::Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                if let Some(pos) = self.last_cursor {
                    if let Some((index, p)) = self.pane_content_point(pos) {
                        // Round 14: a press in a pane makes it the
                        // active one — then the press reaches the
                        // page, so one click both focuses the pane
                        // and does what the user clicked.
                        self.set_active_pane(index);
                        self.content_focused = true;
                        self.panes[index].webview.send_input(FrostInputEvent::MouseButton {
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
                    if let Some((index, p)) = self.pane_content_point(pos) {
                        self.panes[index].webview.send_input(FrostInputEvent::MouseButton {
                            button: map_button(button),
                            action: FrostMouseButtonAction::Up,
                            x: p.x,
                            y: p.y,
                        });
                    }
                }
            },
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                // Round 9: the wheel does not scroll the page
                // behind the open palette (the click-catcher
                // blocks presses, but a mouse_area never sees the
                // wheel — so the wheel is gated here instead).
                if self.palette_open {
                    return Task::none();
                }
                if let Some(pos) = self.last_cursor {
                    if let Some((index, p)) = self.pane_content_point(pos) {
                        // Servo takes pixel deltas; a "line" is ~40 px.
                        let (dx, dy) = match delta {
                            mouse::ScrollDelta::Lines { x, y } => {
                                (x as f64 * 40.0, y as f64 * 40.0)
                            },
                            mouse::ScrollDelta::Pixels { x, y } => {
                                (x as f64, y as f64)
                            },
                        };
                        self.panes[index].webview.send_input(FrostInputEvent::Wheel {
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
                // Round 9: Ctrl+K toggles the command palette — a
                // browser shortcut, like F12. Checked first, so it
                // works from the page; from the address bar or the
                // palette input it arrives as Captured instead
                // (libcosmic's text_input captures every KeyPressed
                // while focused) and the IcedEventCaptured handler
                // toggles it there. Repeats are ignored (one toggle
                // per press). Ctrl+K has no Frost meaning before
                // this round and no page meaning Frost owes the
                // page. The key never reaches the page.
                if !repeat
                    && modifiers.control()
                    && !modifiers.logo()
                    && !modifiers.alt()
                    && matches!(&key, keyboard::Key::Character(c) if c.eq_ignore_ascii_case("k"))
                {
                    return Task::done(cosmic::Action::App(Message::TogglePalette));
                }
                // Round 9: while the palette is open the page takes
                // no keys at all. Escape closes it; Up/Down move
                // the highlight (wrapping); Enter runs the
                // highlighted command. The input's on_submit also
                // fires Enter — only one path can fire per press:
                // a captured Enter publishes on_submit and never
                // reaches this handler, an uncaptured one lands
                // here. Anything else is swallowed: typing goes to
                // the input through its own capture.
                if self.palette_open {
                    if !repeat
                        && matches!(key, keyboard::Key::Named(keyboard::key::Named::Escape))
                    {
                        self.close_palette();
                        return Task::none();
                    }
                    match key {
                        keyboard::Key::Named(keyboard::key::Named::ArrowDown) => {
                            let len = self.palette_filtered().len();
                            self.palette_selected =
                                move_selection(self.palette_selected, len, 1);
                            return Task::none();
                        },
                        keyboard::Key::Named(keyboard::key::Named::ArrowUp) => {
                            let len = self.palette_filtered().len();
                            self.palette_selected =
                                move_selection(self.palette_selected, len, -1);
                            return Task::none();
                        },
                        keyboard::Key::Named(keyboard::key::Named::Enter) => {
                            return Task::done(cosmic::Action::App(
                                Message::PaletteRunSelected,
                            ));
                        },
                        _ => return Task::none(),
                    }
                }
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
                // Round 9: no key releases reach the page while the
                // palette is open, either.
                if self.content_focused && !self.menu_open && !self.palette_open {
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

    /// A keyboard event a widget CAPTURED (Round 9): the only
    /// capturer that matters is the palette's query input — a
    /// focused text input swallows Escape (it just unfocuses
    /// itself), so without this path the palette could never
    /// close from a focused input. Every other captured key is
    /// the widget's own business (typing in the address bar stays
    /// in the address bar) and is ignored here. Only the main
    /// window's events reach the app at all.
    fn handle_iced_event_captured(
        &mut self,
        id: window::Id,
        event: iced::Event,
    ) -> Task<Message> {
        if self.core.main_window_id() != Some(id) {
            return Task::none();
        }
        // Round 9, fix: libcosmic's text_input captures EVERY
        // KeyPressed while it is focused (vendored
        // src/widget/text_input/input.rs — the handler ends in an
        // unconditional shell.capture_event(), except for Escape
        // and Up/Down, which return early without capturing). So
        // a Ctrl+K pressed while the palette input — or the
        // address bar — has focus never reaches the Ignored path
        // where the toggle lives; it arrives here instead. Gated
        // 2026-11-15: three Ctrl+K presses with the palette open
        // (fast chord x2, slow chord x1) all left the palette
        // open until this branch was added. Handle the toggle
        // here, exactly as the KeyPressed handler does — an event
        // is Captured or Ignored, never both, so it fires once.
        if let iced::Event::Keyboard(keyboard::Event::KeyPressed {
            key,
            modifiers,
            repeat,
            ..
        }) = event
        {
            if !repeat
                && modifiers.control()
                && !modifiers.logo()
                && !modifiers.alt()
                && matches!(&key, keyboard::Key::Character(c) if c.eq_ignore_ascii_case("k"))
            {
                return Task::done(cosmic::Action::App(Message::TogglePalette));
            }
            // Finisher fix: F12 is captured by a focused text
            // input exactly like Ctrl+K (see above), so with the
            // address bar focused it never reached the
            // subscription's F12 handler — the Inspector only
            // opened after clicking into the page first. Handle
            // it here, mirroring that handler (open the snapshot,
            // or just focus an Inspector window that is already
            // open). An event is Captured or Ignored, never both,
            // so it fires once.
            if !repeat
                && matches!(key, keyboard::Key::Named(keyboard::key::Named::F12))
            {
                if let Some(id) = self.popup_id(windows::PopupKind::Inspector) {
                    return window::gain_focus(id);
                }
                self.capture_inspector();
            }
            if self.palette_open
                && matches!(key, keyboard::Key::Named(keyboard::key::Named::Escape))
            {
                self.close_palette();
            }
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
        self.active_pane().webview.send_input(FrostInputEvent::Key {
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

    /// Capture the ACTIVE pane's DOM for the Inspector window
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
        if self.active_pane().webview.url().is_none() {
            self.active_pane_mut().status =
                "Inspector: no page loaded yet.".to_string();
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        self.active_pane()
            .webview
            .evaluate_javascript_string(DOM_WALK_SCRIPT, tx);
        self.inspector_shot_rx = Some(rx);
        self.inspector_shot_pending = true;
        self.active_pane_mut().status = "Capturing page structure…".to_string();
    }

    /// Freeze the ACTIVE pane's page as a Deep Freeze archive
    /// (Round 15): the capture script runs on a clone of the live
    /// DOM (never mutating it) and its outerHTML arrives on the
    /// channel; the Tick writes the archive file and says so in
    /// the status line. The page itself never leaves the main
    /// window, so there is no navigation to wait for.
    fn freeze_page(&mut self) {
        if self.freeze_pending {
            return;
        }
        if self.active_pane().webview.url().is_none() {
            self.active_pane_mut().status = "Freeze: no page loaded yet.".to_string();
            return;
        };
        self.freeze_pending_url = self
            .active_pane()
            .webview
            .url()
            .map(|u| u.to_string())
            .unwrap_or_default();
        self.freeze_pending_title = self.freeze_title();
        let (tx, rx) = std::sync::mpsc::channel();
        self.active_pane()
            .webview
            .evaluate_javascript_string(archive::FREEZE_CAPTURE_SCRIPT, tx);
        self.freeze_rx = Some(rx);
        self.freeze_pending = true;
        self.active_pane_mut().status = "Freezing page…".to_string();
    }

    /// The title to freeze the ACTIVE pane under: the page title
    /// when it has one, else the host, else the URL, else
    /// "Untitled page".
    fn freeze_title(&self) -> String {
        let pane = self.active_pane();
        if let Some(t) = &pane.title {
            if !t.trim().is_empty() {
                return t.trim().to_string();
            }
        }
        if let Some(u) = pane.webview.url() {
            if let Some(host) = u.host_str() {
                if !host.is_empty() {
                    return host.to_string();
                }
            }
            return u.to_string();
        }
        "Untitled page".to_string()
    }
    /// Runs once per URL, on LoadFinished (and on the 15 s safety
    /// net, for history traversals that never fire LoadFinished).
    /// Runs every script EXCEPT `DocumentStart` ones that already ran
    /// at HeadParsed — those are re-run here only if HeadParsed never
    /// fired for this URL (fallback, so a missed HeadParsed does not
    /// silently drop a start script). Round 7: there are no internal
    /// pages any more, so every URL the engine loads is a real page —
    /// the old `frost://` / loopback guard is gone.
    fn inject_content_scripts(&mut self, index: usize) {
        self.inject_content_scripts_filtered(index, false);
    }

    /// Runs once per URL, on HeadParsed: only `DocumentStart` scripts,
    /// so they execute before the page's own bootstrap scripts run
    /// (Round 28: wrapping `window.fetch` at load finish is too late —
    /// the app has already cached its own reference).
    fn inject_start_scripts(&mut self, index: usize) {
        self.inject_content_scripts_filtered(index, true);
    }

    fn inject_content_scripts_filtered(&mut self, index: usize, start_only: bool) {
        let Some(url) = self.panes[index].webview.url().map(|u| u.to_string()) else {
            return;
        };
        if start_only {
            if self.panes[index]
                .injected_start_for
                .as_deref()
                .is_some_and(|u| u == url.as_str())
            {
                return;
            }
            self.panes[index].injected_start_for = Some(url.clone());
        } else {
            if self.panes[index]
                .injected_for
                .as_deref()
                .is_some_and(|u| u == url.as_str())
            {
                return;
            }
            self.panes[index].injected_for = Some(url.clone());
        }
        // In the finish pass, a DocumentStart script that already ran
        // at HeadParsed must not run twice.
        let start_ran = self.panes[index]
            .injected_start_for
            .as_deref()
            .is_some_and(|u| u == url.as_str());
        let scripts = self.host.content_scripts_for(&url);
        for (_, script) in scripts {
            let is_start = script.run_at == RunAt::DocumentStart;
            if start_only && !is_start {
                continue;
            }
            if !start_only && is_start && start_ran {
                continue;
            }
            self.panes[index].webview.evaluate_javascript(&script.js);
        }
    }
}

impl cosmic::Application for Frost {
    type Executor = executor::Default;
    type Flags = bool;
    type Message = Message;

    const APP_ID: &'static str = "dev.frost.Frost";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(mut core: Core, flags: bool) -> (Self, Task<Message>) {
        // Round 10: `flags` is whether Frost draws its own
        // titlebar (GNOME fallback) or the window manager does.
        // The built-in libcosmic headerbar renders the title but
        // no window-control buttons on this setup, so it stays
        // hidden in both modes rather than left as an empty strip.
        let client_decorations = flags;
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
        // Round 13: the loaded setting decides whether Snowplow
        // blocks from the first page (default ON).
        engine.set_snowplow_enabled(settings.snowplow);
        let window_size = Size::new(INITIAL_WINDOW.0, INITIAL_WINDOW.1);
        let content_height = INITIAL_WINDOW.1
            - titlebar_height_for(client_decorations)
            - TOOLBAR_HEIGHT
            - STATUS_HEIGHT;
        let webview = engine
            .spawn_webview(&homepage, INITIAL_WINDOW.0 as u32, content_height as u32)
            .expect("frost: could not spawn the first webview");
        webview.set_theme(settings.theme.is_dark());
        (
            Self {
                core,
                engine,
                panes: vec![Pane::new(webview, format!("Loading {homepage} …"))],
                active_pane: 0,
                split: false,
                address: homepage.clone(),
                address_editing: false,
                menu_open: false,
                menu_hovered: None,
                palette_open: false,
                palette_query: String::new(),
                palette_selected: 0,
                window_size,
                client_decorations,
                last_cursor: None,
                content_focused: true,
                title_dirty: true,
                print_shot_rx: None,
                print_shot_pending: false,
                print_pending_label: String::new(),
                print_pending_title: String::new(),
                print_pending_url: String::new(),
                inspector_shot_rx: None,
                inspector_shot_pending: false,
                inspector_recapture: false,
                freeze_rx: None,
                freeze_pending: false,
                freeze_pending_url: String::new(),
                freeze_pending_title: String::new(),
                settings,
                extensions: loaded,
                failed_extensions: failed,
                settings_win: None,
                print_win: None,
                open_win: None,
                inspector_win: None,
                archives_win: None,
                exit_win: None,
                host,
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
                    iced::event::Status::Captured => match &event {
                        // Round 9: libcosmic's text_input captures
                        // EVERY KeyPressed while it is focused (its
                        // handler ends in an unconditional
                        // capture_event, except Escape and Up/Down,
                        // which return early without capturing) —
                        // so a Ctrl+K from the palette input or
                        // the address bar arrives here, not on the
                        // Ignored path. IcedEventCaptured handles
                        // the palette toggle for those; Captured
                        // mouse events stay dropped, so the address
                        // bar's other captured keys still never
                        // reach the page.
                        iced::Event::Keyboard(_) => {
                            Some(Message::IcedEventCaptured(id, event))
                        },
                        _ => None,
                    },
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
        self.resize_panes();
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
                // Round 14: per pane — one pane's stuck load must
                // not freeze the other pane's render.
                for i in 0..self.panes.len() {
                    if self.panes[i].awaiting_load {
                        if let Some(since) = self.panes[i].loading_since {
                            if since.elapsed() > Duration::from_secs(15) {
                                // A history traversal (back/forward)
                                // restores from cache and never fires
                                // LoadFinished, so without this the status
                                // would read "Going back…" forever.
                                let status = self.panes[i]
                                    .title
                                    .clone()
                                    .unwrap_or_else(|| "Done".to_string());
                                self.panes[i].mark_settled(status);
                                self.panes[i].frame_dirty = true;
                                // The traversal's page still gets its
                                // content scripts (no LoadFinished fired).
                                self.inject_content_scripts(i);
                            }
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
                // Round 14: each pane renders on its own — a
                // loading pane never blocks the other pane's paint.
                for i in 0..self.panes.len() {
                    if self.panes[i].frame_dirty && !self.panes[i].awaiting_load {
                        if let Some(frame) = self.panes[i].webview.render() {
                            self.panes[i].frame_strips = frame
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
                            self.panes[i].frame_dirty = false;
                        }
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
                                self.active_pane_mut().status =
                                    format!("Inspector failed: {e}");
                            },
                        }
                    }
                }
                // Round 15 Deep Freeze: the frozen outerHTML
                // arrives on this channel once the capture script
                // runs in the page. Write the archive file and
                // say so in the status line. A failed capture
                // leaves the current page up and says so.
                if let Some(rx) = &self.freeze_rx {
                    if let Ok(result) = rx.try_recv() {
                        self.freeze_rx = None;
                        self.freeze_pending = false;
                        let url = std::mem::take(&mut self.freeze_pending_url);
                        let title = std::mem::take(&mut self.freeze_pending_title);
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        match result.and_then(|html| {
                            archive::save_archive(
                                &archive::archives_dir(),
                                &title,
                                &url,
                                &html,
                                now,
                            )
                        }) {
                            Ok(entry) => {
                                self.active_pane_mut().status =
                                    format!("Frozen: {}", entry.title);
                            },
                            Err(e) => {
                                self.active_pane_mut().status =
                                    format!("Freeze failed: {e}");
                            },
                        }
                    }
                }
                // Keep the OS window title in step with the page
                // title (Round 10): the window manager shows it
                // in WM mode, Frost's own titlebar in the GNOME
                // fallback — either way it must stay correct.
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
            Message::IcedEventCaptured(id, event) => {
                self.handle_iced_event_captured(id, event)
            },
            Message::Back => {
                // Round 4, F-008: mark loading NOW (see
                // load_loopback) so Stop is live even if this
                // traversal never reports a load start.
                let pane = self.active_pane_mut();
                pane.mark_loading("Going back…".to_string());
                pane.webview.go_back();
                Task::none()
            },
            Message::Forward => {
                // Round 4, F-008: mark loading NOW (see
                // load_loopback).
                let pane = self.active_pane_mut();
                pane.mark_loading("Going forward…".to_string());
                pane.webview.go_forward();
                Task::none()
            },
            Message::Reload => {
                let pane = self.active_pane_mut();
                pane.mark_loading("Reloading…".to_string());
                // Round 4, F-006: the fork's `reload()` now notifies
                // the delegate via `set_load_status`, so the engine
                // reports `LoadStarted` for a reload — which clears
                // the content-script guard. The app-side workaround
                // (clearing `injected_for` here) is RETIRED; setting
                // `loading` here is kept so Stop stays live if the
                // reload's first response never arrives (F-008).
                pane.webview.reload();
                Task::none()
            },
            Message::Stop => {
                let pane = self.active_pane_mut();
                match pane.webview.stop() {
                    Ok(()) => pane.status = "Stopping…".to_string(),
                    Err(e) => pane.status = format!("Stop failed: {e}"),
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
                    self.active_pane_mut().status = "Frost no longer has internal pages — Settings, Print, Open file, and Inspector open in their own windows from the ☰ menu.".to_string();
                    return Task::none();
                }
                let url = resolve_address(&trimmed, &self.settings);
                match self.active_pane().webview.load(&url) {
                    Ok(()) => {
                        // Round 4, F-008: mark the load in flight
                        // NOW (see load_url_in_main) so Stop stays
                        // live if the server never responds and
                        // LoadStarted never fires.
                        self.active_pane_mut()
                            .mark_loading(format!("Loading {url} …"));
                        self.address = url.clone();
                        self.address_editing = false;
                        self.content_focused = true;
                    },
                    Err(e) => {
                        self.active_pane_mut().status =
                            format!("Cannot load {url}: {e}");
                    },
                }
                Task::none()
            },
            Message::ToggleSplit => {
                // Finisher fix: every other menu/palette command
                // closes both overlays first; ToggleSplit closed
                // neither, so picking "Split view" / "Close split
                // view" from the menu left the menu open over the
                // new split. (The palette's own close happens in
                // PaletteRun before dispatch, so this is a no-op
                // from the palette path.)
                self.close_menu();
                self.close_palette();
                self.toggle_split()
            },
            Message::ToggleMenu => {
                // Round 8: the menu floats over the content, so
                // opening it does not resize the webview. Round 9:
                // opening the menu closes the palette, and the
                // palette's toggle does the reverse — the two
                // overlays never share the screen.
                if self.menu_open {
                    self.close_menu();
                } else {
                    self.close_palette();
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
            Message::TogglePalette => {
                // Round 9: opening the palette closes the menu.
                // The query input is focused on open, so typing
                // lands in it at once; the page keeps whatever
                // focus it had (the key handler gates page keys
                // on !palette_open, so nothing leaks while the
                // palette is up, and focus is exactly as it was
                // once it closes).
                if self.palette_open {
                    self.close_palette();
                    Task::none()
                } else {
                    self.close_menu();
                    self.palette_open = true;
                    self.palette_query.clear();
                    self.palette_selected = 0;
                    text_input::focus(palette_input_id())
                }
            },
            Message::ClosePalette => {
                self.close_palette();
                Task::none()
            },
            Message::PaletteInputChanged(value) => {
                self.palette_query = value;
                self.palette_selected = 0;
                Task::none()
            },
            Message::PaletteSelect(index) => {
                self.palette_selected = index;
                Task::none()
            },
            Message::PaletteRun(index) => {
                // Resolve against the CURRENT filtered list (the
                // view and this path derive it the same way, so
                // the index always means the row the user saw),
                // close the palette, then run the real message —
                // the palette adds no behavior of its own.
                let commands = self.palette_filtered();
                match commands.get(index) {
                    Some((_, message)) => {
                        let message = message.clone();
                        self.close_palette();
                        self.update(message)
                    },
                    None => Task::none(),
                }
            },
            Message::PaletteRunSelected => {
                let index = self.palette_selected;
                self.update(Message::PaletteRun(index))
            },
            Message::FocusAddress => {
                // Round 9: the palette's "Focus address bar"
                // command. The bar gets the URL selected, so
                // typing replaces it at once; keys go to the bar,
                // not the page, until Enter or a click elsewhere.
                self.close_palette();
                self.close_menu();
                self.address_editing = true;
                self.content_focused = false;
                Task::batch([
                    text_input::focus(address_input_id()),
                    text_input::select_all(address_input_id()),
                ])
            },
            Message::OpenSettings => {
                self.close_menu();
                self.close_palette();
                self.open_settings_window()
            },
            Message::OpenPrint => {
                // F-003: capture the CURRENT page first — the
                // preview must be of the page the user was reading.
                // The Print window opens from the Tick once the PNG
                // bytes arrive (or the capture errors). A Print
                // window that is already open is just focused.
                self.close_menu();
                self.close_palette();
                if let Some(id) = self.popup_id(windows::PopupKind::Print) {
                    return window::gain_focus(id);
                }
                if self.print_shot_pending {
                    return Task::none();
                }
                let (tx, rx) = std::sync::mpsc::channel();
                self.active_pane().webview.request_screenshot_png(tx);
                self.print_shot_rx = Some(rx);
                self.print_shot_pending = true;
                self.active_pane_mut().status =
                    "Capturing print preview…".to_string();
                // Freeze the printed page's identity here: the
                // Print window renders from these.
                let pane_url = self
                    .active_pane()
                    .webview
                    .url()
                    .map(|u| u.to_string())
                    .unwrap_or_else(|| self.address.clone());
                self.print_pending_label = format!("Preview of {pane_url}");
                self.print_pending_title =
                    self.active_pane().title.clone().unwrap_or_default();
                self.print_pending_url = pane_url;
                Task::none()
            },
            Message::OpenInspector => {
                // Round 5: capture the current page's DOM, then
                // open the Inspector window on the snapshot (see
                // capture_inspector). An Inspector window that is
                // already open is just focused.
                self.close_menu();
                self.close_palette();
                if let Some(id) = self.popup_id(windows::PopupKind::Inspector) {
                    window::gain_focus(id)
                } else {
                    self.capture_inspector();
                    Task::none()
                }
            },
            Message::FreezePage => {
                // Round 15: freeze the ACTIVE pane's page. The
                // archive is written from the Tick once the
                // capture script's outerHTML arrives.
                self.close_menu();
                self.close_palette();
                self.freeze_page();
                Task::none()
            },
            Message::OpenArchives => {
                // Round 15: the Archived pages window (or focus
                // it, if open).
                self.close_menu();
                self.close_palette();
                self.open_archives_window()
            },
            Message::OpenFile => {
                self.close_menu();
                self.close_palette();
                let home = file_browser::home_dir();
                self.open_file_window(&home)
            },
            Message::Exit => {
                self.close_menu();
                self.close_palette();
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
        // Titlebar (Round 10: Frost's own, GNOME fallback only —
        // the built-in libcosmic headerbar is hidden): browser
        // name + current site title, and real minimize /
        // maximize-restores / close buttons. Text glyphs, not
        // icons, so the buttons are visible on any setup.
        // Pressing the title (not a button) drags the window;
        // double-press toggles maximize. The buttons sit outside
        // the drag area, so a click on one presses the button.
        // The row is only in the column when
        // `self.client_decorations` is set (see below); in WM
        // mode the toolbar is the top row.
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

        // Toolbar: back / forward / reload-or-stop, address bar,
        // menu. Back/forward only get an on_press when they can
        // act — without one the button renders disabled, never
        // live-looking while doing nothing. The reload/stop slot
        // is always live: reload when idle, stop while a load is
        // in flight (Round 11). All four icon buttons are round
        // (Round 11).
        // Round 12: proper chevrons (❮/❯, U+276E/U+276F) instead
        // of the single guillemets (‹/›) — the guillemets' ink
        // is only ~6×9 px at 20 px and sits low, which is the
        // "icons aren't right" Joel reported; the heavy chevrons
        // are ~12×21 px and read as real toolbar icons. Their
        // ink sits ~2 px low, corrected by the nudge.
        let back = {
            let b = round_icon_button(
                "❮",
                button::ButtonClass::Standard,
                TOOLBAR_CHEVRON_NUDGE_UP,
            );
            if self.active_pane().can_back {
                b.on_press(Message::Back)
            } else {
                b
            }
        };
        let forward = {
            let b = round_icon_button(
                "❯",
                button::ButtonClass::Standard,
                TOOLBAR_CHEVRON_NUDGE_UP,
            );
            if self.active_pane().can_forward {
                b.on_press(Message::Forward)
            } else {
                b
            }
        };
        // Round 11: one reload/stop button — the reload icon
        // while idle, an X while a page is loading (the
        // Firefox/Chrome pattern). It flips back to reload both
        // when the load finishes and when the user stops it.
        let (reload_stop_label, reload_stop_message) =
            reload_stop_button(self.active_pane().loading);
        let reload_stop =
            round_icon_button(reload_stop_label, button::ButtonClass::Standard, 0.0)
                .on_press(reload_stop_message);
        // ☰ button: the suggested (accent) style while the menu is
        // open is the pressed/active state; standard otherwise.
        let menu_button = if self.menu_open {
            round_icon_button("☰", button::ButtonClass::Suggested, 0.0)
                .on_press(Message::ToggleMenu)
        } else {
            round_icon_button("☰", button::ButtonClass::Standard, 0.0)
                .on_press(Message::ToggleMenu)
        };
        let toolbar_row = widget::row::with_children(vec![
            back.into(),
            forward.into(),
            reload_stop.into(),
            text_input("Address", &self.address)
                .id(address_input_id())
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

        // Web content area (Round 14): in single view, the live
        // Servo frame as horizontal strips (Round 6), or a loading
        // note until the first frame paints. In split view, two
        // panes side by side, each its own frame — the active
        // pane wears a 2 px border (the inactive pane's border is
        // transparent at the same width, so switching the active
        // pane never shifts layout).
        let content: Element<Message> = if !self.split {
            pane_element(&self.panes[0], colors, false, false)
        } else {
            widget::row::with_children(vec![
                pane_element(&self.panes[0], colors, true, self.active_pane == 0),
                pane_element(&self.panes[1], colors, true, self.active_pane == 1),
            ])
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        };

        // Round 13: the status line shows the real per-page Snowplow
        // count when anything was blocked on this page — the ACTIVE
        // pane's count, in split view.
        let pane = self.active_pane();
        let status_text = if pane.snowplow_blocked > 0 {
            format!(
                "{} · Snowplow blocked {}",
                pane.status, pane.snowplow_blocked
            )
        } else {
            pane.status.clone()
        };
        let status = container(text(status_text).size(13))
            .style(move |_| chrome_container_style(colors))
            .width(Length::Fill)
            .height(Length::Fixed(STATUS_HEIGHT))
            .padding(6);

        // Round 8: the main column (titlebar in the GNOME
        // fallback, otherwise toolbar / content / status) is the
        // Stack's base layer. When the menu is open,
        // two layers float over it: a transparent full-window
        // click-catcher (any press outside the panel closes the
        // menu, and — like Firefox — does not click through to the
        // page or toolbar underneath), then the dropdown panel
        // itself, anchored top-right under the ☰ button. The Stack
        // dispatches events top-first, so the panel's own buttons
        // still win where they overlap the catcher.
        // Round 10: the titlebar row joins the column only in
        // the GNOME fallback — in WM mode the toolbar is the top
        // row and the window manager draws the titlebar.
        let mut column_children: Vec<Element<'_, Message>> = Vec::new();
        if self.client_decorations {
            column_children.push(titlebar.into());
        }
        column_children.push(toolbar.into());
        column_children.push(content);
        column_children.push(status.into());
        let main_column = widget::column::with_children(column_children);
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
        // Round 9: the command palette floats over the content the
        // same way — a DIMMED full-window click-catcher (any press
        // outside the panel closes the palette without clicking
        // through to the page or toolbar; the dim is what makes
        // the page read as blocked behind the panel), then the
        // centered panel itself.
        if self.palette_open {
            let catcher = mouse_area(
                container(
                    widget::Space::new()
                        .width(Length::Fill)
                        .height(Length::Fill),
                )
                .style(|_| palette_dim_style()),
            )
            .on_press(Message::ClosePalette);
            stack = stack.push(catcher);
            stack = stack.push(self.palette_panel(colors));
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

/// One split-view pane's content element (Round 14): the pane's
/// frame strips (or a loading note until the first frame paints).
/// The strips tile the pane exactly — each is the frame's full
/// width at its own height, no scaling, no seams — so the column
/// is the whole frame at 1:1. In split view the element is wrapped
/// in a 2 px border + the same padding: the ACTIVE pane's border
/// is the theme's border color, the inactive pane's is
/// transparent at the same width, so switching the active pane
/// never shifts layout. In single view there is no wrapper at
/// all — the content is exactly as it was before split view.
fn pane_element<'a>(
    pane: &'a Pane,
    colors: ChromeColors,
    split: bool,
    active: bool,
) -> Element<'a, Message> {
    let inner: Element<'_, Message> = if pane.frame_strips.is_empty() {
        container(text("Loading…").size(16))
            .style(move |_| chrome_container_style(colors))
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    } else {
        widget::column::with_children(
            pane.frame_strips
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
    if !split {
        return inner;
    }
    let border_color = if active {
        rgb(colors.menu_border)
    } else {
        cosmic::iced::Color::TRANSPARENT
    };
    container(inner)
        .style(move |_| pane_border_style(border_color))
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(PANE_BORDER)
        .into()
}

/// The split-view pane border (Round 14): a 2 px ring in `color`
/// (the theme's border color for the active pane, transparent for
/// the inactive one), square corners, no background — the page
/// shows through.
fn pane_border_style(color: cosmic::iced::Color) -> cosmic::iced::widget::container::Style {
    cosmic::iced::widget::container::Style {
        border: cosmic::iced::Border {
            color,
            width: PANE_BORDER,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
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

/// One palette row (Round 9): the command label, full-width click
/// target. Same shape as a menu entry — a `mouse_area`, not a
/// button (see `menu_item` for why) — with the same hover fill for
/// the highlighted row. The pointer entering a row makes it the
/// highlighted one, so mouse and keyboard selection never disagree.
fn palette_row<'a>(
    colors: ChromeColors,
    index: usize,
    selected: bool,
    label: &'a str,
) -> Element<'a, Message> {
    let row = widget::row::with_children(vec![text(label).size(14).into()])
        .align_y(iced::Alignment::Center);
    let style = if selected {
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
    .on_press(Message::PaletteRun(index))
    .on_enter(Message::PaletteSelect(index))
    .into()
}

/// The dim behind the palette (Round 9): a translucent black wash
/// over the whole window, so the page reads as blocked while the
/// palette is open. Same wash in both themes.
fn palette_dim_style() -> cosmic::iced::widget::container::Style {
    cosmic::iced::widget::container::Style {
        background: Some(cosmic::iced::Background::Color(
            cosmic::iced::Color::from_rgba(0.0, 0.0, 0.0, 0.4),
        )),
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
    fn gnome_falls_back_to_the_client_titlebar() {
        // GNOME/Mutter refuses server-side decorations, so
        // Frost draws its own titlebar there.
        assert!(use_client_decorations("GNOME"));
        assert!(use_client_decorations("ubuntu:GNOME"));
        assert!(use_client_decorations("Gnome"));
        assert!(use_client_decorations("GNOME-Classic"));
    }

    #[test]
    fn other_desktops_and_unset_use_the_wm_titlebar() {
        for desktop in ["KDE", "Plasma", "XFCE", "COSMIC", "Hyprland", "Sway", ""] {
            assert!(
                !use_client_decorations(desktop),
                "{desktop:?} must use the WM titlebar"
            );
        }
    }

    #[test]
    fn titlebar_height_is_40_in_the_fallback_and_0_in_wm_mode() {
        assert_eq!(titlebar_height_for(true), TITLEBAR_HEIGHT);
        assert_eq!(titlebar_height_for(false), 0.0);
    }

    #[test]
    fn menu_items_are_nine_with_f12_ctrl_k_split_view_and_freeze() {
        assert_eq!(MENU_ITEMS.len(), 9);
        assert_eq!(SPLIT_MENU_INDEX, 5);
        let labels: Vec<&str> = MENU_ITEMS.iter().map(|(label, _, _)| *label).collect();
        assert_eq!(
            labels,
            [
                "Settings",
                "Print page…",
                "Open file…",
                "Inspector",
                "Command palette…",
                "Split view",
                "Freeze this page…",
                "Archived pages…",
                "Exit"
            ]
        );
        for (label, shortcut, _) in MENU_ITEMS {
            match label {
                "Inspector" => assert_eq!(shortcut, Some("F12")),
                "Command palette…" => assert_eq!(shortcut, Some("Ctrl+K")),
                _ => assert_eq!(shortcut, None, "{label} must not invent a shortcut"),
            }
        }
        assert!(matches!(MENU_ITEMS[SPLIT_MENU_INDEX].2, Message::ToggleSplit));
        assert!(matches!(MENU_ITEMS[6].2, Message::FreezePage));
        assert!(matches!(MENU_ITEMS[7].2, Message::OpenArchives));
    }

    #[test]
    fn fuzzy_match_is_ordered_case_insensitive_subsequence() {
        assert!(fuzzy_match("", "Open Settings"));
        assert!(fuzzy_match("set", "Open Settings"));
        assert!(fuzzy_match("SET", "Open Settings"));
        assert!(fuzzy_match("opst", "Open Settings"));
        assert!(fuzzy_match("go", "Go back"));
        assert!(fuzzy_match("go", "Go forward"));
        assert!(fuzzy_match("rl", "Reload"));
        assert!(!fuzzy_match("xyz", "Open Settings"));
        // Out of order: no 'f' after the 'b' in "Go back".
        assert!(!fuzzy_match("gfb", "Go back"));
        assert!(!fuzzy_match("gfb", "Go forward"));
        assert!(!fuzzy_match("settings open", "Open Settings"));
    }

    #[test]
    fn fuzzy_score_ranks_prefix_above_scattered() {
        // "re" is a contiguous prefix of "Reload" but a scattered
        // mid-string match in "Freeze this page" — the prefix must
        // score higher.
        let reload = fuzzy_score("re", "Reload").unwrap();
        let freeze = fuzzy_score("re", "Freeze this page").unwrap();
        assert!(reload > freeze, "Reload {reload} should beat Freeze {freeze}");
        // Same shape: "op" is a prefix of "Open Settings" but
        // mid-string in "Stop loading".
        let open = fuzzy_score("op", "Open Settings").unwrap();
        let stop = fuzzy_score("op", "Stop loading").unwrap();
        assert!(open > stop, "Open {open} should beat Stop {stop}");
        // Contiguous beats scattered: "set" is contiguous in
        // "Open Settings" and matches nothing in "Split view".
        assert!(fuzzy_score("set", "Open Settings").is_some());
        assert!(fuzzy_score("set", "Split view").is_none());
        // An empty query matches everything with score 0.
        assert_eq!(fuzzy_score("", "Open Settings"), Some(0));
    }

    #[test]
    fn palette_ranking_puts_best_match_first() {
        // The always-available table (nothing to go back/forward
        // to, nothing loading, no page loaded).
        let table = palette_commands_for(false, false, false, false, false);
        // Empty query: the table unchanged, in table order.
        let ranked = rank_palette_commands("", table.clone());
        assert_eq!(
            ranked.iter().map(|(l, _)| *l).collect::<Vec<_>>(),
            table.iter().map(|(l, _)| *l).collect::<Vec<_>>()
        );
        // "re": "Reload" is the contiguous prefix match, so it
        // ranks first; the scattered matches follow.
        let ranked = rank_palette_commands("re", palette_commands_for(false, false, false, false, false));
        let labels: Vec<&str> = ranked.iter().map(|(l, _)| *l).collect();
        assert_eq!(labels[0], "Reload");
        assert_eq!(labels.len(), 4);
        assert!(labels.contains(&"Print page…"));
        assert!(labels.contains(&"Archived pages…"));
        assert!(labels.contains(&"Focus address bar"));
        // "set": "Open Settings" first (contiguous SET at 5-7);
        // "Open Inspector" also matches as a scattered subsequence
        // (s…e…t), so it follows — the palette lists all matches.
        let ranked = rank_palette_commands("set", palette_commands_for(false, false, false, false, false));
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].0, "Open Settings");
        assert_eq!(ranked[1].0, "Open Inspector");
        // No match at all: empty list, never a stale row.
        assert!(rank_palette_commands("zzz", palette_commands_for(false, false, false, false, false)).is_empty());
    }

    #[test]
    fn palette_commands_hide_unavailable_actions() {
        // Nothing to go back/forward to, nothing loading, no page
        // loaded: only the always-available commands show.
        let labels: Vec<&str> = palette_commands_for(false, false, false, false, false)
            .iter()
            .map(|(label, _)| *label)
            .collect();
        assert_eq!(
            labels,
            [
                "Reload",
                "Open Settings",
                "Print page…",
                "Open file…",
                "Open Inspector",
                "Split view",
                "Archived pages…",
                "Focus address bar",
                "Quit Frost",
            ]
        );
        // With history and a load in flight, the conditional
        // commands appear in place.
        let labels: Vec<&str> = palette_commands_for(true, true, true, false, false)
            .iter()
            .map(|(label, _)| *label)
            .collect();
        assert_eq!(
            labels,
            [
                "Go back",
                "Go forward",
                "Reload",
                "Stop loading",
                "Open Settings",
                "Print page…",
                "Open file…",
                "Open Inspector",
                "Split view",
                "Archived pages…",
                "Focus address bar",
                "Quit Frost",
            ]
        );
        // While split, the split command reads as a close.
        let labels: Vec<&str> = palette_commands_for(false, false, false, true, false)
            .iter()
            .map(|(label, _)| *label)
            .collect();
        assert!(labels.contains(&"Close split view"));
        assert!(!labels.contains(&"Split view"));
        // Freeze appears only when a page is loaded (Round 15).
        let labels: Vec<&str> = palette_commands_for(false, false, false, false, true)
            .iter()
            .map(|(label, _)| *label)
            .collect();
        assert!(labels.contains(&"Freeze this page"));
        let labels: Vec<&str> = palette_commands_for(false, false, false, false, false)
            .iter()
            .map(|(label, _)| *label)
            .collect();
        assert!(!labels.contains(&"Freeze this page"));
    }

    #[test]
    fn menu_split_index_still_points_at_split_view() {
        // SPLIT_MENU_INDEX is also covered by the nine-item menu
        // test; this keeps the index itself pinned.
        assert_eq!(MENU_ITEMS[SPLIT_MENU_INDEX].0, "Split view");
    }

    #[test]
    fn reload_stop_button_shows_reload_when_idle_and_stop_when_loading() {
        let (label, message) = reload_stop_button(false);
        assert_eq!(label, "⟳");
        assert!(matches!(message, Message::Reload));
        let (label, message) = reload_stop_button(true);
        assert_eq!(label, "✕");
        assert!(matches!(message, Message::Stop));
    }

    #[test]
    fn toolbar_icon_size_fills_the_toolbar_inner_height() {
        // The row is TOOLBAR_HEIGHT tall with 8 px of padding
        // top and bottom, so the round icon buttons must be
        // exactly the inner height to keep their current
        // visual size — square, never oval.
        assert_eq!(TOOLBAR_ICON_SIZE, TOOLBAR_HEIGHT - 2.0 * 8.0);
        assert_eq!(TOOLBAR_ICON_SIZE, 32.0);
    }

    #[test]
    fn toolbar_glyph_is_bigger_than_the_old_textbutton_default() {
        // Round 12: the Round 11 glyphs rendered ~10 px tall
        // because libcosmic's TextButton labels at 14 px, and
        // sat at the button's left edge. The glyph must be
        // clearly larger than that default, and still fit
        // inside the 32 px circle with room to spare on every
        // side (centering itself is verified by the gate
        // screenshots, not by a unit test).
        assert!(TOOLBAR_GLYPH_SIZE > 14.0);
        assert_eq!(TOOLBAR_GLYPH_SIZE, 20.0);
        assert!(TOOLBAR_GLYPH_SIZE < TOOLBAR_ICON_SIZE);
        assert!(TOOLBAR_ICON_SIZE - TOOLBAR_GLYPH_SIZE >= 12.0);
    }

    #[test]
    fn toolbar_chevron_nudge_is_small_and_positive() {
        // Round 12: the ❮/❯ chevrons' ink sits ~2 px low in
        // the em next to ↻/☰/✕ at the same size, so their
        // buttons nudge the glyph up by this much. The nudge
        // must be positive but small — a big nudge would push
        // the glyph off-center the other way, and the bottom
        // padding it turns into (2× the nudge) must leave the
        // glyph room inside the 32 px button.
        assert!(TOOLBAR_CHEVRON_NUDGE_UP > 0.0);
        assert_eq!(TOOLBAR_CHEVRON_NUDGE_UP, 2.0);
        assert!(2.0 * TOOLBAR_CHEVRON_NUDGE_UP < TOOLBAR_GLYPH_SIZE);
    }

    #[test]
    fn split_pane_rects_tile_the_content_area() {
        // Even widths split exactly in half; odd widths give the
        // extra pixel to the right pane; the halves never gap or
        // overlap, at any size.
        for (w, h) in [(1280.0, 684.0), (1279.0, 684.0), (800.0, 600.0), (1.0, 1.0)] {
            let content = Rectangle {
                x: 0.0,
                y: 48.0,
                width: w,
                height: h,
            };
            let (left, right) = split_pane_rects(content);
            assert_eq!(left.x, 0.0);
            assert_eq!(left.y, 48.0);
            assert_eq!(left.height, h);
            assert_eq!(right.height, h);
            assert_eq!(left.width + right.width, w);
            assert_eq!(right.x, left.x + left.width);
            assert_eq!(left.width, (w / 2.0).floor());
        }
    }

    #[test]
    fn pane_at_point_picks_the_half() {
        let content = Rectangle {
            x: 0.0,
            y: 48.0,
            width: 1280.0,
            height: 684.0,
        };
        // Single view: everything is pane 0.
        assert_eq!(pane_at_point(content, false, Point::new(1000.0, 100.0)), 0);
        // Split view: left half, the exact middle, and right half.
        assert_eq!(pane_at_point(content, true, Point::new(100.0, 100.0)), 0);
        assert_eq!(pane_at_point(content, true, Point::new(639.0, 100.0)), 0);
        assert_eq!(pane_at_point(content, true, Point::new(640.0, 100.0)), 1);
        assert_eq!(pane_at_point(content, true, Point::new(1200.0, 100.0)), 1);
    }

    #[test]
    fn pane_viewport_size_subtracts_the_border_when_split() {
        let rect = Rectangle {
            x: 0.0,
            y: 48.0,
            width: 640.0,
            height: 684.0,
        };
        // Single view: the webview IS the content area.
        assert_eq!(pane_viewport_size(rect, false), (640, 684));
        // Split view: 2 px border + 2 px padding on each side.
        assert_eq!(
            pane_viewport_size(rect, true),
            (
                (640.0 - 2.0 * PANE_BORDER) as u32,
                (684.0 - 2.0 * PANE_BORDER) as u32
            )
        );
        // Degenerate rects never go to zero.
        let tiny = Rectangle {
            x: 0.0,
            y: 0.0,
            width: 2.0,
            height: 2.0,
        };
        assert_eq!(pane_viewport_size(tiny, true), (1, 1));
        assert_eq!(pane_viewport_size(tiny, false), (2, 2));
    }

    #[test]
    fn move_selection_wraps_both_ends() {
        assert_eq!(move_selection(0, 5, -1), 4);
        assert_eq!(move_selection(4, 5, 1), 0);
        assert_eq!(move_selection(2, 5, 1), 3);
        assert_eq!(move_selection(2, 5, -1), 1);
        assert_eq!(move_selection(0, 0, 1), 0);
        assert_eq!(move_selection(0, 1, 1), 0);
    }

    #[test]
    fn palette_panel_is_centered_and_never_overflows() {
        // 560 px panel, centered.
        assert_eq!(palette_panel_geometry(1280.0), (360.0, 560.0));
        assert_eq!(palette_panel_geometry(800.0), (120.0, 560.0));
        for width in [592.0, 400.0, 100.0, 20.0] {
            let (x, w) = palette_panel_geometry(width);
            assert!(x >= 0.0, "x {x} at window width {width}");
            assert!(w >= 1.0, "width {w} at window width {width}");
            assert!(
                x + w <= width,
                "panel ({x} + {w}) runs off-screen at window width {width}"
            );
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
    // Round 10: pick the titlebar mode once, before the app
    // starts — GNOME falls back to Frost's own titlebar
    // (Mutter refuses server-side decorations); everything
    // else lets the window manager draw it. The same flag goes
    // to libcosmic (client_decorations) and to the app as its
    // Flags, so the view agrees with the window.
    let client_decorations = use_client_decorations(&detected_desktop());
    let settings = Settings::default()
        .size(Size::new(INITIAL_WINDOW.0, INITIAL_WINDOW.1))
        .client_decorations(client_decorations);
    cosmic::app::run::<Frost>(settings, client_decorations)?;
    Ok(())
}