//! Deep Freeze archives (Round 15): save the active pane's page as a
//! single self-contained HTML file, so it can be opened later with no
//! network.
//!
//! The capture runs in the page, on a CLONE of the live DOM — the live
//! page is never mutated: relative `src`/`href`/`srcset` URLs are
//! rewritten to absolute against the page's own URL (the browser's own
//! URL parser does the resolving), and a `<base href="…">` is inserted
//! into the clone's `<head>` so the snapshot keeps resolving the same
//! way when it is opened as a `file://` URL. The captured
//! `outerHTML` is then wrapped with a doctype + an identifying
//! comment and written to the archives dir.
//!
//! Storage: `$XDG_DATA_HOME/frost/archives/`, else
//! `~/.local/share/frost/archives/` — the XDG data-dir parallel of
//! `settings.rs`'s config-dir rule. An `index.json` in the same dir
//! lists the archives (title, original URL, file name, freeze time,
//! size); listing also picks up orphan `.html` files with no index
//! entry (title = file name, URL unknown) and prunes index entries
//! whose file is gone, so the list stays honest if files are moved
//! by hand.
//!
//! Honest v1 limits: the snapshot is the static DOM at freeze time.
//! Scripts in the snapshot run against that static DOM when the
//! archive is opened — a plain page replays cleanly, a heavy
//! JavaScript app may not. Subresources re-resolve against the
//! original URL, so they need the network (or the browser cache) to
//! load when the archive is opened; the DOM text itself is frozen
//! in the file. Snowplow-blocked resources never loaded, so they
//! are simply not in the archive. There is no incremental update —
//! re-freeze to refresh.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cache::DiskCache;

/// The capture script (Round 15): runs in the page via
/// `WebView::evaluate_javascript_string` and returns the frozen
/// document's `outerHTML` as a string. It works on a clone of
/// `documentElement`, never on the live DOM.
pub const FREEZE_CAPTURE_SCRIPT: &str = r#"
(() => {
  const base = document.baseURI;
  const clone = document.documentElement.cloneNode(true);
  const head = clone.querySelector('head');
  if (head && !clone.querySelector('base')) {
    const b = document.createElement('base');
    b.setAttribute('href', base);
    head.insertBefore(b, head.firstChild);
  }
  const absolute = (v) => {
    try { return new URL(v, base).href; } catch (e) { return v; }
  };
  const relative = (v) => v && !/^(https?:|data:|blob:|file:|#|javascript:|mailto:|tel:)/i.test(v);
  clone.querySelectorAll('[src]').forEach((el) => {
    const v = el.getAttribute('src');
    if (relative(v)) el.setAttribute('src', absolute(v));
  });
  clone.querySelectorAll('[href]').forEach((el) => {
    const v = el.getAttribute('href');
    if (relative(v)) el.setAttribute('href', absolute(v));
  });
  clone.querySelectorAll('[srcset]').forEach((el) => {
    const v = el.getAttribute('srcset');
    if (!v) return;
    el.setAttribute('srcset', v.split(',').map((part) => {
      const trimmed = part.trim();
      if (!trimmed) return part;
      const bits = trimmed.split(/\s+/);
      if (relative(bits[0])) bits[0] = absolute(bits[0]);
      return bits.join(' ');
    }).join(', '));
  });
  return clone.outerHTML;
})()
"#;

/// One frozen page, as listed in `index.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveEntry {
    /// The page title at freeze time (or the host, when the page
    /// had no title).
    pub title: String,
    /// The page's URL at freeze time. Empty for orphan files
    /// recovered from the directory with no index entry.
    #[serde(default)]
    pub url: String,
    /// File name inside the archives dir (never a path).
    pub filename: String,
    /// Unix seconds when the page was frozen.
    pub frozen_at_unix: u64,
    /// File size in bytes.
    #[serde(default)]
    pub size_bytes: u64,
}

impl ArchiveEntry {
    /// The archive's file path inside `dir`.
    pub fn path(&self, dir: &Path) -> PathBuf {
        dir.join(&self.filename)
    }
}

/// The archives directory: `$XDG_DATA_HOME/frost/archives/`, else
/// `~/.local/share/frost/archives/`.
///
/// Round 34: resolved by the `dirs` crate instead of hand-rolled
/// env-var chains. Same paths in every normal case (`$XDG_DATA_HOME`
/// when set and absolute, else `$HOME/.local/share`). Two degenerate
/// cases differ, both in the spec-correct direction: a *relative*
/// `XDG_DATA_HOME` is ignored (the XDG spec says relative values
/// are invalid; the old code used it verbatim), and with `HOME`
/// unset `dirs` falls back to the passwd entry instead of a relative
/// `.local/share` in the working directory.
pub fn archives_dir() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from(".local").join("share"));
    base.join("frost").join("archives")
}

