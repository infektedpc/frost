//! frost-extensions: extension host scaffolding for the Frost browser.
//!
//! Round 1 is deliberately a *shape* crate: it defines the manifest format,
//! the content-script model, the `frost.*` JS API bridge trait, and an
//! in-memory stub host — so that later rounds can wire real extensions into
//! the engine without rewriting the browser around them.
//!
//! Design decisions (locked in now, so no huge rewrite later):
//!
//! * Extensions are described by a JSON manifest (name, version,
//!   permissions, content scripts, optional background script).
//! * Content scripts are matched against page URLs with Chrome-style match
//!   patterns (`*://*.example.com/*`); see [`matches_pattern`].
//! * The `frost.*` bridge ([`FrostApi`]) is the ONLY way extension JS talks
//!   to the browser. Page JS never sees it; content scripts get a scoped
//!   bridge bound to their extension id. The bridge is a Rust trait so the
//!   real implementation can be swapped in without touching callers.
//! * [`ExtensionHost`] is a trait; [`StubHost`] is the Round 1 in-memory
//!   implementation. The real host (Round 3+) will drive content-script
//!   injection through `frost-engine`'s script evaluation hook.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Opaque id handed out by the host when an extension is loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExtensionId(pub u64);

/// A loaded extension as the host sees it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadedExtension {
    pub id: ExtensionId,
    pub manifest: ExtensionManifest,
    /// Whether the extension is currently enabled (disabled extensions keep
    /// their content scripts inert).
    pub enabled: bool,
}

/// The extension manifest. This is the format extension authors write;
/// it is intentionally close to the WebExtensions `manifest.json` shape
/// minus the parts Servo will never have (background pages, browser
/// action popups), so authors can port existing extensions with small edits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionManifest {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    #[serde(default)]
    pub content_scripts: Vec<ContentScript>,
    pub background: Option<BackgroundScript>,
}

impl ExtensionManifest {
    /// Parse a manifest from its JSON text.
    pub fn from_json(json: &str) -> Result<Self, ExtensionError> {
        serde_json::from_str(json).map_err(|e| ExtensionError::InvalidManifest(e.to_string()))
    }

    /// Minimal validation: name/version present, every content script has at
    /// least one match pattern and non-empty JS.
    pub fn validate(&self) -> Result<(), ExtensionError> {
        if self.name.trim().is_empty() {
            return Err(ExtensionError::InvalidManifest(
                "manifest name must not be empty".into(),
            ));
        }
        if self.version.trim().is_empty() {
            return Err(ExtensionError::InvalidManifest(
                "manifest version must not be empty".into(),
            ));
        }
        for (i, script) in self.content_scripts.iter().enumerate() {
            if script.matches.is_empty() {
                return Err(ExtensionError::InvalidManifest(format!(
                    "content script {i} has no match patterns"
                )));
            }
            if script.js.trim().is_empty() {
                return Err(ExtensionError::InvalidManifest(format!(
                    "content script {i} has empty JS"
                )));
            }
        }
        Ok(())
    }
}

/// Permissions an extension may request. Host permissions are match
/// patterns, exactly like content-script matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Permission {
    /// Read tab URLs/titles (never page content — that needs a content script).
    Tabs,
    /// `frost.storage` key/value store, namespaced per extension.
    Storage,
    /// Observe/modify network requests (Snowplow blocking is built on this).
    WebRequest,
    /// Read/write the clipboard via the browser, never directly.
    Clipboard,
    /// Show desktop notifications.
    Notifications,
    /// A host match pattern, e.g. `*://*.example.com/*`.
    Host(String),
}

/// A content script: JS injected into matching pages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentScript {
    /// Chrome-style match patterns.
    pub matches: Vec<String>,
    /// The JS source to inject. Round 1 stores source inline; a later round
    /// may allow `"js_file": "content.js"` referencing a packaged file.
    pub js: String,
    pub run_at: RunAt,
    #[serde(default)]
    pub all_frames: bool,
}

/// When a content script runs relative to page load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunAt {
    DocumentStart,
    DocumentEnd,
    DocumentIdle,
}

/// An optional background script (event-page style: runs once at browser
/// start, sleeps between events; `persistent: false` is the default and
/// the only mode Frost will ever support — no always-on background pages).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundScript {
    pub js: String,
    #[serde(default)]
    pub persistent: bool,
}

/// Minimal info about a tab, as exposed to extensions through `frost.tabs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabInfo {
    pub id: u32,
    pub url: String,
    pub title: Option<String>,
}

/// The `frost.*` JS API bridge. Content scripts and background scripts call
/// into the browser through this trait — never through raw engine access.
/// The Round 1 implementation is a stub; the real one (Round 3+) will be
/// backed by the engine and the tab strip.
pub trait FrostApi {
    /// `frost.storage.get(extension_id, key)`.
    fn storage_get(&self, extension_id: ExtensionId, key: &str) -> Option<String>;
    /// `frost.storage.set(extension_id, key, value)`.
    fn storage_set(&mut self, extension_id: ExtensionId, key: &str, value: String);
    /// `frost.tabs.query()` — the tabs this extension may see.
    fn tabs_query(&self) -> Vec<TabInfo>;
    /// `frost.tabs.create(url)` — open a new tab; returns its id.
    fn tabs_create(&mut self, url: &str) -> u32;
}

