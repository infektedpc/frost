//! File-browser page for the Open menu entry (Round 2; Round 3 serves
//! it through the loopback server instead of a `data:` page).
//!
//! Open is a real working file browser, not a placeholder. The app
//! generates a directory listing as HTML and the internal server serves
//! it at `{base}/open?path=...` (Round 3), so the address bar shows
//! `frost://open?path=...`. Folder clicks navigate to another
//! `{base}/open` URL, which the server lists directly — no app
//! interception needed for the listing itself. The app still intercepts
//! `frost://open` navigations (typed in the bar, or reached through
//! history) and translates them to the loopback form; see
//! `parse_open_url`.
//!
//! File clicks are plain `file://` URLs, which Servo loads natively,
//! except `.md` files: those link to `{base}/md?path=...` and the
//! server renders them as formatted HTML (`render_markdown`).
//!
//! The listing starts at the home directory. Hidden entries (names
//! starting with `.`) are skipped — a home listing with every dotfile
//! would drown the real folders. Sizes are shown for files; directories
//! sort first, then files, each alphabetical (case-insensitive).

use std::path::{Path, PathBuf};

use url::Url;

/// Scheme of the `frost://open` form (typed in the address bar, or
/// reached through history).
pub const OPEN_SCHEME: &str = "frost";
/// Host of the `frost://open` form.
pub const OPEN_HOST: &str = "open";

/// The directory the browser starts in: `$HOME`, or `/` if unset.
pub fn home_dir() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"))
}

/// The URL a folder link points at: `{base}/open?path=<url-encoded
/// path>`, where `base` is the internal server's base URL
/// (`http://127.0.0.1:PORT`).
pub fn listing_url(path: &Path, base: &str) -> String {
    let encoded: String =
        url::form_urlencoded::byte_serialize(path.as_os_str().as_encoded_bytes()).collect();
    format!("{base}/open?path={encoded}")
}

/// If `url` is a folder navigation — either the `frost://open` form or
/// the loopback `/open` form the server serves — return the directory
/// it names. Anything else returns `None`.
pub fn parse_open_url(url: &Url) -> Option<PathBuf> {
    let is_open = (url.scheme() == OPEN_SCHEME && url.host_str() == Some(OPEN_HOST))
        || (matches!(url.scheme(), "http" | "https") && url.path() == "/open");
    if !is_open {
        return None;
    }
    url.query_pairs()
        .find(|(key, _)| key == "path")
        .map(|(_, value)| PathBuf::from(value.into_owned()))
}

/// The URL a Markdown file link points at: the server renders it.
fn markdown_url(path: &Path, base: &str) -> String {
    let encoded: String =
        url::form_urlencoded::byte_serialize(path.as_os_str().as_encoded_bytes()).collect();
    format!("{base}/md?path={encoded}")
}

/// Escape text for insertion into the listing's HTML (names and paths
/// can contain `&`, `<`, `>`, `"`).
pub fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Human-readable file size (`842 B`, `12 KB`, `3.4 MB`).
fn human_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// `(title, body_html)` for the listing of `dir`. `base` is the
/// internal server's base URL; folder and Markdown links are built
/// from it. The title is already HTML-escaped (it is inserted verbatim
/// into the page shell).
pub fn listing_page(dir: &Path, base: &str) -> (String, String) {
    let dir_display = dir.display().to_string();
    let title = format!("Files — {}", html_escape(&dir_display));
    let mut body = String::new();
    body.push_str(&format!("<p>Folder: <code>{}</code></p>", html_escape(&dir_display)));

    // Up link, unless already at the root.
    if let Some(parent) = dir.parent() {
        body.push_str(&format!(
            "<p><a href=\"{}\">↑ Up to {}</a></p>",
            listing_url(parent, base),
            html_escape(&parent.display().to_string()),
        ));
    }

    match std::fs::read_dir(dir) {
        Ok(entries) => {
            // (name, is_dir, size): directories first, then files,
            // each alphabetical, case-insensitive. Hidden entries skipped.
            let mut rows: Vec<(String, bool, u64)> = Vec::new();
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                let size = if is_dir {
                    0
                } else {
                    entry.metadata().map(|m| m.len()).unwrap_or(0)
                };
                rows.push((name, is_dir, size));
            }
            rows.sort_by(|a, b| {
                b.1.cmp(&a.1)
                    .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
            });
            if rows.is_empty() {
                body.push_str("<p>(Empty folder.)</p>");
            } else {
                body.push_str("<ul>");
                for (name, is_dir, size) in rows {
                    let path = dir.join(&name);
                    if is_dir {
                        body.push_str(&format!(
                            "<li>[dir] <a href=\"{}\">{}/</a></li>",
                            listing_url(&path, base),
                            html_escape(&name),
                        ));
                    } else if path.extension().is_some_and(|ext| ext == "md") {
                        // Markdown renders as formatted HTML through the
                        // server (Round 3 decision: render, don't show
                        // Servo's "Unknown content type" page).
                        body.push_str(&format!(
                            "<li><a href=\"{}\">{}</a> ({})</li>",
                            markdown_url(&path, base),
                            html_escape(&name),
                            human_size(size),
                        ));
                    } else {
                        let file_url = Url::from_file_path(&path)
                            .map(|u| u.to_string())
                            .unwrap_or_default();
                        body.push_str(&format!(
                            "<li><a href=\"{}\">{}</a> ({})</li>",
                            file_url,
                            html_escape(&name),
                            human_size(size),
                        ));
                    }
                }
                body.push_str("</ul>");
            }
        },
        Err(e) => {
            body.push_str(&format!(
                "<p>Cannot list this folder: {}</p>",
                html_escape(&e.to_string()),
            ));
        },
    }
    (title, body)
}