/// The `index.json` path inside an archives dir.
fn index_path(dir: &Path) -> PathBuf {
    dir.join("index.json")
}

/// Sanitize a page title into a file-name stem: lowercase
/// alphanumerics and `-` only, runs of anything else collapsing to
/// one `-`, trimmed of edge dashes, capped at 40 characters.
/// Falls back to `"page"` when nothing survives.
pub fn sanitize_title(title: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
        if out.len() >= 40 {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "page".to_string()
    } else {
        out
    }
}

/// Civil date/time (UTC) for a Unix timestamp, via Howard Hinnant's
/// civil-from-days — the same algorithm as
/// `windows::format_capture_time`. Returns (year, month, day, hour,
/// minute).
fn civil_utc(unix_secs: u64) -> (i64, u32, u32, u64, u64) {
    let days = (unix_secs / 86400) as i64;
    let tod = unix_secs % 86400;
    let (hour, min) = (tod / 3600, (tod % 3600) / 60);
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
    (year, month as u32, day as u32, hour, min)
}

/// Compact UTC timestamp for file names: `YYYYMMDD-HHMM`.
pub fn timestamp_compact(unix_secs: u64) -> String {
    let (year, month, day, hour, min) = civil_utc(unix_secs);
    format!("{year:04}{month:02}{day:02}-{hour:02}{min:02}")
}

/// Readable UTC time for the archive comment: `YYYY-MM-DD HH:MM`.
fn format_frozen_time(unix_secs: u64) -> String {
    let (year, month, day, hour, min) = civil_utc(unix_secs);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}")
}

/// File-name stem for an archive: sanitized title + compact
/// timestamp, e.g. `example-org-20261116-2050`.
fn archive_stem(title: &str, unix_secs: u64) -> String {
    format!(
        "{}-{}",
        sanitize_title(title),
        timestamp_compact(unix_secs)
    )
}

/// File name for an archive: sanitized title + compact timestamp,
/// e.g. `example-org-20261116-2050.html`.
pub fn archive_filename(title: &str, unix_secs: u64) -> String {
    format!("{}.html", archive_stem(title, unix_secs))
}

/// A URL made safe for the HTML comment header: `--` would end the
/// comment early, so it is spaced out. The comment is identifying
/// text only — the URL is never parsed back out of it.
fn comment_safe(url: &str) -> String {
    url.replace("--", "- -")
}

/// Wrap captured `outerHTML` in the archive file: doctype plus an
/// identifying comment (original URL + freeze time in UTC). The
/// captured HTML passes through untouched.
pub fn build_archive_html(url: &str, captured_outer_html: &str, frozen_at_unix: u64) -> String {
    format!(
        "<!DOCTYPE html>\n<!-- Frost Deep Freeze archive of {}, frozen {} UTC. Read-only snapshot: scripts in the page run against this static DOM, and subresources re-resolve against the original URL. -->\n{}",
        comment_safe(url),
        format_frozen_time(frozen_at_unix),
        captured_outer_html
    )
}

/// The parsed archive-index cache (Round 34): `list_archives`
/// runs on every Archives-window open, and the index file does not
/// change between Frost-side writes — so the parsed index is cached
/// per directory (bounded, 60 s TTL) instead of being re-read and
/// re-parsed every time. The directory scan in `list_archives`
/// still runs every time (orphan recovery + stale pruning), so the
/// list stays honest if files are moved by hand; only the JSON
/// read/parse is cached. Frost-side writes go through `save_index`,
/// which writes the cache through, so the cache never disagrees
/// with what Frost wrote. An external edit to `index.json`
/// surfaces within the TTL (60 s) or on restart.
fn index_cache() -> &'static DiskCache<PathBuf, Vec<ArchiveEntry>> {
    static CACHE: std::sync::OnceLock<DiskCache<PathBuf, Vec<ArchiveEntry>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| DiskCache::new(64))
}

/// Load the index, through the cache: a miss reads and parses the
/// file once, a hit serves the parsed entries with no disk read.
/// (Missing or unparseable means empty — never an error page, never
/// a crash; a corrupt index just re-derives from the directory scan
/// in `list_archives`.)
fn load_index(dir: &Path) -> Vec<ArchiveEntry> {
    index_cache()
        .get_or_load(dir.to_path_buf(), || load_index_from_disk(dir))
        .as_ref()
        .clone()
}