/// A no-op [`FrostApi`] used by tests and by the stub host. Storage lives in
/// memory; tabs are faked with a single about:blank tab.
#[derive(Debug, Default)]
pub struct StubFrostApi {
    storage: HashMap<(u64, String), String>,
    next_tab: u32,
}

impl FrostApi for StubFrostApi {
    fn storage_get(&self, extension_id: ExtensionId, key: &str) -> Option<String> {
        self.storage.get(&(extension_id.0, key.to_string())).cloned()
    }

    fn storage_set(&mut self, extension_id: ExtensionId, key: &str, value: String) {
        self.storage.insert((extension_id.0, key.to_string()), value);
    }

    fn tabs_query(&self) -> Vec<TabInfo> {
        vec![TabInfo {
            id: 0,
            url: "about:blank".to_string(),
            title: None,
        }]
    }

    fn tabs_create(&mut self, _url: &str) -> u32 {
        let id = self.next_tab;
        self.next_tab += 1;
        id
    }
}

/// Errors from the extension host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionError {
    InvalidManifest(String),
    NotFound(ExtensionId),
    AlreadyLoaded(String),
}

impl std::fmt::Display for ExtensionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtensionError::InvalidManifest(msg) => write!(f, "invalid manifest: {msg}"),
            ExtensionError::NotFound(id) => write!(f, "no extension with id {}", id.0),
            ExtensionError::AlreadyLoaded(name) => {
                write!(f, "extension {name} is already loaded")
            }
        }
    }
}

impl std::error::Error for ExtensionError {}

/// The extension host: load/unload extensions and answer "which content
/// scripts run on this URL?". The engine calls `content_scripts_for` on
/// every navigation; the host never touches the DOM itself.
pub trait ExtensionHost {
    fn load(&mut self, manifest: ExtensionManifest) -> Result<ExtensionId, ExtensionError>;
    fn unload(&mut self, id: ExtensionId) -> Result<(), ExtensionError>;
    fn set_enabled(&mut self, id: ExtensionId, enabled: bool) -> Result<(), ExtensionError>;
    /// Content scripts (with their owning extension id) that apply to `url`,
    /// in load order. Disabled extensions contribute nothing.
    fn content_scripts_for(&self, url: &str) -> Vec<(ExtensionId, &ContentScript)>;
    fn extensions(&self) -> Vec<&LoadedExtension>;
}

/// Round 1 in-memory host. No disk I/O, no JS execution — that comes when
/// the engine gains a script-evaluation hook.
#[derive(Debug, Default)]
pub struct StubHost {
    loaded: HashMap<u64, LoadedExtension>,
    next_id: u64,
}

impl StubHost {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ExtensionHost for StubHost {
    fn load(&mut self, manifest: ExtensionManifest) -> Result<ExtensionId, ExtensionError> {
        manifest.validate()?;
        if self.loaded.values().any(|e| e.manifest.name == manifest.name) {
            return Err(ExtensionError::AlreadyLoaded(manifest.name.clone()));
        }
        let id = ExtensionId(self.next_id);
        self.next_id += 1;
        self.loaded.insert(
            id.0,
            LoadedExtension {
                id,
                manifest,
                enabled: true,
            },
        );
        Ok(id)
    }

    fn unload(&mut self, id: ExtensionId) -> Result<(), ExtensionError> {
        self.loaded
            .remove(&id.0)
            .map(|_| ())
            .ok_or(ExtensionError::NotFound(id))
    }

    fn set_enabled(&mut self, id: ExtensionId, enabled: bool) -> Result<(), ExtensionError> {
        self.loaded
            .get_mut(&id.0)
            .map(|e| e.enabled = enabled)
            .ok_or(ExtensionError::NotFound(id))
    }

    fn content_scripts_for(&self, url: &str) -> Vec<(ExtensionId, &ContentScript)> {
        let mut out = Vec::new();
        for ext in self.loaded.values() {
            if !ext.enabled {
                continue;
            }
            for script in &ext.manifest.content_scripts {
                if script.matches.iter().any(|p| matches_pattern(p, url)) {
                    out.push((ext.id, script));
                }
            }
        }
        out
    }

