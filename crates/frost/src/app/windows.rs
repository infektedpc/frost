//! Native libcosmic windows for Frost's menu pages (Round 7).
//!
//! Round 3–6 served Settings / Print / Open / Inspector / Exit (and
//! Extensions / Markdown) as `frost://` pages from the app's own
//! loopback server (`server.rs`). Round 7 deletes that server entirely:
//! each menu item now pops up its own libcosmic window, drawn with
//! iced widgets, in the same process.
//!
//! Windows are single-instance per kind: opening one that is already
//! open just focuses it. Each window is opened with
//! `exit_on_close_request: false`, so closing a popup never quits the
//! browser; the OS close button is handled like any other close.
//!
//! Every popup MUST be opened with `transparent: true`, even though
//! its content paints fully opaque: iced's wgpu compositor picks one
//! alpha mode at startup from the MAIN window's surface (PreMultiplied
//! — libcosmic creates the main window transparent, so its surface is
//! a 32-bit visual) and applies that same mode to every window's
//! surface. A popup opened opaque gets a 24-bit visual whose surface
//! supports only [Opaque, Inherit], and `surface.configure` with
//! PreMultiplied panics fatally, killing the whole browser (found
//! 2026-11-15: the first Settings-window open crashed Frost this
//! way). `transparent: true` gives the popup the same 32-bit visual
//! as the main window, so the configure succeeds; the painted content
//! is opaque, so the window looks identical.
//!
//! This module is a child of the `app` module (`app.rs`), so it sees
//! `Frost`, `Message`, and the chrome styling helpers directly.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use cosmic::app::Task;
use cosmic::iced::widget::image::{Handle, Image};
use cosmic::iced::widget::{scrollable, text};
use cosmic::iced::{window, ContentFit, Length, Size};
use cosmic::widget::{button, container, text_input};
use cosmic::{iced, ApplicationExt, Element};

use crate::file_browser::{self, DirEntry, MdBlock};
use crate::settings::{
    is_loadable_homepage, SearchEngine, Settings as FrostSettings, ThemeChoice,
};
use frost_inspector::{DomNode, DomSnapshot};

use super::{chrome_container_style, Frost, Message};

/// Which popup a window is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PopupKind {
    Settings,
    Print,
    Open,
    Inspector,
    Exit,
}

/// The Settings window's state: a draft of the settings, edited in
/// the window and applied on Save. `note` reports the last save/reset
/// result (or a validation error) in the window itself.
pub struct SettingsWindow {
    pub id: window::Id,
    pub draft_theme: ThemeChoice,
    pub draft_homepage: String,
    pub draft_search: SearchEngine,
    pub draft_confirm: bool,
    pub note: String,
    pub show_extensions: bool,
}

/// The Print window's state: the captured preview (PNG bytes for
/// saving, plus an iced handle for drawing), frozen at print-open
/// time so a later Save cannot re-describe the printed page.
pub struct PrintWindow {
    pub id: window::Id,
    pub png: Vec<u8>,
    pub preview: Handle,
    pub label: String,
    pub page_title: String,
    pub page_url: String,
    pub saved_path: Option<String>,
    pub error: Option<String>,
}

/// The Markdown view inside the Open window: a `.md` file rendered as
/// styled native text, with links back to its folder and the raw file.
pub struct MarkdownView {
    pub path: PathBuf,
    pub blocks: Vec<MdBlock>,
}

/// The Open window's state: the directory being listed (or an error),
/// plus an optional Markdown view replacing the listing.
pub struct OpenWindow {
    pub id: window::Id,
    pub path: PathBuf,
    pub entries: Vec<DirEntry>,
    pub error: Option<String>,
    pub markdown: Option<MarkdownView>,
}

/// The Inspector window's state: the Round 5 snapshot (URL, capture
/// time, node count, tree), which nodes are collapsed, and which node
/// is selected for the details pane.
pub struct InspectorWindow {
    pub id: window::Id,
    pub snapshot: DomSnapshot,
    pub collapsed: HashSet<u64>,
    pub selected: Option<u64>,
}

/// Messages from the popup windows. Each kind is single-instance, so
/// no window id travels with the message — the kind implies it.
#[derive(Debug, Clone)]
pub enum PopupMessage {
    SettingsTheme(ThemeChoice),
    SettingsHomepage(String),
    SettingsSearch(SearchEngine),
    SettingsConfirm(bool),
    SettingsSave,
    SettingsReset,
    SettingsToggleExtensions,
    PrintSave,
    PrintClose,
    OpenNavigate(PathBuf),
    OpenFile(PathBuf),
    OpenMarkdown(PathBuf),
    OpenMarkdownBack,
    OpenClose,
    InspectorSelect(u64),
    InspectorToggle(u64),
    InspectorRecapture,
    InspectorBack,
    ExitConfirm,
    ExitCancel,
}

/// The user's Downloads directory (`~/Downloads`, or a relative
/// `Downloads` when HOME is unset).
pub fn downloads_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("Downloads"))
        .unwrap_or_else(|| PathBuf::from("Downloads"))
}

/// Save captured print-preview PNG bytes to `dir` (creating it first,
/// so the save works even when ~/Downloads does not exist yet).
/// Returns the path written on success. Pure over the bytes plus the
/// directory, so tests can drive it with a temp dir.
pub fn save_print_preview(png: &[u8], dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("frost-print-{secs}.png"));
    std::fs::write(&path, png)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