/// The actual disk read behind `load_index`.
fn load_index_from_disk(dir: &Path) -> Vec<ArchiveEntry> {
    match std::fs::read_to_string(index_path(dir)) {
        Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Save the index (creating the directory first), writing the cache
/// through: the file now contains exactly `entries`, so the cached
/// parse is the same list.
fn save_index(dir: &Path, entries: &[ArchiveEntry]) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let json = serde_json::to_string_pretty(entries)
        .map_err(|e| format!("cannot serialize archive index: {e}"))?;
    std::fs::write(index_path(dir), json)
        .map_err(|e| format!("cannot write {}: {e}", index_path(dir).display()))?;
    index_cache().insert(dir.to_path_buf(), entries.to_vec());
    Ok(())
}

/// Save a frozen page to `dir` (creating it first) and record it in
/// the index. A file-name collision (same title frozen twice in
/// the same minute) gets a `-2`, `-3`, … suffix on the stem.
pub fn save_archive(
    dir: &Path,
    title: &str,
    url: &str,
    captured_outer_html: &str,
    now_unix: u64,
) -> Result<ArchiveEntry, String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let stem = archive_stem(title, now_unix);
    let mut filename = format!("{stem}.html");
    let mut n = 2;
    while dir.join(&filename).exists() {
        filename = format!("{stem}-{n}.html");
        n += 1;
    }
    let html = build_archive_html(url, captured_outer_html, now_unix);
    let path = dir.join(&filename);
    std::fs::write(&path, &html)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(html.len() as u64);
    let entry = ArchiveEntry {
        title: title.to_string(),
        url: url.to_string(),
        filename,
        frozen_at_unix: now_unix,
        size_bytes,
    };
    let mut entries = load_index(dir);
    entries.retain(|e| e.filename != entry.filename);
    entries.push(entry.clone());
    entries.sort_by(|a, b| {
        b.frozen_at_unix
            .cmp(&a.frozen_at_unix)
            .then_with(|| a.filename.cmp(&b.filename))
    });
    save_index(dir, &entries)?;
    Ok(entry)
}

/// List the archives in `dir`, newest first: the index entries
/// whose file still exists, plus any orphan `.html` files with no
/// index entry (title = file stem, URL unknown, freeze time = file
/// modification time, or 0 when that cannot be read).
pub fn list_archives(dir: &Path) -> Vec<ArchiveEntry> {
    let mut entries: Vec<ArchiveEntry> = load_index(dir)
        .into_iter()
        .filter(|e| dir.join(&e.filename).exists())
        .collect();
    if let Ok(read) = std::fs::read_dir(dir) {
        for item in read.flatten() {
            let path = item.path();
            let is_html = path
                .extension()
                .map(|e| e.eq_ignore_ascii_case("html"))
                .unwrap_or(false);
            if !is_html {
                continue;
            }
            let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                continue;
            };
            if entries.iter().any(|e| e.filename == name) {
                continue;
            }
            let (frozen_at_unix, size_bytes) = match std::fs::metadata(&path) {
                Ok(m) => (
                    m.modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                    m.len(),
                ),
                Err(_) => (0, 0),
            };
            let title = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "page".to_string());
            entries.push(ArchiveEntry {
                title,
                url: String::new(),
                filename: name,
                frozen_at_unix,
                size_bytes,
            });
        }
    }
    entries.sort_by(|a, b| {
        b.frozen_at_unix
            .cmp(&a.frozen_at_unix)
            .then_with(|| a.filename.cmp(&b.filename))
    });
    entries
}

