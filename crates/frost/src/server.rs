//! Internal-page loopback server (Round 3).
//!
//! Frost's internal pages live at `frost://` URLs in the address bar,
//! but Servo cannot load the `frost:` scheme, so the app runs a tiny
//! HTTP server on 127.0.0.1 (ephemeral port) and loads the pages from
//! it. The app translates `frost://X` to `http://127.0.0.1:PORT/X`
//! before loading and translates back for display, so the user only
//! ever sees `frost://`.
//!
//! The server owns no browser state itself: it reads/writes a shared
//! `Shared` struct (settings, current page, extensions) behind a
//! Mutex. The app polls the shared settings version each Tick and
//! applies changes (theme and search engine live, homepage on next
//! launch). The server never touches the engine — it only serves HTML.
//!
//! Same pattern as the servo-de embedder's loopback server: one
//! listener thread, one thread per connection, plain HTTP/1.1, no
//! dependencies.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::file_browser;
use crate::settings::{percent_decode_query, Settings, ThemeChoice};

/// Browser state the server may read or change. The app owns the
/// authoritative copies; the server only edits `settings` (through the
/// settings page) and reads everything else for display.
#[derive(Debug)]
pub struct Shared {
    /// The server's base URL (`http://127.0.0.1:PORT`), set by the app
    /// right after `InternalServer::start` returns. File-browser links
    /// are built from it.
    pub base_url: String,
    pub settings: Settings,
    /// Bumped on every server-side settings change; the app polls it.
    pub settings_version: u64,
    pub current_page_title: String,
    pub current_page_url: String,
    pub last_external_url: Option<String>,
    /// (name, version, enabled) for every loaded extension.
    pub extensions: Vec<(String, String, bool)>,
    /// (directory, error) for every extension directory that failed.
    pub failed_extensions: Vec<(String, String)>,
    /// Round 4, F-003: the print preview — PNG bytes of the current
    /// page's viewport, captured by the app before frost://print
    /// opens. Served as /print/preview.png.
    pub print_preview_png: Option<Vec<u8>>,
    /// Label for the preview (which page it is a preview of).
    pub print_preview_label: String,
    /// Round 5: the printed page's identity, frozen at print-open
    /// time (title + display URL). The print page renders from THESE,
    /// not from current_page_*: after Save the server 303s back to
    /// /print, which re-renders — and by then the live current page
    /// IS frost://print, so the header would describe the print page
    /// itself ("Page: Print page") instead of the printed page.
    pub print_page_title: String,
    pub print_page_url: String,
    /// The last print-preview capture error, if it failed.
    pub print_preview_error: Option<String>,
    /// Where the preview PNG was last saved (~/Downloads), if saved.
    pub print_saved_path: Option<String>,
    /// Round 5: the inspector snapshot — the walker JSON for the page
    /// the user inspected, plus the page it came from. Served verbatim
    /// as /inspector/snapshot.json; the /inspector page renders it.
    pub inspector_snapshot: Option<InspectorSnapshot>,
    /// Round 5: the inspector page asked for a re-capture (the
    /// /inspector/recapture link). The app polls this each Tick, clears
    /// it, and re-captures the inspected page once it loads again.
    pub inspector_recapture_requested: bool,
}

/// One captured inspector snapshot (Round 5).
#[derive(Debug, Clone)]
pub struct InspectorSnapshot {
    /// The inspected page's URL (the display form, e.g.
    /// `https://example.org/` or `frost://settings`).
    pub url: String,
    /// Unix milliseconds when the walker ran.
    pub captured_at_ms: u64,
    pub node_count: usize,
    pub truncated: bool,
    /// The walker JSON string, served verbatim at
    /// /inspector/snapshot.json.
    pub json: String,
}

impl Shared {
    pub fn new(settings: Settings) -> Self {
        Shared {
            base_url: String::new(),
            settings,
            settings_version: 0,
            current_page_title: String::new(),
            current_page_url: String::new(),
            last_external_url: None,
            extensions: Vec::new(),
            failed_extensions: Vec::new(),
            print_preview_png: None,
            print_preview_label: String::new(),
            print_page_title: String::new(),
            print_page_url: String::new(),
            print_preview_error: None,
            print_saved_path: None,
            inspector_snapshot: None,
            inspector_recapture_requested: false,
        }
    }
}

/// The running server: the listener lives on its own thread (detached),
/// so the server serves for the process lifetime.
pub struct InternalServer {
    pub base_url: String,
    pub shared: Arc<Mutex<Shared>>,
}