/// Format a Unix-milliseconds timestamp as `YYYY-MM-DD HH:MM:SS UTC`
/// (civil-from-days; no date library in the tree, and UTC is stated
/// rather than guessed as local time).
pub fn format_capture_time(unix_ms: u64) -> String {
    let secs = unix_ms / 1000;
    let days = (secs / 86400) as i64;
    let tod = secs % 86400;
    let (hour, min, sec) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    // Howard Hinnant's civil-from-days.
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02} UTC")
}

/// The one-line label for a DOM node in the tree (same shape as the
/// old inspector page's JavaScript label): `tag#id.class…`, or the
/// quoted text for `#text` nodes.
pub fn node_label(node: &DomNode) -> String {
    if node.tag == "#text" {
        return format!("\"{}\"", node.text.clone().unwrap_or_default());
    }
    let mut label = node.tag.clone();
    if let Some(id) = node.attributes.get("id") {
        if !id.is_empty() {
            label.push('#');
            label.push_str(id);
        }
    }
    if let Some(class) = node.attributes.get("class") {
        for cls in class.split_whitespace() {
            label.push('.');
            label.push_str(cls);
        }
    }
    label
}

/// Flatten the visible part of the tree into rows: (depth, node, has
/// children). Collapsed subtrees contribute only their root row.
fn visible_rows<'a>(
    node: &'a DomNode,
    depth: usize,
    collapsed: &HashSet<u64>,
    out: &mut Vec<(usize, &'a DomNode, bool)>,
) {
    let has_children = !node.children.is_empty();
    out.push((depth, node, has_children));
    if has_children && !collapsed.contains(&node.node_id) {
        for child in &node.children {
            visible_rows(child, depth + 1, collapsed, out);
        }
    }
}

impl Frost {
    /// The window id of the open popup of this kind, if any.
    pub fn popup_id(&self, kind: PopupKind) -> Option<window::Id> {
        match kind {
            PopupKind::Settings => self.settings_win.as_ref().map(|w| w.id),
            PopupKind::Print => self.print_win.as_ref().map(|w| w.id),
            PopupKind::Open => self.open_win.as_ref().map(|w| w.id),
            PopupKind::Inspector => self.inspector_win.as_ref().map(|w| w.id),
            PopupKind::Exit => self.exit_win,
        }
    }

    /// Which popup (if any) a window id belongs to.
    pub fn popup_kind(&self, id: window::Id) -> Option<PopupKind> {
        if self.popup_id(PopupKind::Settings) == Some(id) {
            Some(PopupKind::Settings)
        } else if self.popup_id(PopupKind::Print) == Some(id) {
            Some(PopupKind::Print)
        } else if self.popup_id(PopupKind::Open) == Some(id) {
            Some(PopupKind::Open)
        } else if self.popup_id(PopupKind::Inspector) == Some(id) {
            Some(PopupKind::Inspector)
        } else if self.popup_id(PopupKind::Exit) == Some(id) {
            Some(PopupKind::Exit)
        } else {
            None
        }
    }

    /// Open the Settings window (draft initialized from the live
    /// settings). Re-opens are a focus, not a second window.
    pub fn open_settings_window(&mut self) -> Task<Message> {
        if let Some(id) = self.popup_id(PopupKind::Settings) {
            return window::gain_focus(id);
        }
        let (id, spawn) = window::open(window::Settings {
            size: Size::new(640.0, 620.0),
            exit_on_close_request: false,
            transparent: true,
            position: window::Position::Centered,
            ..Default::default()
        });
        self.settings_win = Some(SettingsWindow {
            id,
            draft_theme: self.settings.theme,
            draft_homepage: self.settings.homepage.clone(),
            draft_search: self.settings.search_engine,
            draft_confirm: self.settings.confirm_exit,
            note: String::new(),
            show_extensions: false,
        });
        Task::batch([
            spawn.map(|id| cosmic::Action::App(Message::WindowOpened(id))),
            self.set_window_title("Frost Settings".to_string(), id),
        ])
    }

    /// Open the Print window on a captured preview (frozen at
    /// print-open time: label, page title, page URL). A failed
    /// capture opens the window on the error instead — the preview
    /// image is skipped and the error is shown.
    pub fn open_print_window(
        &mut self,
        result: Result<Vec<u8>, String>,
        label: String,
        page_title: String,
        page_url: String,
    ) -> Task<Message> {
        if let Some(id) = self.popup_id(PopupKind::Print) {
            return window::gain_focus(id);
        }
        let (id, spawn) = window::open(window::Settings {
            size: Size::new(960.0, 720.0),
            exit_on_close_request: false,
            transparent: true,
            position: window::Position::Centered,
            ..Default::default()
        });
        let (png, preview, error) = match result {
            Ok(png) => {
                let preview = Handle::from_bytes(png.clone());
                (png, preview, None)
            },
            Err(e) => (Vec::new(), Handle::from_bytes(Vec::<u8>::new()), Some(e)),
        };
        self.print_win = Some(PrintWindow {
            id,
            png,
            preview,
            label,
            page_title,
            page_url,
            saved_path: None,
            error,
        });
        Task::batch([
            spawn.map(|id| cosmic::Action::App(Message::WindowOpened(id))),
            self.set_window_title("Print page".to_string(), id),
        ])
    }

