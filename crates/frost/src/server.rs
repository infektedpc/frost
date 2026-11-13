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
    let title = if shared.current_page_title.is_empty() {
        "Untitled page".to_string()
    } else {
        shared.current_page_title.clone()
    };
    let body = format!(
        "<h1>Print page</h1>\
         <p>Page: <b>{}</b></p>\
         <p class=\"muted\">{}</p>\
         <p>Printing is not wired up yet (fork patch F-003, planned). \
         There is no print preview or printer output in this build — \
         this page only names the page you asked to print.</p>\
         <p><a href=\"{}\">Back to the page</a></p>",
        file_browser::html_escape(&title),
        file_browser::html_escape(&shared.current_page_url),
        file_browser::html_escape(
            shared
                .last_external_url
                .as_deref()
                .unwrap_or(&shared.current_page_url)
        ),
    );
    page_shell("Print page", &body, shared)
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
}