impl InternalServer {
    /// Bind 127.0.0.1 on an ephemeral port and start serving.
    /// `settings_path` is where the settings page persists changes.
    pub fn start(shared: Arc<Mutex<Shared>>, settings_path: PathBuf) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|e| format!("cannot bind the internal server: {e}"))?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("cannot read the internal server port: {e}"))?
            .port();
        let base_url = format!("http://127.0.0.1:{port}");
        let shared_for_thread = Arc::clone(&shared);
        std::thread::spawn(move || serve_loop(listener, shared_for_thread, settings_path));
        Ok(InternalServer { base_url, shared })
    }
}

fn serve_loop(listener: TcpListener, shared: Arc<Mutex<Shared>>, settings_path: PathBuf) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let shared = Arc::clone(&shared);
                let settings_path = settings_path.clone();
                std::thread::spawn(move || handle_connection(stream, &shared, &settings_path));
            },
            Err(_) => continue,
        }
    }
}

fn handle_connection(mut stream: TcpStream, shared: &Arc<Mutex<Shared>>, settings_path: &PathBuf) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 65536 {
                    break;
                }
            },
            Err(_) => return,
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let request_line = text.lines().next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let _method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    // F-003: the print preview is binary PNG, not HTML — serve it
    // here, before the HTML-only dispatcher. (The route never reaches
    // `handle_request`, which returns String bodies.)
    if parse_target(target).0 == "/print/preview.png" {
        let bytes = shared.lock().ok().and_then(|s| s.print_preview_png.clone());
        match bytes {
            Some(bytes) => {
                let response = format!(
                    "HTTP/1.1 200\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(&bytes);
            },
            None => {
                let body = "no print preview captured";
                let response = format!(
                    "HTTP/1.1 404\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            },
        }
        return;
    }
    // The inspector snapshot is JSON, not HTML — serve it here, before
    // the HTML-only dispatcher (same pattern as the print preview PNG).
    if parse_target(target).0 == "/inspector/snapshot.json" {
        let json = shared
            .lock()
            .ok()
            .and_then(|s| s.inspector_snapshot.as_ref().map(|snap| snap.json.clone()));
        match json {
            Some(json) => {
                let response = format!(
                    "HTTP/1.1 200\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    json.len(),
                    json
                );
                let _ = stream.write_all(response.as_bytes());
            },
            None => {
                let body = "{\"error\":\"no snapshot\"}";
                let response = format!(
                    "HTTP/1.1 404\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            },
        }
        return;
    }
    let (status, body, location) = {
        let mut shared = shared.lock().unwrap_or_else(|e| e.into_inner());
        handle_request(target, &mut shared, settings_path)
    };
    let mut response = format!("HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n");
    if let Some(location) = location {
        response.push_str(&format!("Location: {location}\r\n"));
    }
    response.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    ));
    let _ = stream.write_all(response.as_bytes());
}

/// Parse a request target (`/path?query`) into path + query map.
fn parse_target(target: &str) -> (String, HashMap<String, String>) {
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };
    let mut map = HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        match pair.split_once('=') {
            Some((k, v)) => {
                map.insert(percent_decode_query(k), percent_decode_query(v));
            },
            None => {
                map.insert(percent_decode_query(pair), String::new());
            },
        }
    }
    (path.to_string(), map)
}