/// Delete an archive's file and drop it from the index. A missing
/// file is fine (the index entry still goes); a missing index
/// entry is fine too.
pub fn delete_archive(dir: &Path, entry: &ArchiveEntry) -> Result<(), String> {
    let path = entry.path(dir);
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|e| format!("cannot delete {}: {e}", path.display()))?;
    }
    let mut entries = load_index(dir);
    let before = entries.len();
    entries.retain(|e| e.filename != entry.filename);
    if entries.len() != before {
        save_index(dir, &entries)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("frost-archive-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn sanitize_title_keeps_alphanumerics_and_dashes() {
        assert_eq!(sanitize_title("Example Domain"), "example-domain");
        assert_eq!(sanitize_title("Google — Search"), "google-search");
        assert_eq!(sanitize_title("  spaced   out  "), "spaced-out");
        assert_eq!(sanitize_title("a/b?c=d&e"), "a-b-c-d-e");
        assert_eq!(sanitize_title(""), "page");
        assert_eq!(sanitize_title("!!!"), "page");
        assert_eq!(sanitize_title("Ünïcode Tïtle"), "n-code-t-tle");
    }

    #[test]
    fn timestamp_compact_matches_known_dates() {
        // 2026-11-16 20:50 UTC.
        assert_eq!(timestamp_compact(1794862200), "20261116-2050");
        // 1970-01-01 00:00 UTC.
        assert_eq!(timestamp_compact(0), "19700101-0000");
        // 2000-02-29 12:00 UTC (leap day).
        assert_eq!(timestamp_compact(951825600), "20000229-1200");
    }

    #[test]
    fn archive_filename_combines_title_and_time() {
        assert_eq!(
            archive_filename("Example Domain", 1794862200),
            "example-domain-20261116-2050.html"
        );
        assert_eq!(
            archive_filename("", 1794862200),
            "page-20261116-2050.html"
        );
    }

    #[test]
    fn build_archive_html_keeps_the_captured_dom() {
        let html = build_archive_html(
            "https://example.org/",
            "<html><head></head><body><h1>Example Domain</h1></body></html>",
            1794862200,
        );
        assert!(html.starts_with("<!DOCTYPE html>\n<!-- Frost Deep Freeze archive"));
        assert!(html.contains("https://example.org/"));
        assert!(html.contains("<h1>Example Domain</h1>"));
        // A URL with "--" must not be able to close the comment.
        let tricky = build_archive_html("https://example.org/a--b", "<html></html>", 0);
        assert!(!tricky.contains("a--b"));
    }

    #[test]
    fn capture_script_clones_and_rewrites() {
        // Cheap guard against accidental edits: the script must work
        // on a clone (never the live DOM), insert a base tag, rewrite
        // relative src/href/srcset to absolute, and return outerHTML.
        assert!(FREEZE_CAPTURE_SCRIPT.contains("cloneNode(true)"));
        assert!(FREEZE_CAPTURE_SCRIPT.contains("createElement('base')"));
        assert!(FREEZE_CAPTURE_SCRIPT.contains("new URL(v, base)"));
        assert!(FREEZE_CAPTURE_SCRIPT.contains("[src]"));
        assert!(FREEZE_CAPTURE_SCRIPT.contains("[href]"));
        assert!(FREEZE_CAPTURE_SCRIPT.contains("[srcset]"));
        assert!(FREEZE_CAPTURE_SCRIPT.contains("clone.outerHTML"));
        // The live document must never be mutated by the script.
        assert!(!FREEZE_CAPTURE_SCRIPT.contains("document.documentElement.insertBefore"));
        assert!(!FREEZE_CAPTURE_SCRIPT.contains("document.head.appendChild"));
    }

    #[test]
    fn save_list_delete_round_trips() {
        let dir = temp_dir("roundtrip");
        let entry = save_archive(
            &dir,
            "Example Domain",
            "https://example.org/",
            "<html><head></head><body>hi</body></html>",
            1794862200,
        )
        .unwrap();
        assert_eq!(entry.filename, "example-domain-20261116-2050.html");
        assert!(entry.size_bytes > 0);
        let listed = list_archives(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], entry);
        // The file on disk is the wrapped archive.
        let text = std::fs::read_to_string(entry.path(&dir)).unwrap();
        assert!(text.contains("Frost Deep Freeze archive"));
        assert!(text.contains("<body>hi</body>"));
        delete_archive(&dir, &entry).unwrap();
        assert!(!entry.path(&dir).exists());
        assert!(list_archives(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_title_twice_in_a_minute_gets_a_suffix() {
        let dir = temp_dir("collision");
        let first = save_archive(&dir, "Example", "https://example.org/", "<html/>", 1794862200)
            .unwrap();
        let second =
            save_archive(&dir, "Example", "https://example.com/", "<html/>", 1794862200)
                .unwrap();
        assert_eq!(first.filename, "example-20261116-2050.html");
        assert_eq!(second.filename, "example-20261116-2050-2.html");
        assert_eq!(list_archives(&dir).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_recovers_orphans_and_prunes_stale_entries() {
        let dir = temp_dir("orphans");
        // An orphan file with no index entry.
        std::fs::write(dir.join("handmade-20260101-0000.html"), "<html/>").unwrap();
        // A stale index entry whose file is gone.
        let stale = ArchiveEntry {
            title: "Gone".to_string(),
            url: "https://example.org/".to_string(),
            filename: "gone-20260101-0000.html".to_string(),
            frozen_at_unix: 1767225600,
            size_bytes: 10,
        };
        save_index(&dir, &[stale]).unwrap();
        let listed = list_archives(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].filename, "handmade-20260101-0000.html");
        assert_eq!(listed[0].title, "handmade-20260101-0000");
        assert!(listed[0].url.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_index_lists_from_the_directory() {
        let dir = temp_dir("corrupt");
        std::fs::write(dir.join("index.json"), "this is not json").unwrap();
        std::fs::write(dir.join("page-20260101-0000.html"), "<html/>").unwrap();
        let listed = list_archives(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].filename, "page-20260101-0000.html");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_newest_first() {
        let dir = temp_dir("order");
        save_archive(&dir, "Old", "https://a.example/", "<html/>", 1700000000).unwrap();
        save_archive(&dir, "New", "https://b.example/", "<html/>", 1794862200).unwrap();
        let listed = list_archives(&dir);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].title, "New");
        assert_eq!(listed[1].title, "Old");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn archives_dir_prefers_xdg_data_home() {
        crate::with_env(
            &[("XDG_DATA_HOME", Some("/tmp/frost-xdg-data")), ("HOME", Some("/tmp/frost-home"))],
            || {
                assert_eq!(
                    archives_dir(),
                    PathBuf::from("/tmp/frost-xdg-data/frost/archives")
                );
            },
        );
    }

    #[test]
    fn archives_dir_falls_back_to_home_dot_local_share() {
        crate::with_env(
            &[("XDG_DATA_HOME", None), ("HOME", Some("/tmp/frost-home"))],
            || {
                assert_eq!(
                    archives_dir(),
                    PathBuf::from("/tmp/frost-home/.local/share/frost/archives")
                );
            },
        );
    }

    #[test]
    fn archives_dir_ignores_relative_xdg_data_home() {
        // The XDG spec says a relative XDG_DATA_HOME is invalid;
        // `dirs` ignores it and falls back to $HOME/.local/share,
        // where the old hand-rolled code used it verbatim.
        crate::with_env(
            &[("XDG_DATA_HOME", Some("relative/xdg")), ("HOME", Some("/tmp/frost-home"))],
            || {
                assert_eq!(
                    archives_dir(),
                    PathBuf::from("/tmp/frost-home/.local/share/frost/archives")
                );
            },
        );
    }

    #[test]
    fn archives_dir_without_home_is_absolute() {
        // With HOME unset, `dirs` falls back to the passwd entry
        // instead of the old code's relative `.local/share` in the
        // working directory: the resolved path is absolute and
        // still ends in `.local/share/frost/archives`.
        crate::with_env(&[("XDG_DATA_HOME", None), ("HOME", None)], || {
            let path = archives_dir();
            assert!(path.is_absolute());
            assert!(path.ends_with(".local/share/frost/archives"));
        });
    }

    #[test]
    fn archive_index_is_cached_between_lists() {
        // Proof the cache is consulted: after the first list, the
        // index file is deleted from disk. A re-read would recover
        // the archive as an orphan titled by its file stem; the
        // cached index still titles it "Example".
        let dir = temp_dir("cache-hit");
        save_archive(&dir, "Example", "https://example.org/", "<html/>", 1794862200).unwrap();
        let listed = list_archives(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "Example");
        std::fs::remove_file(dir.join("index.json")).unwrap();
        let listed = list_archives(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].title, "Example");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn archive_index_write_through_after_save() {
        // A Frost-side write refreshes the cache, so a later list
        // shows the new entry without a stale read.
        let dir = temp_dir("cache-write");
        save_archive(&dir, "First", "https://a.example/", "<html/>", 1794862200).unwrap();
        save_archive(&dir, "Second", "https://b.example/", "<html/>", 1794862300).unwrap();
        let listed = list_archives(&dir);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].title, "Second");
        assert_eq!(listed[1].title, "First");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn archive_index_write_through_after_delete() {
        let dir = temp_dir("cache-delete");
        let entry =
            save_archive(&dir, "Example", "https://example.org/", "<html/>", 1794862200)
                .unwrap();
        assert_eq!(list_archives(&dir).len(), 1);
        delete_archive(&dir, &entry).unwrap();
        assert!(list_archives(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