    fn extensions(&self) -> Vec<&LoadedExtension> {
        self.loaded.values().collect()
    }
}

/// Chrome-style match-pattern test.
///
/// Supported forms (enough for Round 1; a full implementation follows the
/// MDN match-pattern grammar in a later round):
///
/// * `*://*.example.com/*` — any scheme, any sub-domain of example.com
/// * `*://example.com/*` — any scheme, exactly example.com
/// * `https://example.com/path/*` — exact scheme/host, path prefix
/// * `*://*/*` — everything
/// * `<all_urls>` — everything
///
/// The `*` in a segment matches any run of characters within that segment.
/// Scheme `*` matches `http` and `https` only.
pub fn matches_pattern(pattern: &str, url: &str) -> bool {
    if pattern == "<all_urls>" {
        return true;
    }
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let Some((scheme_pat, rest)) = pattern.split_once("://") else {
        return false;
    };
    if scheme_pat != "*" && scheme_pat != parsed.scheme() {
        return false;
    }
    if scheme_pat == "*" && !matches!(parsed.scheme(), "http" | "https") {
        return false;
    }
    let Some((host_pat, path_pat)) = rest.split_once('/') else {
        return false;
    };
    let path_pat = format!("/{path_pat}");
    let host = parsed.host_str().unwrap_or("");
    let host_ok = if let Some(suffix) = host_pat.strip_prefix("*.") {
        host == suffix || host.ends_with(&format!(".{suffix}"))
    } else if host_pat == "*" {
        true
    } else {
        host_pat == host
    };
    if !host_ok {
        return false;
    }
    let path = parsed.path();
    glob_match(&path_pat, path)
}

/// `*` matches any run of characters; everything else is literal.
fn glob_match(pattern: &str, text: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let txt: Vec<char> = text.chars().collect();
    glob_match_inner(&pat, &txt)
}

fn glob_match_inner(pat: &[char], txt: &[char]) -> bool {
    if pat.is_empty() {
        return txt.is_empty();
    }
    if pat[0] == '*' {
        // `*` eats zero or more characters.
        for i in 0..=txt.len() {
            if glob_match_inner(&pat[1..], &txt[i..]) {
                return true;
            }
        }
        return false;
    }
    if txt.is_empty() || pat[0] != txt[0] {
        return false;
    }
    glob_match_inner(&pat[1..], &txt[1..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest() -> ExtensionManifest {
        ExtensionManifest {
            name: "Frost Test".into(),
            version: "0.1.0".into(),
            description: Some("test extension".into()),
            permissions: vec![Permission::Storage, Permission::Host("*://*.example.com/*".into())],
            content_scripts: vec![ContentScript {
                matches: vec!["*://*.example.com/*".into()],
                js: "document.title = 'frost';".into(),
                run_at: RunAt::DocumentIdle,
                all_frames: false,
            }],
            background: None,
        }
    }

    #[test]
    fn manifest_round_trips_through_json() {
        let m = sample_manifest();
        let json = serde_json::to_string(&m).unwrap();
        let back = ExtensionManifest::from_json(&json).unwrap();
        assert_eq!(back.name, "Frost Test");
        assert_eq!(back.content_scripts.len(), 1);
        assert!(back.validate().is_ok());
    }

    #[test]
    fn manifest_validation_rejects_empty_name_and_empty_script() {
        let mut m = sample_manifest();
        m.name = "  ".into();
        assert!(m.validate().is_err());
        let mut m = sample_manifest();
        m.content_scripts[0].js = String::new();
        assert!(m.validate().is_err());
    }

    #[test]
    fn stub_host_loads_and_matches_content_scripts() {
        let mut host = StubHost::new();
        let id = host.load(sample_manifest()).unwrap();
        let scripts = host.content_scripts_for("https://sub.example.com/page");
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].0, id);
        assert!(host.content_scripts_for("https://other.org/").is_empty());
        // Disabled extensions contribute nothing.
        host.set_enabled(id, false).unwrap();
        assert!(host.content_scripts_for("https://sub.example.com/page").is_empty());
        host.unload(id).unwrap();
        assert!(host.unload(id).is_err());
    }

    #[test]
    fn stub_host_rejects_duplicate_names() {
        let mut host = StubHost::new();
        host.load(sample_manifest()).unwrap();
        assert!(matches!(
            host.load(sample_manifest()),
            Err(ExtensionError::AlreadyLoaded(_))
        ));
    }

    #[test]
    fn match_patterns_behave() {
        assert!(matches_pattern("*://*.example.com/*", "https://a.b.example.com/x"));
        assert!(matches_pattern("*://*.example.com/*", "https://example.com/"));
        assert!(!matches_pattern("*://*.example.com/*", "https://example.org/"));
        assert!(matches_pattern("https://example.com/docs/*", "https://example.com/docs/a"));
        assert!(!matches_pattern("https://example.com/docs/*", "http://example.com/docs/a"));
        assert!(!matches_pattern("https://example.com/docs/*", "https://example.com/other"));
        assert!(matches_pattern("*://*/*", "https://anything.example/"));
        assert!(matches_pattern("<all_urls>", "https://anything.example/"));
        assert!(!matches_pattern("*://*.example.com/*", "not a url"));
    }

    #[test]
    fn stub_frost_api_storage_is_namespaced_per_extension() {
        let mut api = StubFrostApi::default();
        api.storage_set(ExtensionId(1), "k", "v1".into());
        api.storage_set(ExtensionId(2), "k", "v2".into());
        assert_eq!(api.storage_get(ExtensionId(1), "k").as_deref(), Some("v1"));
        assert_eq!(api.storage_get(ExtensionId(2), "k").as_deref(), Some("v2"));
        assert_eq!(api.tabs_create("https://example.org"), 0);
    }
}
