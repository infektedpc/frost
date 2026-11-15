//! Frost settings: theme, homepage, search engine, exit confirmation.
//!
//! Persisted as JSON under the user's config dir
//! (`$XDG_CONFIG_HOME/frost/settings.json`, else
//! `~/.config/frost/settings.json`). Round 3: the settings page
//! (`frost://settings`, served by the loopback server) edits these; the
//! app applies the theme and search engine live and the homepage on
//! next launch. A missing or unparseable file means defaults — never an
//! error page, never a crash.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Light or dark. The default is Dark (Frost's Round 1/2 look).
///
/// Implementation note: libcosmic's global theme is `pub(crate)` with no
/// public setter, so Frost cannot switch the toolkit theme at runtime.
/// Instead the app themes its OWN chrome explicitly (see
/// `ThemeChoice::chrome_colors`, applied to the titlebar / toolbar /
/// menu / status containers in the view) and renders its internal pages
/// with matching light/dark CSS, and tells the page via
/// `Engine::set_theme` (Servo maps it onto `prefers-color-scheme`).
/// The libcosmic buttons and text field keep the global (dark) styling
/// in both modes — readable on either chrome, just not recolored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeChoice {
    Dark,
    Light,
}

impl ThemeChoice {
    /// Chrome colors for Frost's own titlebar / toolbar / menu / status.
    pub fn chrome_colors(&self) -> ChromeColors {
        match self {
            ThemeChoice::Dark => ChromeColors {
                background: [43, 42, 51],
                text: [253, 251, 255],
                muted: [168, 168, 178],
                menu_hover: [64, 63, 73],
                menu_border: [110, 109, 124],
            },
            ThemeChoice::Light => ChromeColors {
                background: [242, 242, 245],
                text: [26, 26, 26],
                muted: [92, 92, 102],
                menu_hover: [222, 222, 226],
                menu_border: [190, 190, 196],
            },
        }
    }

    pub fn is_dark(&self) -> bool {
        matches!(self, ThemeChoice::Dark)
    }
}

/// Explicit chrome colors (0–255 RGB) for one theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChromeColors {
    pub background: [u8; 3],
    pub text: [u8; 3],
    pub muted: [u8; 3],
    /// Hover fill for one hamburger-menu entry (Round 8).
    pub menu_hover: [u8; 3],
    /// Panel border and separator lines of the hamburger menu (Round 8).
    pub menu_border: [u8; 3],
}

/// Default search engine, used when the address bar gets a search query
/// instead of a URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchEngine {
    DuckDuckGo,
    Google,
    Brave,
    Bing,
}

impl SearchEngine {
    pub fn all() -> [SearchEngine; 4] {
        [
            SearchEngine::DuckDuckGo,
            SearchEngine::Google,
            SearchEngine::Brave,
            SearchEngine::Bing,
        ]
    }

    pub fn label(&self) -> &'static str {
        match self {
            SearchEngine::DuckDuckGo => "DuckDuckGo",
            SearchEngine::Google => "Google",
            SearchEngine::Brave => "Brave Search",
            SearchEngine::Bing => "Bing",
        }
    }

    /// Search URL for a raw query string.
    pub fn search_url(&self, query: &str) -> String {
        let encoded = percent_encode_query(query);
        match self {
            SearchEngine::DuckDuckGo => format!("https://duckduckgo.com/?q={encoded}"),
            SearchEngine::Google => format!("https://www.google.com/search?q={encoded}"),
            SearchEngine::Brave => format!("https://search.brave.com/search?q={encoded}"),
            SearchEngine::Bing => format!("https://www.bing.com/search?q={encoded}"),
        }
    }
}