/// Dispatch one request. Takes `&mut Shared` because the settings
/// routes mutate it (and persist). Pure over the shared state plus the
/// settings file path, so tests can drive it directly.
/// Returns (HTTP status, body, optional redirect Location).
pub fn handle_request(
    target: &str,
    shared: &mut Shared,
    settings_path: &std::path::Path,
) -> (u16, String, Option<String>) {
    let (path, query) = parse_target(target);
    match path.as_str() {
        "/settings" => (
            200,
            settings_page(shared, query.get("note").map(String::as_str)),
            None,
        ),
        "/settings/save" => {
            let note = apply_settings_save(&query, shared, settings_path);
            (
                303,
                String::new(),
                Some(format!("/settings?note={note}")),
            )
        },
        "/settings/reset" => {
            apply_settings_reset(shared, settings_path);
            (
                303,
                String::new(),
                Some("/settings?note=reset".to_string()),
            )
        },
        "/print" => (200, print_page(shared), None),
        "/print/save" => {
            // Round 5: the save is a real server route now (it used to
            // be an app-side side effect on the UrlChanged event, while
            // the webview itself loaded /print/save and got the 404
            // "Not found" page — the "Page: Not found" flash after
            // Save). The route saves, then redirects to /print, which
            // shows the saved path (or the error) — no 404 anywhere.
            match save_print_preview(shared, &downloads_dir()) {
                Ok(_) => (303, String::new(), Some("/print".to_string())),
                Err(e) => {
                    shared.print_preview_error = Some(e);
                    (303, String::new(), Some("/print".to_string()))
                },
            }
        },
        "/inspector" => (200, inspector_page(shared), None),
        "/inspector/recapture" => {
            // Ask the app to capture the inspected page again: set the
            // flag (the app polls it each Tick) and send the webview
            // back to the inspected page, so the capture runs on a
            // fresh load. A frost:// URL is translated back to the
            // loopback form the engine can load.
            match &shared.inspector_snapshot {
                Some(snap) => {
                    shared.inspector_recapture_requested = true;
                    let target = frost_url_to_loopback(&snap.url, &shared.base_url)
                        .unwrap_or_else(|| snap.url.clone());
                    (303, String::new(), Some(target))
                },
                None => (303, String::new(), Some("/inspector".to_string())),
            }
        },
        "/exit" => (200, exit_page(shared), None),
        "/exit/confirm" => (200, exit_confirm_page(shared), None),
        "/open" => {
            let path = query.get("path").cloned().unwrap_or_default();
            let path = std::path::Path::new(&path);
            let base = shared_base_url(shared);
            let (title, body) = file_browser::listing_page(path, &base);
            (200, page_shell(&title, &body, shared), None)
        },
        "/md" => {
            let path = query.get("path").cloned().unwrap_or_default();
            match file_browser::render_markdown(std::path::Path::new(&path), &shared_base_url(shared)) {
                Some((title, body)) => (200, page_shell(&title, &body, shared), None),
                None => (
                    404,
                    page_shell(
                        "Not found",
                        "<p>That file could not be read as Markdown.</p>",
                        shared,
                    ),
                    None,
                ),
            }
        },
        "/extensions" => (200, extensions_page(shared), None),
        _ => (
            404,
            page_shell(
                "Not found",
                "<p>There is no internal page at this address.</p><p><a href=\"/settings\">Settings</a></p>",
                shared,
            ),
            None,
        ),
    }
}

/// The base URL the server listens on, set by the app at startup.
fn shared_base_url(shared: &Shared) -> String {
    shared.base_url.clone()
}

/// Whether a homepage value is a loadable http(s) URL.
pub fn is_loadable_homepage(value: &str) -> bool {
    match url::Url::parse(value) {
        Ok(u) => matches!(u.scheme(), "http" | "https") && u.host_str().is_some(),
        Err(_) => false,
    }
}

/// Apply a settings-save request: validate the query, persist, and bump
/// the shared version. Called by the connection handler (which holds the
/// shared lock). Returns the note for the redirect.
pub fn apply_settings_save(
    query: &HashMap<String, String>,
    shared: &mut Shared,
    settings_path: &std::path::Path,
) -> &'static str {
    let mut settings = shared.settings.clone();
    let mut error: Option<&'static str> = None;
    if let Some(theme) = query.get("theme") {
        match theme.as_str() {
            "dark" => settings.theme = ThemeChoice::Dark,
            "light" => settings.theme = ThemeChoice::Light,
            _ => error = Some("bad_theme"),
        }
    }
    if let Some(homepage) = query.get("homepage") {
        if is_loadable_homepage(homepage) {
            settings.homepage = homepage.clone();
        } else {
            error = Some("bad_homepage");
        }
    }
    if let Some(search) = query.get("search") {
        match crate::settings::SearchEngine::from_id(search) {
            Some(engine) => settings.search_engine = engine,
            None => error = Some("bad_search"),
        }
    }
    if let Some(confirm) = query.get("confirm_exit") {
        settings.confirm_exit = confirm == "1";
    }
    if error.is_none() {
        shared.settings = settings;
        shared.settings_version += 1;
        if let Err(e) = shared.settings.save_to(settings_path) {
            eprintln!("frost: cannot persist settings: {e}");
        }
        "saved"
    } else {
        error.unwrap()
    }
}

/// The user's Downloads directory (`~/Downloads`, or a relative
/// `Downloads` when HOME is unset).
pub fn downloads_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("Downloads"))
        .unwrap_or_else(|| PathBuf::from("Downloads"))
}

/// Save the captured print preview PNG to `dir` (Round 5: creates the
/// directory first, so the save works even when ~/Downloads does not
/// exist yet — the Round 4 gap). Records the saved path (or the error)
/// on the shared state and returns the path on success. Pure over the
/// shared state plus the directory, so tests can drive it with a temp
/// dir.
pub fn save_print_preview(shared: &mut Shared, dir: &std::path::Path) -> Result<PathBuf, String> {
    let bytes = shared
        .print_preview_png
        .clone()
        .ok_or_else(|| "no print preview captured".to_string())?;
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("frost-print-{secs}.png"));
    std::fs::write(&path, &bytes)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    shared.print_saved_path = Some(path.display().to_string());
    shared.print_preview_error = None;
    Ok(path)
}