/// Render a Markdown file as formatted HTML (Round 3). Returns
/// `(title, body_html)`, or `None` when the file cannot be read.
/// The body links back to the parent folder and to the raw file.
pub fn render_markdown(path: &Path, base: &str) -> Option<(String, String)> {
    let text = std::fs::read_to_string(path).ok()?;
    let parser = pulldown_cmark::Parser::new_ext(&text, pulldown_cmark::Options::all());
    let mut rendered = String::new();
    pulldown_cmark::html::push_html(&mut rendered, parser);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file.md".to_string());
    let title = format!("Markdown — {}", html_escape(&name));
    let raw_url = Url::from_file_path(path).map(|u| u.to_string()).unwrap_or_default();
    let parent = path.parent().map(|p| p.to_path_buf());
    let mut body = format!(
        "<p><a href=\"{}\">← Back to folder</a> · <a href=\"{}\">Raw file</a></p>",
        parent
            .as_ref()
            .map(|p| listing_url(p, base))
            .unwrap_or_else(|| format!("{base}/open")),
        raw_url,
    );
    body.push_str(&rendered);
    Some((title, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_url_round_trips() {
        let dir = Path::new("/home/someone/My Docs");
        let base = "http://127.0.0.1:54321";
        let url = Url::parse(&listing_url(dir, base)).unwrap();
        assert_eq!(parse_open_url(&url).unwrap(), dir);
        // The frost:// form parses too (typed in the bar / history).
        let frost_url = Url::parse("frost://open?path=%2Fhome%2Fsomeone%2FMy%20Docs").unwrap();
        assert_eq!(parse_open_url(&frost_url).unwrap(), dir);
    }

    #[test]
    fn non_open_urls_are_ignored() {
        assert!(parse_open_url(&Url::parse("https://example.org/").unwrap()).is_none());
        assert!(parse_open_url(&Url::parse("frost://settings").unwrap()).is_none());
        assert!(parse_open_url(&Url::parse("frost://open").unwrap()).is_none());
        assert!(parse_open_url(&Url::parse("http://127.0.0.1:54321/settings").unwrap())
            .is_none());
    }

    #[test]
    fn listing_skips_hidden_entries() {
        let dir = std::env::temp_dir().join("frost-file-browser-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir(dir.join("visible-dir")).unwrap();
        std::fs::write(dir.join("visible.txt"), b"hello").unwrap();
        std::fs::write(dir.join(".hidden"), b"nope").unwrap();
        let (_, body) = listing_page(&dir, "http://127.0.0.1:54321");
        assert!(body.contains("visible-dir/"), "{body}");
        assert!(body.contains("visible.txt"), "{body}");
        assert!(!body.contains(".hidden"), "{body}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn markdown_renders_to_html() {
        let dir = std::env::temp_dir().join("frost-markdown-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("note.md");
        std::fs::write(&path, "# Hello\n\nSome **bold** text.\n").unwrap();
        let (title, body) = render_markdown(&path, "http://127.0.0.1:54321").unwrap();
        assert!(title.contains("note.md"), "{title}");
        assert!(body.contains("<h1>Hello</h1>"), "{body}");
        assert!(body.contains("<strong>bold</strong>"), "{body}");
        assert!(render_markdown(&dir.join("missing.md"), "http://127.0.0.1:54321").is_none());
        // A listing links .md files to the render route, not file://.
        let (_, listing) = listing_page(&dir, "http://127.0.0.1:54321");
        assert!(listing.contains("/md?path="), "{listing}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