    /// Open the file-browser window on `path` (or focus it, if open).
    /// A fresh listing is read for the path.
    pub fn open_file_window(&mut self, path: &Path) -> Task<Message> {
        if let Some(id) = self.popup_id(PopupKind::Open) {
            return window::gain_focus(id);
        }
        let (id, spawn) = window::open(window::Settings {
            size: Size::new(820.0, 620.0),
            exit_on_close_request: false,
            transparent: true,
            position: window::Position::Centered,
            ..Default::default()
        });
        let (entries, error) = match file_browser::list_dir(path) {
            Ok(entries) => (entries, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        self.open_win = Some(OpenWindow {
            id,
            path: path.to_path_buf(),
            entries,
            error,
            markdown: None,
        });
        Task::batch([
            spawn.map(|id| cosmic::Action::App(Message::WindowOpened(id))),
            self.set_window_title("Open file".to_string(), id),
        ])
    }

    /// Open the Inspector window on a snapshot (or focus it, if open).
    pub fn open_inspector_window(
        &mut self,
        snapshot: DomSnapshot,
    ) -> Task<Message> {
        if let Some(id) = self.popup_id(PopupKind::Inspector) {
            return window::gain_focus(id);
        }
        let (id, spawn) = window::open(window::Settings {
            size: Size::new(1120.0, 720.0),
            exit_on_close_request: false,
            transparent: true,
            position: window::Position::Centered,
            ..Default::default()
        });
        self.inspector_win = Some(InspectorWindow {
            id,
            snapshot,
            collapsed: HashSet::new(),
            selected: None,
        });
        Task::batch([
            spawn.map(|id| cosmic::Action::App(Message::WindowOpened(id))),
            self.set_window_title("Inspector".to_string(), id),
        ])
    }

    /// Open the Exit confirmation dialog (or focus it, if open).
    pub fn open_exit_window(&mut self) -> Task<Message> {
        if let Some(id) = self.popup_id(PopupKind::Exit) {
            return window::gain_focus(id);
        }
        let (id, spawn) = window::open(window::Settings {
            size: Size::new(420.0, 200.0),
            exit_on_close_request: false,
            transparent: true,
            position: window::Position::Centered,
            ..Default::default()
        });
        self.exit_win = Some(id);
        Task::batch([
            spawn.map(|id| cosmic::Action::App(Message::WindowOpened(id))),
            self.set_window_title("Quit Frost?".to_string(), id),
        ])
    }

    /// Close a popup: drop its state and close its window. A no-op
    /// when no popup of that kind is open.
    pub fn close_popup(&mut self, kind: PopupKind) -> Task<Message> {
        let Some(id) = self.popup_id(kind) else {
            return Task::none();
        };
        self.forget_window(id);
        window::close(id)
    }

    /// Drop a popup's state for a window id (no window command — used
    /// when the window is already gone).
    pub fn forget_window(&mut self, id: window::Id) {
        match self.popup_kind(id) {
            Some(PopupKind::Settings) => self.settings_win = None,
            Some(PopupKind::Print) => self.print_win = None,
            Some(PopupKind::Open) => self.open_win = None,
            Some(PopupKind::Inspector) => self.inspector_win = None,
            Some(PopupKind::Exit) => self.exit_win = None,
            None => {},
        }
    }

    /// Handle a close request for a window id: popups close (state
    /// dropped); the main window closes the app, as before.
    pub fn close_window_by_id(&mut self, id: window::Id) -> Task<Message> {
        if self.popup_kind(id).is_some() {
            self.forget_window(id);
        }
        window::close(id)
    }

    /// Replace the open Inspector window's snapshot in place (a
    /// re-capture): tree state resets, since node ids are stable
    /// within one snapshot only.
    pub fn replace_inspector_snapshot(&mut self, snapshot: DomSnapshot) {
        if let Some(w) = &mut self.inspector_win {
            w.snapshot = snapshot;
            w.collapsed = HashSet::new();
            w.selected = None;
        }
    }

    /// Load a URL in the main webview with the same in-flight
    /// ceremony as the address bar (Round 4, F-008: mark the load now
    /// so Stop stays live if the first response never arrives).
    pub fn load_url_in_main(&mut self, url: &str) {
        match self.webview.load(url) {
            Ok(()) => {
                self.loading = true;
                self.awaiting_load = true;
                self.loading_since = Some(std::time::Instant::now());
                self.address = url.to_string();
                self.address_editing = false;
                self.content_focused = true;
                self.status = format!("Loading {url} …");
            },
            Err(e) => self.status = format!("Cannot load {url}: {e}"),
        }
    }
}

impl Frost {
    /// Handle a message from one of the popup windows.
    pub fn update_popup(&mut self, msg: PopupMessage) -> Task<Message> {
        match msg {
            PopupMessage::SettingsTheme(theme) => {
                if let Some(w) = &mut self.settings_win {
                    w.draft_theme = theme;
                }
                Task::none()
            },
            PopupMessage::SettingsHomepage(value) => {
                if let Some(w) = &mut self.settings_win {
                    w.draft_homepage = value;
                }
                Task::none()
            },
            PopupMessage::SettingsSearch(engine) => {
                if let Some(w) = &mut self.settings_win {
                    w.draft_search = engine;
                }
                Task::none()
            },
            PopupMessage::SettingsConfirm(value) => {
                if let Some(w) = &mut self.settings_win {
                    w.draft_confirm = value;
                }
                Task::none()
            },
            PopupMessage::SettingsSave => {
                // Take the draft out of the window first: applying it
                // touches self.settings (a separate field), and the
                // borrow checker will not let the window borrow live
                // across that.
                let Some(draft) = self.settings_win.as_ref().map(|w| {
                    (
                        w.draft_theme,
                        w.draft_homepage.trim().to_string(),
                        w.draft_search,
                        w.draft_confirm,
                    )
                }) else {
                    return Task::none();
                };
                let (theme, homepage, search, confirm) = draft;
                let mut new = self.settings.clone();
                new.theme = theme;
                new.homepage = homepage;
                new.search_engine = search;
                new.confirm_exit = confirm;
                let note = if !is_loadable_homepage(&new.homepage) {
                    "That homepage is not a valid http(s) URL — nothing was changed."
                        .to_string()
                } else {
                    let theme_changed = new.theme != self.settings.theme;
                    match new.save_to(&FrostSettings::config_path()) {
                        Ok(()) => {
                            self.settings = new;
                            if theme_changed {
                                self.webview.set_theme(self.settings.theme.is_dark());
                            }
                            "Settings saved.".to_string()
                        },
                        Err(e) => format!("Could not save settings: {e}"),
                    }
                };
                if let Some(w) = &mut self.settings_win {
                    w.note = note;
                }
                Task::none()
            },
            PopupMessage::SettingsReset => {
                let defaults = FrostSettings::default();
                let theme_changed = defaults.theme != self.settings.theme;
                let note = match defaults.save_to(&FrostSettings::config_path()) {
                    Ok(()) => {
                        self.settings = defaults;
                        if theme_changed {
                            self.webview.set_theme(self.settings.theme.is_dark());
                        }
                        "Settings reset to defaults.".to_string()
                    },
                    Err(e) => format!("Could not save settings: {e}"),
                };
                if let Some(w) = &mut self.settings_win {
                    w.draft_theme = self.settings.theme;
                    w.draft_homepage = self.settings.homepage.clone();
                    w.draft_search = self.settings.search_engine;
                    w.draft_confirm = self.settings.confirm_exit;
                    w.note = note;
                }
                Task::none()
            },
            PopupMessage::SettingsToggleExtensions => {
                if let Some(w) = &mut self.settings_win {
                    w.show_extensions = !w.show_extensions;
                }
                Task::none()
            },
            PopupMessage::PrintSave => {
                let Some(result) = self.print_win.as_ref().map(|w| {
                    save_print_preview(&w.png, &downloads_dir())
                }) else {
                    return Task::none();
                };
                if let Some(w) = &mut self.print_win {
                    match result {
                        Ok(path) => {
                            w.saved_path = Some(path.display().to_string());
                            w.error = None;
                        },
                        Err(e) => w.error = Some(e),
                    }
                }
                Task::none()
            },
            PopupMessage::PrintClose => self.close_popup(PopupKind::Print),
            PopupMessage::OpenNavigate(path) => {
                let (entries, error) = match file_browser::list_dir(&path) {
                    Ok(entries) => (entries, None),
                    Err(e) => (Vec::new(), Some(e)),
                };
                if let Some(w) = &mut self.open_win {
                    w.path = path;
                    w.entries = entries;
                    w.error = error;
                    w.markdown = None;
                }
                Task::none()
            },
            PopupMessage::OpenFile(path) => {
                let url = match url::Url::from_file_path(&path) {
                    Ok(u) => u.to_string(),
                    Err(()) => {
                        self.status =
                            format!("Cannot open {}: not a loadable file path", path.display());
                        return Task::none();
                    },
                };
                self.load_url_in_main(&url);
                self.close_popup(PopupKind::Open)
            },
            PopupMessage::OpenMarkdown(path) => {
                let view = file_browser::read_markdown(&path).map(|text| {
                    MarkdownView {
                        path,
                        blocks: file_browser::markdown_blocks(&text),
                    }
                });
                if let Some(w) = &mut self.open_win {
                    w.markdown = view;
                }
                Task::none()
            },
            PopupMessage::OpenMarkdownBack => {
                if let Some(w) = &mut self.open_win {
                    w.markdown = None;
                }
                Task::none()
            },
            PopupMessage::OpenClose => self.close_popup(PopupKind::Open),
            PopupMessage::InspectorSelect(id) => {
                if let Some(w) = &mut self.inspector_win {
                    w.selected = Some(id);
                }
                Task::none()
            },
            PopupMessage::InspectorToggle(id) => {
                if let Some(w) = &mut self.inspector_win {
                    if !w.collapsed.remove(&id) {
                        w.collapsed.insert(id);
                    }
                }
                Task::none()
            },
            PopupMessage::InspectorRecapture => {
                // Re-run the walker on the current page; the snapshot
                // lands in the open window (see the Tick's inspector
                // channel poll). The page never left the main window,
                // so no navigation is needed.
                self.inspector_recapture = true;
                self.capture_inspector();
                Task::none()
            },
            PopupMessage::InspectorBack => self.close_popup(PopupKind::Inspector),
            PopupMessage::ExitConfirm => {
                std::process::exit(0);
            },
            PopupMessage::ExitCancel => self.close_popup(PopupKind::Exit),
        }
    }

    /// The view for a popup window id (the main window uses `view()`).
    /// An unknown id — e.g. a window that closed mid-frame — gets an
    /// empty container rather than a panic.
    pub fn view_popup(&self, id: window::Id) -> Element<'_, Message> {
        match self.popup_kind(id) {
            Some(PopupKind::Settings) => {
                let Some(w) = &self.settings_win else {
                    return container(text("")).into();
                };
                settings_view(self, w)
            },
            Some(PopupKind::Print) => {
                let Some(w) = &self.print_win else {
                    return container(text("")).into();
                };
                print_view(self, w)
            },
            Some(PopupKind::Open) => {
                let Some(w) = &self.open_win else {
                    return container(text("")).into();
                };
                open_view(self, w)
            },
            Some(PopupKind::Inspector) => {
                let Some(w) = &self.inspector_win else {
                    return container(text("")).into();
                };
                inspector_view(self, w)
            },
            Some(PopupKind::Exit) => exit_view(self),
            None => container(text("")).into(),
        }
    }
}

/// A labeled section heading inside a popup window.
fn section(label: &str) -> Element<'_, Message> {
    container(text(label).size(15))
        .padding([12, 0, 2, 0])
        .into()
}