/// Apply a settings reset: defaults, persist, bump the version.
pub fn apply_settings_reset(shared: &mut Shared, settings_path: &std::path::Path) {
    shared.settings = Settings::default();
    shared.settings_version += 1;
    if let Err(e) = shared.settings.save_to(settings_path) {
        eprintln!("frost: cannot persist settings: {e}");
    }
}

/// Translate a `frost://` URL into the loopback URL the engine can load.
/// `base` is the server's base URL (`http://127.0.0.1:PORT`).
pub fn frost_url_to_loopback(frost_url: &str, base: &str) -> Option<String> {
    let rest = frost_url.strip_prefix("frost://")?;
    Some(format!("{base}/{rest}"))
}

/// Translate a loopback URL back into the `frost://` form for display.
pub fn loopback_url_to_frost(loopback_url: &str, base: &str) -> Option<String> {
    let rest = loopback_url.strip_prefix(base)?.strip_prefix('/')?;
    Some(format!("frost://{rest}"))
}

/// Whether `url` is one of this server's loopback URLs.
pub fn is_loopback_url(url: &str, base: &str) -> bool {
    url == base || url.starts_with(&format!("{base}/"))
}

/// HTML shell for an internal page, themed from the current settings.
pub fn page_shell(title: &str, body: &str, shared: &Shared) -> String {
    let colors = shared.settings.theme.chrome_colors();
    let link = if shared.settings.theme.is_dark() {
        "#4da3ff"
    } else {
        "#0057d9"
    };
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>{title}</title>\
         <style>body{{background:{};color:{};font-family:sans-serif;margin:0;padding:24px 32px;max-width:720px}}\
         a{{color:{}}}h1{{font-size:22px;margin:0 0 16px}}h2{{font-size:16px;margin:24px 0 8px}}\
         p,li{{line-height:1.5}}input,select{{font-size:14px;padding:6px 8px;margin:4px 0}}\
         button{{font-size:14px;padding:8px 16px;margin-top:12px;cursor:pointer}}\
         .muted{{color:{}}}label{{display:block;margin:12px 0 4px;font-weight:bold}}\
         .note{{border:1px solid #888;padding:8px 12px;margin:0 0 16px}}\
         code{{background:rgba(127,127,127,.25);padding:1px 4px}}\
         pre{{background:rgba(127,127,127,.2);padding:12px;overflow:auto}}\
         table{{border-collapse:collapse}}td,th{{border:1px solid #888;padding:6px 10px;text-align:left}}\
         </style></head><body>{body}</body></html>",
        colors.css_background(),
        colors.css_text(),
        link,
        colors.css_muted(),
    )
}

fn settings_page(shared: &Shared, note: Option<&str>) -> String {
    let s = &shared.settings;
    let note_html = match note {
        Some("saved") => "<p class=\"note\">Settings saved.</p>",
        Some("reset") => "<p class=\"note\">Settings reset to defaults.</p>",
        Some("bad_homepage") => {
            "<p class=\"note\">That homepage is not a valid http(s) URL — nothing was changed.</p>"
        },
        Some("bad_search") => {
            "<p class=\"note\">Unknown search engine — nothing was changed.</p>"
        },
        Some("bad_theme") => "<p class=\"note\">Unknown theme — nothing was changed.</p>",
        _ => "",
    };
    let theme_links = format!(
        "<p>Theme: <a href=\"/settings/save?theme=dark\">Dark</a> \
         | <a href=\"/settings/save?theme=light\">Light</a> \
         <span class=\"muted\">(current: {})</span></p>",
        if s.theme.is_dark() { "Dark" } else { "Light" }
    );
    let search_options: String = crate::settings::SearchEngine::all()
        .iter()
        .map(|e| {
            let selected = if *e == s.search_engine { " selected" } else { "" };
            format!(
                "<option value=\"{}\"{}>{}</option>",
                e.id(),
                selected,
                e.label()
            )
        })
        .collect();
    let confirm_checked = if s.confirm_exit { " checked" } else { "" };
    let body = format!(
        "<h1>Settings</h1>{note_html}{theme_links}\
         <form action=\"/settings/save\" method=\"get\">\
         <label for=\"homepage\">Homepage (opens on launch)</label>\
         <input id=\"homepage\" name=\"homepage\" type=\"text\" size=\"48\" value=\"{}\">\
         <label for=\"search\">Default search engine (used when the address bar gets a search query)</label>\
         <select id=\"search\" name=\"search\">{search_options}</select>\
         <label><input type=\"checkbox\" name=\"confirm_exit\" value=\"1\"{confirm_checked}> \
         Ask before quitting (show the exit confirmation page)</label>\
         <br><button type=\"submit\">Save settings</button></form>\
         <h2>More</h2>\
         <p><a href=\"/extensions\">Extensions</a> — content-script extensions loaded from disk.</p>\
         <p><a href=\"/settings/reset\">Restore defaults</a></p>\
         <p class=\"muted\">Theme and search apply immediately; the homepage applies on next launch.</p>",
        file_browser::html_escape(&s.homepage),
    );
    page_shell("Settings", &body, shared)
}

fn print_page(shared: &Shared) -> String {
    // The printed page's identity is frozen at print-open time
    // (print_page_title/print_page_url): the live current page is
    // frost://print by the time this re-renders after a Save.
    let title = if shared.print_page_title.is_empty() {
        "Untitled page".to_string()
    } else {
        shared.print_page_title.clone()
    };
    // F-003 (Round 4): a real print preview — a screenshot of the
    // page's current viewport, served as /print/preview.png. Servo
    // 0.8.0 has no print pipeline (no paginated print layout, no
    // print-to-PDF), so this is a viewport screenshot, not a print
    // layout: what you see is the page as it appears on screen, and
    // "Save as PNG" writes that image to ~/Downloads for printing
    // from any viewer. The page says so plainly.
    let preview = if let Some(error) = &shared.print_preview_error {
        format!(
            "<p><b>Preview failed:</b> {}</p>",
            file_browser::html_escape(error),
        )
    } else if shared.print_preview_png.is_some() {
        let saved = match &shared.print_saved_path {
            Some(path) => format!(
                "<p>Saved to <b>{}</b></p>",
                file_browser::html_escape(path),
            ),
            None => String::new(),
        };
        format!(
            "<p>{}</p>\
             <p><img src=\"/print/preview.png\" alt=\"Print preview\" \
             style=\"max-width:100%;border:1px solid #555\"></p>\
             <p><a href=\"/print/save\">Save as PNG</a> \
             (writes the preview to ~/Downloads)</p>{saved}",
            file_browser::html_escape(&shared.print_preview_label),
        )
    } else {
        "<p>Capturing the preview… (if this stays, the capture failed — \
         try Print page again.)</p>"
            .to_string()
    };
    let body = format!(
        "<h1>Print page</h1>\
         <p>Page: <b>{}</b></p>\
         <p class=\"muted\">{}</p>\
         {preview}\
         <p>This is a screenshot of the page as it appears on screen, \
         not a paginated print layout — Servo 0.8.0 has no print \
         pipeline, so there is no print-to-PDF or printer output. \
         Save the PNG and print it from any image viewer.</p>\
         <p><a href=\"{}\">Back to the page</a></p>",
        file_browser::html_escape(&title),
        file_browser::html_escape(&shared.print_page_url),
        file_browser::html_escape(
            shared
                .last_external_url
                .as_deref()
                .unwrap_or(&shared.current_page_url)
        ),
    );
    page_shell("Print page", &body, shared)
}

/// The inspector page (Round 5). The shell is static: the tree, the
/// header, and the details pane are all rendered by the page's own
/// JavaScript from `/inspector/snapshot.json`, so the server only has
/// to fill in the "back to the page" link for the no-snapshot state
/// (`{BACK_URL}` placeholder). The tree is a snapshot — the page says
/// so — never presented as live.
fn inspector_page(shared: &Shared) -> String {
    let back = shared
        .last_external_url
        .clone()
        .unwrap_or_else(|| shared.settings.homepage.clone());
    let html = r##"<h1>Inspector</h1>
<div id="insp-head"><p class="muted">Loading snapshot…</p></div>
<div id="insp-main" style="display:none">
<div style="display:flex;gap:24px;align-items:flex-start">
<div id="insp-tree" style="flex:1;min-width:0;overflow:auto"></div>
<div id="insp-details" style="flex:0 0 340px;border:1px solid #888;padding:12px">
<p class="muted">Click a node to see its details.</p>
</div>
</div>
</div>
<div id="insp-empty" style="display:none">
<p>No page inspected yet.</p>
<p>Go to a page, then open the Inspector from the hamburger menu (or press F12). Frost captures the page's structure first, then shows it here — a snapshot of the page as it was when captured, not a live view.</p>
<p><a href="{BACK_URL}">Back to the page</a></p>
</div>
<style>
body{max-width:1100px}
#insp-tree ul{list-style:none;margin:0;padding-left:22px}
#insp-tree>ul{padding-left:0}
#insp-tree li{font-family:monospace;font-size:13px;line-height:1.7;white-space:nowrap}
#insp-tree .tw{display:inline-block;width:16px;cursor:pointer;color:#888;text-align:center}
#insp-tree .nl{cursor:pointer;padding:1px 4px}
#insp-tree .nl:hover{background:rgba(127,127,127,.3)}
#insp-details h2{font-size:16px;word-break:break-all}
#insp-details h3{font-size:13px;margin:16px 0 4px}
#insp-details table{font-size:12px}
#insp-details td{word-break:break-all}
</style>
<script>
(function(){
var byId = {};
function esc(s){ return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;'); }
function label(n){
  if(n.tag === '#text') return '"' + esc(n.text || '') + '"';
  var s = esc(n.tag);
  if(n.attributes.id) s += '#' + esc(n.attributes.id);
  if(n.attributes.class){
    var cls = n.attributes.class.trim().split(/\s+/);
    for(var i=0;i<cls.length;i++) if(cls[i]) s += '.' + esc(cls[i]);
  }
  return s;
}
function renderNode(n){
  byId[n.node_id] = n;
  var html = '<li data-id="' + n.node_id + '">';
  if(n.children && n.children.length){
    html += '<span class="tw" data-toggle="1">\u2212</span>';
  } else {
    html += '<span class="tw">&nbsp;</span>';
  }
  html += '<span class="nl">' + label(n) + '</span>';
  if(n.children && n.children.length){
    html += '<ul>';
    for(var i=0;i<n.children.length;i++) html += renderNode(n.children[i]);
    html += '</ul>';
  }
  return html + '</li>';
}
function details(n){
  var h = '<h2>' + esc(n.tag) + '</h2>';
  var keys = Object.keys(n.attributes || {});
  if(keys.length){
    h += '<h3>Attributes</h3><table>';
    for(var i=0;i<keys.length;i++) h += '<tr><td>' + esc(keys[i]) + '</td><td>' + esc(n.attributes[keys[i]]) + '</td></tr>';
    h += '</table>';
  } else {
    h += '<p class="muted">No attributes.</p>';
  }
  if(n.text) h += '<h3>Text</h3><p>' + esc(n.text) + '</p>';
  var cs = n.computed_style || {};
  var shown = ['color','background_color','display','position','font_family','font_size','font_weight','width','height'];
  var rows = '';
  for(var j=0;j<shown.length;j++) if(cs[shown[j]]) rows += '<tr><td>' + esc(shown[j]) + '</td><td>' + esc(cs[shown[j]]) + '</td></tr>';
  if(rows) h += '<h3>Computed style</h3><table>' + rows + '</table>';
  if(n.box_model){
    var b = n.box_model;
    h += '<h3>Box (viewport-relative)</h3><p>x ' + b.x + ' &middot; y ' + b.y + ' &middot; ' + b.width + ' &times; ' + b.height + '</p>';
    h += '<p class="muted">margin ' + b.margin.join(' ') + '<br>padding ' + b.padding.join(' ') + '<br>border ' + b.border.join(' ') + '</p>';
  }
  return h;
}
fetch('/inspector/snapshot.json').then(function(r){
  if(!r.ok) throw new Error('no snapshot');
  return r.json();
}).then(function(data){
  var when = new Date(data.capturedAt).toLocaleString();
  var head = '<p>Snapshot of <b>' + esc(data.url) + '</b></p>'
    + '<p class="muted">Captured ' + esc(when) + ' &middot; ' + data.nodeCount + ' nodes'
    + (data.truncated ? ' &middot; <b>truncated</b> — the page is bigger than the walker limit, so the deepest nodes are missing' : '')
    + ' — this is a snapshot, not a live view. The page may have changed since.</p>'
    + '<p><a href="' + esc(data.url) + '">Back to the page</a> &middot; <a href="/inspector/recapture">Re-capture</a></p>';
  document.getElementById('insp-head').innerHTML = head;
  document.getElementById('insp-tree').innerHTML = '<ul>' + renderNode(data.root) + '</ul>';
  document.getElementById('insp-main').style.display = 'block';
  document.getElementById('insp-tree').addEventListener('click', function(e){
    var t = e.target.closest('.tw');
    if(t && t.getAttribute('data-toggle')){
      var ul = t.parentElement.querySelector('ul');
      if(ul){
        var hidden = ul.style.display === 'none';
        ul.style.display = hidden ? '' : 'none';
        t.textContent = hidden ? '\u2212' : '+';
      }
      return;
    }
    var li = e.target.closest('li[data-id]');
    if(!li) return;
    var n = byId[+li.getAttribute('data-id')];
    if(n) document.getElementById('insp-details').innerHTML = details(n);
  });
}).catch(function(){
  document.getElementById('insp-head').innerHTML = '';
  document.getElementById('insp-empty').style.display = 'block';
});
})();
</script>
"##;
    page_shell(
        "Inspector",
        &html.replace("{BACK_URL}", &file_browser::html_escape(&back)),
        shared,
    )
}

fn exit_page(shared: &Shared) -> String {
    let back = shared
        .last_external_url
        .clone()
        .unwrap_or_else(|| shared.settings.homepage.clone());
    let body = format!(
        "<h1>Quit Frost?</h1>\
         <p>Your tabs are not saved yet — quitting closes the browser.</p>\
         <p><a href=\"/exit/confirm\">Quit Frost</a></p>\
         <p><a href=\"{}\">Cancel — back to the page</a></p>",
        file_browser::html_escape(&back),
    );
    page_shell("Exit Frost", &body, shared)
}

fn exit_confirm_page(shared: &Shared) -> String {
    page_shell(
        "Exiting…",
        "<h1>Quitting Frost…</h1><p>If the window is still here, close it.</p>",
        shared,
    )
}

fn extensions_page(shared: &Shared) -> String {
    let mut body = String::from("<h1>Extensions</h1>");
    if shared.extensions.is_empty() {
        body.push_str(
            "<p>No extensions loaded. Put an extension directory (manifest.json + content script) \
             under <code>~/.config/frost/extensions/</code> or set <code>FROST_EXTENSIONS_DIR</code>, \
             then restart Frost.</p>",
        );
    } else {
        body.push_str("<table><tr><th>Name</th><th>Version</th><th>Enabled</th></tr>");
        for (name, version, enabled) in &shared.extensions {
            body.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
                file_browser::html_escape(name),
                file_browser::html_escape(version),
                if *enabled { "yes" } else { "no" }
            ));
        }
        body.push_str("</table>");
    }
    if !shared.failed_extensions.is_empty() {
        body.push_str("<h2>Skipped (bad manifest)</h2><ul>");
        for (dir, error) in &shared.failed_extensions {
            body.push_str(&format!(
                "<li><code>{}</code> — {}</li>",
                file_browser::html_escape(dir),
                file_browser::html_escape(error)
            ));
        }
        body.push_str("</ul>");
    }
    body.push_str("<p><a href=\"/settings\">Back to Settings</a></p>");
    page_shell("Extensions", &body, shared)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_shared() -> Shared {
        Shared::new(Settings::default())
    }

    #[test]
    fn frost_and_loopback_urls_translate_both_ways() {
        let base = "http://127.0.0.1:54321";
        assert_eq!(
            frost_url_to_loopback("frost://settings", base).as_deref(),
            Some("http://127.0.0.1:54321/settings")
        );
        assert_eq!(
            frost_url_to_loopback("frost://open?path=%2Fhome%2Fjoel", base).as_deref(),
            Some("http://127.0.0.1:54321/open?path=%2Fhome%2Fjoel")
        );
        assert_eq!(
            frost_url_to_loopback("frost://exit/confirm", base).as_deref(),
            Some("http://127.0.0.1:54321/exit/confirm")
        );
        assert_eq!(
            loopback_url_to_frost("http://127.0.0.1:54321/settings", base).as_deref(),
            Some("frost://settings")
        );
        assert_eq!(
            loopback_url_to_frost("http://127.0.0.1:54321/open?path=%2Ftmp", base).as_deref(),
            Some("frost://open?path=%2Ftmp")
        );
        assert_eq!(frost_url_to_loopback("https://example.org", base), None);
        assert_eq!(loopback_url_to_frost("https://example.org", base), None);
        assert!(is_loopback_url("http://127.0.0.1:54321/settings", base));
        assert!(!is_loopback_url("https://example.org", base));
    }

    #[test]
    fn settings_page_renders_current_values() {
        let mut shared = test_shared();
        let (_, body, _) =
            handle_request("/settings", &mut shared, std::path::Path::new("/nonexistent"));
        assert!(body.contains("Settings"));
        assert!(body.contains("https://example.org"));
        assert!(body.contains("DuckDuckGo"));
    }

    #[test]
    fn settings_save_applies_and_persists() {
        let path = std::env::temp_dir().join("frost-server-settings-test.json");
        let _ = std::fs::remove_file(&path);
        let mut shared = test_shared();
        let mut query = HashMap::new();
        query.insert("theme".to_string(), "light".to_string());
        query.insert("homepage".to_string(), "https://example.com".to_string());
        query.insert("search".to_string(), "google".to_string());
        query.insert("confirm_exit".to_string(), "0".to_string());
        let note = apply_settings_save(&query, &mut shared, &path);
        assert_eq!(note, "saved");
        assert_eq!(shared.settings.theme, ThemeChoice::Light);
        assert_eq!(shared.settings.homepage, "https://example.com");
        assert_eq!(
            shared.settings.search_engine,
            crate::settings::SearchEngine::Google
        );
        assert!(!shared.settings.confirm_exit);
        assert_eq!(shared.settings_version, 1);
        assert_eq!(Settings::load_from(&path).homepage, "https://example.com");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn settings_save_rejects_bad_homepage_and_keeps_old() {
        let path = std::env::temp_dir().join("frost-server-settings-test2.json");
        let _ = std::fs::remove_file(&path);
        let mut shared = test_shared();
        let mut query = HashMap::new();
        query.insert("homepage".to_string(), "not a url".to_string());
        let note = apply_settings_save(&query, &mut shared, &path);
        assert_eq!(note, "bad_homepage");
        assert_eq!(shared.settings.homepage, "https://example.org");
        assert_eq!(shared.settings_version, 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn exit_page_links_back_to_last_external_url() {
        let mut shared = test_shared();
        shared.last_external_url = Some("https://example.org".to_string());
        let (_, body, _) =
            handle_request("/exit", &mut shared, std::path::Path::new("/nonexistent"));
        assert!(body.contains("Quit Frost"));
        assert!(body.contains("https://example.org"));
    }

    #[test]
    fn unknown_path_is_404() {
        let mut shared = test_shared();
        let (status, _, _) =
            handle_request("/nope", &mut shared, std::path::Path::new("/nonexistent"));
        assert_eq!(status, 404);
    }

    #[test]
    fn inspector_page_renders_shell_and_fetch_endpoint() {
        let mut shared = test_shared();
        let (_, body, _) =
            handle_request("/inspector", &mut shared, std::path::Path::new("/nonexistent"));
        assert!(body.contains("Inspector"), "{body}");
        assert!(body.contains("/inspector/snapshot.json"), "{body}");
        assert!(body.contains("No page inspected yet"), "{body}");
        // No snapshot stored yet: the recapture link goes back to the
        // inspector page, not to a page that was never captured.
        let (_, _, location) =
            handle_request("/inspector/recapture", &mut shared, std::path::Path::new("/nonexistent"));
        assert_eq!(location.as_deref(), Some("/inspector"));
    }

    #[test]
    fn inspector_recapture_redirects_to_the_inspected_page() {
        let mut shared = test_shared();
        shared.base_url = "http://127.0.0.1:54321".to_string();
        shared.inspector_snapshot = Some(InspectorSnapshot {
            url: "https://example.org/".to_string(),
            captured_at_ms: 1794643768000,
            node_count: 8,
            truncated: false,
            json: "{}".to_string(),
        });
        let (status, _, location) = handle_request(
            "/inspector/recapture",
            &mut shared,
            std::path::Path::new("/nonexistent"),
        );
        assert_eq!(status, 303);
        assert_eq!(location.as_deref(), Some("https://example.org/"));
        assert!(shared.inspector_recapture_requested);
    }

    #[test]
    fn print_save_creates_the_downloads_dir_and_redirects() {
        // Round 5 wart fix: the save used to fail outright when
        // ~/Downloads did not exist; the route now creates it first.
        let dir = std::env::temp_dir().join("frost-print-save-test");
        let _ = std::fs::remove_dir_all(&dir);
        let mut shared = test_shared();
        shared.print_preview_png = Some(vec![0x89, 0x50, 0x4e, 0x47]); // PNG magic
        let path = save_print_preview(&mut shared, &dir).unwrap();
        assert!(path.starts_with(&dir));
        assert!(path.exists());
        assert!(shared.print_saved_path.is_some());
        // And the route itself redirects to /print (no 404 "Not found"
        // page — the Round 4 flash after Save). Drive it with no
        // preview captured, so it does not write to the real
        // ~/Downloads: the error path redirects the same way.
        shared.print_preview_png = None;
        let (status, _, location) = handle_request(
            "/print/save",
            &mut shared,
            std::path::Path::new("/nonexistent"),
        );
        assert_eq!(status, 303);
        assert_eq!(location.as_deref(), Some("/print"));
        assert!(shared.print_preview_error.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn print_save_fails_cleanly_with_no_preview() {
        let mut shared = test_shared();
        assert!(shared.print_preview_png.is_none());
        let dir = std::env::temp_dir().join("frost-print-save-test2");
        assert!(save_print_preview(&mut shared, &dir).is_err());
    }

    #[test]
    fn print_page_keeps_the_printed_page_after_a_reload() {
        // Round 5 follow-up (caught in the harness gate): after Save
        // the server 303s back to /print, which re-renders — and by
        // then the live current page IS frost://print. The header must
        // still name the page that was printed, frozen at print-open.
        let mut shared = test_shared();
        shared.print_page_title = "Example Domain".to_string();
        shared.print_page_url = "https://example.org/".to_string();
        shared.print_preview_png = Some(vec![0x89, 0x50, 0x4e, 0x47]);
        shared.print_preview_label = "Preview of https://example.org/".to_string();
        shared.current_page_title = "Print page".to_string();
        shared.current_page_url = "frost://print".to_string();
        let body = print_page(&shared);
        assert!(body.contains("Page: <b>Example Domain</b>"));
        assert!(body.contains("https://example.org/"));
        assert!(!body.contains("Page: <b>Print page</b>"));
    }
}
