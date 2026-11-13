//! File-browser page for the Open menu entry (Round 2).
//!
//! Open is a real working file browser, not a placeholder. The app
//! generates a directory listing as HTML and loads it through the
//! engine as a `data:` page, the same way the other menu pages load.
//! Folder clicks navigate the webview to `frost://open?path=<url-encoded
//! path>`; the app intercepts that navigation (see `parse_open_url`)
//! and re-lists the folder. File clicks are plain `file://` URLs, which
//! Servo loads natively, so clicking a file opens it in the browser.
//!
//! The listing starts at the home directory. Hidden entries (names
//! starting with `.`) are skipped — a home listing with every dotfile
//! would drown the real folders. Sizes are shown for files; directories
//! sort first, then files, each alphabetical (case-insensitive).

use std::path::{Path, PathBuf};

use url::Url;

/// Scheme of the listing's folder links.
pub const OPEN_SCHEME: &str = "frost";
/// Host of the listing's folder links.
pub const OPEN_HOST: &str = "open";

/// The directory the browser starts in: `$HOME`, or `/` if unset.
pub fn home_dir() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/"))
}

/// The URL a folder link points at: `frost://open?path=<url-encoded path>`.
pub fn listing_url(path: &Path) -> String {
    let encoded: String =
        url::form_urlencoded::byte_serialize(path.as_os_str().as_encoded_bytes()).collect();
    format!("{OPEN_SCHEME}://{OPEN_HOST}?path={encoded}")
}

/// If `url` is a `frost://open` folder navigation, return the directory
/// it names. Anything else returns `None`.
pub fn parse_open_url(url: &Url) -> Option<PathBuf> {
    if url.scheme() != OPEN_SCHEME || url.host_str() != Some(OPEN_HOST) {
        return None;
    }
    url.query_pairs()
        .find(|(key, _)| key == "path")
        .map(|(_, value)| PathBuf::from(value.into_owned()))
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

/// `(title, body_html)` for the listing of `dir`. The title is already
/// HTML-escaped (it is inserted verbatim into the page shell).
pub fn listing_page(dir: &Path) -> (String, String) {
    let dir_display = dir.display().to_string();
    let title = format!("Files — {}", html_escape(&dir_display));
    let mut body = String::new();
    body.push_str(&format!("<p>Folder: <code>{}</code></p>", html_escape(&dir_display)));

    // Up link, unless already at the root.
    if let Some(parent) = dir.parent() {
        body.push_str(&format!(
            "<p><a href=\"{}\">↑ Up to {}</a></p>",
            listing_url(parent),
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
                            listing_url(&path),
                            html_escape(&name),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_url_round_trips() {
        let dir = Path::new("/home/someone/My Docs");
        let url = Url::parse(&listing_url(dir)).unwrap();
        assert_eq!(parse_open_url(&url).unwrap(), dir);
    }

    #[test]
    fn non_open_urls_are_ignored() {
        assert!(parse_open_url(&Url::parse("https://example.org/").unwrap()).is_none());
        assert!(parse_open_url(&Url::parse("frost://settings").unwrap()).is_none());
        assert!(parse_open_url(&Url::parse("frost://open").unwrap()).is_none());
    }

    #[test]
    fn listing_skips_hidden_entries() {
        let dir = std::env::temp_dir().join("frost-file-browser-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir(dir.join("visible-dir")).unwrap();
        std::fs::write(dir.join("visible.txt"), b"hello").unwrap();
        std::fs::write(dir.join(".hidden"), b"nope").unwrap();
        let (_, body) = listing_page(&dir);
        assert!(body.contains("visible-dir/"), "{body}");
        assert!(body.contains("visible.txt"), "{body}");
        assert!(!body.contains(".hidden"), "{body}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