/// Percent-encode a search query: alphanumerics and `-_.~` pass through,
/// everything else becomes `%XX` (space becomes `%20`).
pub fn percent_encode_query(query: &str) -> String {
    let mut out = String::with_capacity(query.len());
    for b in query.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            },
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Whether a homepage value is a loadable http(s) URL. (Round 7: moved
/// here from the deleted loopback server, which used it to validate the
/// settings page's homepage field.)
pub fn is_loadable_homepage(value: &str) -> bool {
    match url::Url::parse(value) {
        Ok(u) => matches!(u.scheme(), "http" | "https") && u.host_str().is_some(),
        Err(_) => false,
    }
}


#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub theme: ThemeChoice,
    pub homepage: String,
    pub search_engine: SearchEngine,
    /// Whether the menu's Exit entry opens the confirmation dialog
    /// (true) or quits immediately (false).
    pub confirm_exit: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: ThemeChoice::Dark,
            homepage: "https://example.org".to_string(),
            search_engine: SearchEngine::DuckDuckGo,
            confirm_exit: true,
        }
    }
}

impl Settings {
    /// The settings file path: `$XDG_CONFIG_HOME/frost/settings.json`,
    /// else `~/.config/frost/settings.json`.
    pub fn config_path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from(".config"));
        base.join("frost").join("settings.json")
    }

    /// Load from the config path; missing or unparseable means defaults.
    pub fn load() -> Settings {
        Self::load_from(&Self::config_path())
    }

    pub fn load_from(path: &std::path::Path) -> Settings {
        match std::fs::read_to_string(path) {
            Ok(json) => match serde_json::from_str::<Settings>(&json) {
                Ok(settings) => settings,
                Err(e) => {
                    eprintln!("frost: settings file {path:?} is unparseable ({e}); using defaults");
                    Settings::default()
                },
            },
            Err(_) => Settings::default(),
        }
    }

    /// Save to a path (creating the parent directory).
    pub fn save_to(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("cannot serialize settings: {e}"))?;
        std::fs::write(path, json).map_err(|e| format!("cannot write {}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_urls_encode_queries() {
        assert_eq!(
            SearchEngine::DuckDuckGo.search_url("hello world"),
            "https://duckduckgo.com/?q=hello%20world"
        );
        assert_eq!(
            SearchEngine::Google.search_url("rust & servo"),
            "https://www.google.com/search?q=rust%20%26%20servo"
        );
        assert_eq!(
            SearchEngine::Brave.search_url("x"),
            "https://search.brave.com/search?q=x"
        );
        assert_eq!(
            SearchEngine::Bing.search_url("x"),
            "https://www.bing.com/search?q=x"
        );
    }

    #[test]
    fn query_percent_codec_round_trips() {
        assert_eq!(percent_encode_query("hello world!"), "hello%20world%21");
        assert_eq!(percent_encode_query("a/b"), "a%2Fb");
        assert_eq!(percent_encode_query("plain"), "plain");
    }

    #[test]
    fn loadable_homepage_needs_http_host() {
        assert!(is_loadable_homepage("https://example.org"));
        assert!(is_loadable_homepage("http://example.com/page"));
        assert!(!is_loadable_homepage("https://"));
        assert!(!is_loadable_homepage("frost://settings"));
        assert!(!is_loadable_homepage("example.org"));
        assert!(!is_loadable_homepage("file:///home/user/index.html"));
        assert!(!is_loadable_homepage(""));
    }

    #[test]
    fn settings_round_trip_through_a_file() {
        let path = std::env::temp_dir().join("frost-settings-test.json");
        let _ = std::fs::remove_file(&path);
        let mut s = Settings::default();
        s.theme = ThemeChoice::Light;
        s.homepage = "https://example.com".to_string();
        s.search_engine = SearchEngine::Bing;
        s.confirm_exit = false;
        s.save_to(&path).unwrap();
        let back = Settings::load_from(&path);
        assert_eq!(back, s);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn settings_load_missing_file_is_defaults() {
        let path = std::env::temp_dir().join("frost-settings-test-missing.json");
        let _ = std::fs::remove_file(&path);
        assert_eq!(Settings::load_from(&path), Settings::default());
    }

    #[test]
    fn theme_chrome_colors_differ() {
        let dark = ThemeChoice::Dark.chrome_colors();
        let light = ThemeChoice::Light.chrome_colors();
        assert_ne!(dark.background, light.background);
        assert_ne!(dark.text, light.text);
        assert_eq!(dark.background, [0x2b, 0x2a, 0x33]);
        assert_eq!(light.background, [0xf2, 0xf2, 0xf5]);
    }
}