/// A muted one-line note inside a popup window.
fn muted(label: &str) -> Element<'_, Message> {
    container(text(label).size(13)).padding([2, 0, 2, 0]).into()
}

/// A choice button: `●` on the selected choice, `○` on the others.
fn choice_button(
    label: &str,
    selected: bool,
    msg: PopupMessage,
) -> Element<'_, Message> {
    let mark = if selected { "●" } else { "○" };
    let b = button::standard(format!("{mark} {label}"));
    if selected {
        b.into()
    } else {
        b.on_press(Message::Popup(msg)).into()
    }
}

/// The Settings window: theme, homepage, search engine, exit
/// confirmation — the same options as the old `frost://settings`
/// page — plus the Extensions section (Round 7: extensions moved here
/// from the deleted `frost://extensions` page).
fn settings_view<'a>(frost: &'a Frost, w: &'a SettingsWindow) -> Element<'a, Message> {
    let colors = frost.settings.theme.chrome_colors();
    let mut children: Vec<Element<'_, Message>> = vec![
        container(text("Settings").size(22))
            .padding([8, 0, 4, 0])
            .into(),
    ];
    if !w.note.is_empty() {
        children.push(
            container(text(&w.note).size(14))
                .padding([4, 0, 4, 0])
                .into(),
        );
    }
    children.push(section("Theme"));
    children.push(
        cosmic::widget::row::with_children(vec![
            choice_button(
                "Dark",
                w.draft_theme == ThemeChoice::Dark,
                PopupMessage::SettingsTheme(ThemeChoice::Dark),
            ),
            choice_button(
                "Light",
                w.draft_theme == ThemeChoice::Light,
                PopupMessage::SettingsTheme(ThemeChoice::Light),
            ),
        ])
        .spacing(8)
        .into(),
    );
    children.push(section("Homepage (opens on launch)"));
    children.push(
        text_input("Homepage", &w.draft_homepage)
            .on_input(|v| Message::Popup(PopupMessage::SettingsHomepage(v)))
            .width(Length::Fill)
            .into(),
    );
    children.push(section(
        "Default search engine (used when the address bar gets a search query)",
    ));
    children.push(
        cosmic::widget::row::with_children(
            SearchEngine::all()
                .iter()
                .map(|e| {
                    choice_button(
                        e.label(),
                        *e == w.draft_search,
                        PopupMessage::SettingsSearch(*e),
                    )
                })
                .collect::<Vec<Element<'_, Message>>>(),
        )
        .spacing(8)
        .into(),
    );
    children.push(section("Ask before quitting"));
    children.push(
        choice_button(
            if w.draft_confirm {
                "Ask before quitting: Yes"
            } else {
                "Ask before quitting: No"
            },
            false,
            PopupMessage::SettingsConfirm(!w.draft_confirm),
        ),
    );
    children.push(
        cosmic::widget::row::with_children(vec![
            button::standard("Save settings")
                .on_press(Message::Popup(PopupMessage::SettingsSave))
                .into(),
            button::standard("Restore defaults")
                .on_press(Message::Popup(PopupMessage::SettingsReset))
                .into(),
        ])
        .spacing(8)
        .padding([12, 0, 0, 0])
        .into(),
    );
    children.push(muted(
        "Theme and search apply when you save; the homepage applies on next launch.",
    ));
    children.push(section("Extensions"));
    let ext_label = if w.show_extensions {
        "Hide extensions"
    } else {
        "Show extensions"
    };
    children.push(
        button::standard(ext_label)
            .on_press(Message::Popup(PopupMessage::SettingsToggleExtensions))
            .into(),
    );
    if w.show_extensions {
        if frost.extensions.is_empty() {
            children.push(muted(
                "No extensions loaded. Put an extension directory (manifest.json + content script) under ~/.config/frost/extensions/ or set FROST_EXTENSIONS_DIR, then restart Frost.",
            ));
        } else {
            for (name, version, enabled) in &frost.extensions {
                children.push(
                    container(
                        text(format!(
                            "{name} — {version} — {}",
                            if *enabled { "enabled" } else { "disabled" }
                        ))
                        .size(14),
                    )
                    .padding([2, 0, 2, 0])
                    .into(),
                );
            }
        }
        if !frost.failed_extensions.is_empty() {
            children.push(section("Skipped (bad manifest)"));
            for (dir, error) in &frost.failed_extensions {
                children.push(
                    container(text(format!("{dir} — {error}")).size(13))
                        .padding([2, 0, 2, 0])
                        .into(),
                );
            }
        }
    }
    // Round 7: the Settings content scrolls if the extensions list
    // ever outgrows the 620 px window (the scrollable is the original
    // design; an earlier "hit-test offset" here turned out to be a
    // harness misread of the button positions, not a real bug — the
    // buttons hit-test exactly where they render).
    container(
        scrollable(
            cosmic::widget::column::with_children(children)
                .spacing(4)
                .padding([8, 16, 16, 16]),
        )
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .style(move |_| chrome_container_style(colors))
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// The Print window: preview image, Save as PNG, and the honest
/// no-print-pipeline note — the same content as the old
/// `frost://print` page.
fn print_view<'a>(frost: &'a Frost, w: &'a PrintWindow) -> Element<'a, Message> {
    let colors = frost.settings.theme.chrome_colors();
    let title = if w.page_title.is_empty() {
        "Untitled page".to_string()
    } else {
        w.page_title.clone()
    };
    let mut children: Vec<Element<'_, Message>> = vec![
        container(text("Print page").size(22))
            .padding([8, 0, 4, 0])
            .into(),
        container(text(format!("Page: {title}")).size(15))
            .padding([2, 0, 2, 0])
            .into(),
        muted(&w.page_url),
        muted(&w.label),
    ];
    if let Some(error) = &w.error {
        children.push(
            container(text(format!("Preview failed: {error}")).size(14))
                .padding([8, 0, 4, 0])
                .into(),
        );
    } else {
        children.push(
            container(
                Image::new(w.preview.clone())
                    .width(Length::Fill)
                    .content_fit(ContentFit::Contain),
            )
            .padding([8, 0, 8, 0])
            .into(),
        );
        children.push(
            cosmic::widget::row::with_children(vec![
                button::standard("Save as PNG")
                    .on_press(Message::Popup(PopupMessage::PrintSave))
                    .into(),
                button::standard("Close")
                    .on_press(Message::Popup(PopupMessage::PrintClose))
                    .into(),
            ])
            .spacing(8)
            .into(),
        );
        if let Some(path) = &w.saved_path {
            children.push(
                container(text(format!("Saved to {path}")).size(14))
                    .padding([4, 0, 4, 0])
                    .into(),
            );
        }
        children.push(muted("(writes the preview to ~/Downloads)"));
    }
    children.push(muted(
        "This is a screenshot of the page as it appears on screen, not a paginated print layout — Servo 0.8.0 has no print pipeline, so there is no print-to-PDF or printer output. Save the PNG and print it from any image viewer.",
    ));
    container(
        scrollable(
            cosmic::widget::column::with_children(children)
                .spacing(4)
                .padding([8, 16, 16, 16]),
        )
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .style(move |_| chrome_container_style(colors))
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// One Markdown block as native widgets.
fn markdown_block_view(block: &MdBlock) -> Element<'_, Message> {
    match block {
        MdBlock::Heading { level, text: heading } => {
            let size = match level {
                1 => 22,
                2 => 19,
                3 => 17,
                _ => 15,
            };
            container(text(heading).size(size))
                .padding([10, 0, 2, 0])
                .into()
        },
        MdBlock::Paragraph(s) => container(text(s).size(14))
            .padding([2, 0, 2, 0])
            .into(),
        MdBlock::Code(s) => container(text(s).size(13).font(iced::Font::MONOSPACE))
            .padding(8)
            .into(),
        MdBlock::List(items) => {
            let rows: Vec<Element<'_, Message>> = items
                .iter()
                .map(|item| {
                    container(text(format!("• {item}")).size(14))
                        .padding([1, 0, 1, 12])
                        .into()
                })
                .collect();
            cosmic::widget::column::with_children(rows).into()
        },
        MdBlock::Quote(s) => container(text(s).size(14))
            .padding([2, 0, 2, 16])
            .into(),
        MdBlock::Rule => container(text("─".repeat(40)).size(13))
            .padding([6, 0, 6, 0])
            .into(),
    }
}

/// The Open window: a native directory listing (or the Markdown view
/// for a `.md` file) — the same listing as the old `frost://open`
/// page, without the server.
fn open_view<'a>(frost: &'a Frost, w: &'a OpenWindow) -> Element<'a, Message> {
    let colors = frost.settings.theme.chrome_colors();
    let mut children: Vec<Element<'_, Message>> = vec![];
    if let Some(md) = &w.markdown {
        let name = md
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file.md".to_string());
        children.push(
            container(text(format!("Markdown — {name}")).size(22))
                .padding([8, 0, 4, 0])
                .into(),
        );
        let links: Vec<Element<'_, Message>> = vec![
            button::standard("← Back to folder")
                .on_press(Message::Popup(PopupMessage::OpenMarkdownBack))
                .into(),
            // Raw file: load the file itself in the main window.
            button::standard("Raw file")
                .on_press(Message::Popup(PopupMessage::OpenFile(
                    md.path.clone(),
                )))
                .into(),
        ];
        children.push(
            cosmic::widget::row::with_children(links)
                .spacing(8)
                .into(),
        );
        for block in &md.blocks {
            children.push(markdown_block_view(block));
        }
    } else {
        children.push(
            container(text(format!("Files — {}", w.path.display())).size(22))
                .padding([8, 0, 4, 0])
                .into(),
        );
        if let Some(parent) = w.path.parent() {
            let parent = parent.to_path_buf();
            children.push(
                button::text(format!("↑ Up to {}", parent.display()))
                    .on_press(Message::Popup(PopupMessage::OpenNavigate(parent)))
                    .into(),
            );
        }
        if let Some(error) = &w.error {
            children.push(
                container(text(error).size(14))
                    .padding([8, 0, 4, 0])
                    .into(),
            );
        } else if w.entries.is_empty() {
            children.push(muted("(Empty folder.)"));
        } else {
            for entry in &w.entries {
                let path = w.path.join(&entry.name);
                let label = if entry.is_dir {
                    format!("[dir] {}/", entry.name)
                } else {
                    format!(
                        "{} ({})",
                        entry.name,
                        file_browser::human_size(entry.size)
                    )
                };
                let msg = if entry.is_dir {
                    PopupMessage::OpenNavigate(path)
                } else if entry.name.ends_with(".md") {
                    PopupMessage::OpenMarkdown(path)
                } else {
                    PopupMessage::OpenFile(path)
                };
                children.push(
                    button::text(label)
                        .on_press(Message::Popup(msg))
                        .width(Length::Fill)
                        .into(),
                );
            }
        }
    }
    children.push(
        container(
            button::standard("Close")
                .on_press(Message::Popup(PopupMessage::OpenClose)),
        )
        .padding([12, 0, 0, 0])
        .into(),
    );
    container(
        scrollable(
            cosmic::widget::column::with_children(children)
                .spacing(4)
                .padding([8, 16, 16, 16]),
        )
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .style(move |_| chrome_container_style(colors))
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// The Inspector window: the snapshot header, Back / Re-capture, the
/// tree on the left and the details pane on the right — the same
/// snapshot model as the old `frost://inspector` page (Round 5).
fn inspector_view<'a>(frost: &'a Frost, w: &'a InspectorWindow) -> Element<'a, Message> {
    let colors = frost.settings.theme.chrome_colors();
    let snap = &w.snapshot;
    let mut rows: Vec<(usize, &DomNode, bool)> = Vec::new();
    visible_rows(&snap.root, 0, &w.collapsed, &mut rows);
    let mut tree_children: Vec<Element<'_, Message>> = Vec::new();
    for (depth, node, has_children) in &rows {
        let indent = (*depth as f32) * 22.0;
        let toggle = if *has_children {
            if w.collapsed.contains(&node.node_id) {
                "+"
            } else {
                "−"
            }
        } else {
            " "
        };
        let node_id = node.node_id;
        let toggle_btn = button::text(toggle)
            .on_press(Message::Popup(PopupMessage::InspectorToggle(node_id)))
            .width(Length::Fixed(20.0));
        // The selected row renders as a standard button; the rest as
        // text buttons, so the selection is visible at a glance.
        let label = node_label(node);
        let label_btn = if w.selected == Some(node_id) {
            button::standard(label)
                .on_press(Message::Popup(PopupMessage::InspectorSelect(node_id)))
        } else {
            button::text(label)
                .on_press(Message::Popup(PopupMessage::InspectorSelect(node_id)))
        };
        tree_children.push(
            cosmic::widget::row::with_children(vec![
                container(text("")).width(Length::Fixed(indent)).into(),
                toggle_btn.into(),
                label_btn.into(),
            ])
            .spacing(2)
            .into(),
        );
    }
    let details: Element<'_, Message> = match w.selected.and_then(|id| snap.find(id)) {
        Some(node) => inspector_details(node),
        None => muted("Click a node to see its details."),
    };
    let header = format!(
        "Snapshot of {} — Captured {} · {} nodes{} — this is a snapshot, not a live view. The page may have changed since.",
        snap.url,
        format_capture_time(snap.captured_at_unix_ms),
        snap.node_count,
        if snap.truncated {
            " · truncated — the page is bigger than the walker limit, so the deepest nodes are missing"
        } else {
            ""
        },
    );
    container(
        cosmic::widget::column::with_children(vec![
            container(text("Inspector").size(22))
                .padding([8, 0, 4, 0])
                .into(),
            container(text(header).size(13))
                .padding([2, 0, 6, 0])
                .into(),
            cosmic::widget::row::with_children(vec![
                button::standard("Back to the page")
                    .on_press(Message::Popup(PopupMessage::InspectorBack))
                    .into(),
                button::standard("Re-capture")
                    .on_press(Message::Popup(PopupMessage::InspectorRecapture))
                    .into(),
            ])
            .spacing(8)
            .padding([0, 0, 8, 0])
            .into(),
            cosmic::widget::row::with_children(vec![
                container(
                    scrollable(
                        cosmic::widget::column::with_children(tree_children)
                            .spacing(1)
                            .padding([4, 8, 8, 4]),
                    )
                    .width(Length::Fill)
                    .height(Length::Fill),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
                container(
                    scrollable(details)
                        .width(Length::Fixed(340.0))
                        .height(Length::Fill),
                )
                .width(Length::Fixed(340.0))
                .height(Length::Fill)
                .padding([4, 4, 4, 12])
                .into(),
            ])
            .spacing(12)
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
        ])
        .spacing(2)
        .padding([8, 16, 16, 16]),
    )
    .style(move |_| chrome_container_style(colors))
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// The Inspector details pane for one node: tag, attributes, text,
/// computed style (the handful of properties people read first), and
/// the viewport-relative box.
fn inspector_details(node: &DomNode) -> Element<'_, Message> {
    let mut children: Vec<Element<'_, Message>> = vec![
        container(text(&node.tag).size(18))
            .padding([4, 0, 4, 0])
            .into(),
    ];
    if node.attributes.is_empty() {
        children.push(muted("No attributes."));
    } else {
        children.push(section("Attributes"));
        let mut keys: Vec<&String> = node.attributes.keys().collect();
        keys.sort();
        for key in keys {
            let value = node.attributes.get(key).cloned().unwrap_or_default();
            children.push(
                container(text(format!("{key}: {value}")).size(13))
                    .padding([1, 0, 1, 0])
                    .into(),
            );
        }
    }
    if let Some(text_value) = &node.text {
        if !text_value.trim().is_empty() {
            children.push(section("Text"));
            children.push(
                container(text(text_value).size(13))
                    .padding([1, 0, 1, 0])
                    .into(),
            );
        }
    }
    let cs = &node.computed_style;
    let shown: [(&str, &Option<String>); 9] = [
        ("color", &cs.color),
        ("background_color", &cs.background_color),
        ("display", &cs.display),
        ("position", &cs.position),
        ("font_family", &cs.font_family),
        ("font_size", &cs.font_size),
        ("font_weight", &cs.font_weight),
        ("width", &cs.width),
        ("height", &cs.height),
    ];
    let style_rows: Vec<(&str, &str)> = shown
        .iter()
        .filter_map(|(name, value)| value.as_deref().map(|v| (*name, v)))
        .collect();
    if !style_rows.is_empty() {
        children.push(section("Computed style"));
        for (name, value) in style_rows {
            children.push(
                container(text(format!("{name}: {value}")).size(13))
                    .padding([1, 0, 1, 0])
                    .into(),
            );
        }
    }
    if let Some(b) = &node.box_model {
        children.push(section("Box (viewport-relative)"));
        children.push(
            container(
                text(format!("x {} · y {} · {} × {}", b.x, b.y, b.width, b.height))
                    .size(13),
            )
            .padding([1, 0, 1, 0])
            .into(),
        );
        children.push(
            container(
                text(format!(
                    "margin {} {} {} {}\npadding {} {} {} {}\nborder {} {} {} {}",
                    b.margin[0], b.margin[1], b.margin[2], b.margin[3],
                    b.padding[0], b.padding[1], b.padding[2], b.padding[3],
                    b.border[0], b.border[1], b.border[2], b.border[3],
                ))
                .size(13),
            )
            .padding([1, 0, 1, 0])
            .into(),
        );
    }
    cosmic::widget::column::with_children(children)
        .spacing(2)
        .into()
}

/// The Exit confirmation dialog: Quit Frost, or Cancel back to the
/// page. Only shown when "Ask before quitting" is on; otherwise the
/// menu's Exit quits directly.
fn exit_view(frost: &Frost) -> Element<'_, Message> {
    let colors = frost.settings.theme.chrome_colors();
    container(
        cosmic::widget::column::with_children(vec![
            container(text("Quit Frost?").size(22))
                .padding([8, 0, 4, 0])
                .into(),
            muted("Your tabs are not saved yet — quitting closes the browser."),
            cosmic::widget::row::with_children(vec![
                button::standard("Quit Frost")
                    .on_press(Message::Popup(PopupMessage::ExitConfirm))
                    .into(),
                button::standard("Cancel")
                    .on_press(Message::Popup(PopupMessage::ExitCancel))
                    .into(),
            ])
            .spacing(8)
            .padding([12, 0, 0, 0])
            .into(),
        ])
        .spacing(4)
        .padding([8, 16, 16, 16]),
    )
    .style(move |_| chrome_container_style(colors))
    .width(Length::Fill)
    .height(Length::Fill)
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use frost_inspector::DomSnapshot;

    #[test]
    fn capture_time_formats_utc() {
        assert_eq!(format_capture_time(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(
            format_capture_time(1700000000000),
            "2023-11-14 22:13:20 UTC"
        );
        assert_eq!(
            format_capture_time(1794643768000),
            "2026-11-14 08:09:28 UTC"
        );
    }

    #[test]
    fn print_save_writes_png_and_creates_dir() {
        let dir = std::env::temp_dir().join("frost-print-save-test-r7");
        let _ = std::fs::remove_dir_all(&dir);
        let path = save_print_preview(&[0x89, 0x50, 0x4e, 0x47], &dir).unwrap();
        assert!(path.starts_with(&dir));
        assert!(path.exists());
        assert_eq!(std::fs::read(&path).unwrap(), vec![0x89, 0x50, 0x4e, 0x47]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn node_labels_match_the_old_page() {
        let mut node = DomNode::new(7, "div");
        node.attributes
            .insert("id".to_string(), "main".to_string());
        node.attributes
            .insert("class".to_string(), "wrap lead".to_string());
        assert_eq!(node_label(&node), "div#main.wrap.lead");
        let mut text_node = DomNode::new(8, "#text");
        text_node.text = Some("hello".to_string());
        assert_eq!(node_label(&text_node), "\"hello\"");
    }

    #[test]
    fn visible_rows_hide_collapsed_subtrees() {
        let mut snap = DomSnapshot::empty("https://example.org/");
        // empty(): #document > html > head + body — collapse html.
        let mut collapsed = HashSet::new();
        collapsed.insert(1);
        let mut rows = Vec::new();
        visible_rows(&snap.root, 0, &collapsed, &mut rows);
        let ids: Vec<u64> = rows.iter().map(|(_, n, _)| n.node_id).collect();
        assert_eq!(ids, vec![0, 1], "{ids:?}");
        let mut rows = Vec::new();
        visible_rows(&snap.root, 0, &HashSet::new(), &mut rows);
        assert_eq!(rows.len(), snap.node_count);
        // Collapse the root itself: only the root row shows.
        snap.root.children.clear();
        let mut rows = Vec::new();
        visible_rows(&snap.root, 0, &HashSet::new(), &mut rows);
        assert_eq!(rows.len(), 1);
    }
}
