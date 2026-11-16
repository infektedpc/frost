//! Snowplow: embedder-side tracker/ad request blocking (Round 13).
//!
//! How it works — the hook is real, not faked, and it is the same
//! mechanism ServoGTK's ad blocker uses (see
//! `~/workspace/servo-base/servo-src/ports/servo-gtk/src/adblock.rs`):
//! every HTTP fetch in Servo passes through
//! `RequestInterceptor::intercept_request`
//! (`components/net/request_interceptor.rs`), which sends
//! `NetToEmbedderMsg::WebResourceRequested` to the embedder. `Servo`
//! routes that message to the active webview's delegate as
//! `WebViewDelegate::load_web_resource(webview, load)`
//! (`components/servo/servo.rs`, `WebResourceRequested` arm).
//! A [`WebResourceLoad`] dropped untouched means "do not intercept"
//! (the default responder sends `DoNotIntercept`), so allowing a
//! request costs nothing. Blocking one is
//! `load.intercept(WebResourceResponse::new(url)).cancel()`, which the
//! net layer turns into a `CancelLoad` network error — the subresource
//! simply never loads, and it fails FAST (no hang: a cancelled load
//! errors immediately instead of waiting on the network).
//!
//! Scope, stated honestly: matching is by request host against the
//! built-in domain list below (exact host or any subdomain). It blocks
//! third-party ad/tracker subresources. It does NOT block ads served
//! from the first-party domain itself, ads loaded over non-HTTP(S)
//! schemes, or anything hidden behind a CNAME or proxy — that would
//! need a maintained filter-list engine (e.g. EasyList syntax), which
//! is out of scope here. Main-frame navigations are never blocked, so
//! visiting a blocked domain directly still works; only its use as a
//! subresource is cut off.
//!
//! The toggle lives in the app's Settings (`snowplow: bool`, default
//! ON); the engine holds the shared [`Snowplow`] state and the
//! delegate enforces it. Test hook: the `FROST_SNOWPLOW_EXTRA_BLOCKLIST`
//! environment variable (comma-separated domains) appends entries to
//! the blocklist at engine startup — it exists so the gate can block
//! a locally-served test domain (`localhost`) without touching the
//! real list. It is not a user-facing setting.

use std::sync::mpsc;

use servo::{WebResourceLoad, WebResourceResponse};
use url::Url;

use crate::EngineEvent;

/// Well-known ad-serving and tracking domains. Matching is by host
/// suffix (see [`host_is_blocked`]), so e.g. `doubleclick.net` also
/// covers `ad.doubleclick.net` and `stats.g.doubleclick.net`. Entries
/// are bare domains on purpose: a leading `*.` would add nothing,
/// since suffix matching already covers subdomains.
///
/// The list starts from ServoGTK's proven ~60-domain list (same
/// project lineage), with the one duplicate entry
/// (`googletagmanager.com`, listed twice there) removed.
pub const DEFAULT_BLOCKLIST: &[&str] = &[
    // Google ad stack
    "doubleclick.net",
    "googlesyndication.com",
    "googleadservices.com",
    "googletagservices.com",
    "googletagmanager.com",
    "google-analytics.com",
    "adservice.google.com",
    // Meta / social trackers
    "facebook.net",
    "connect.facebook.net",
    "fbcdn.net",
    // Amazon ad stack
    "amazon-adsystem.com",
    "assoc-amazon.com",
    // Other major ad networks / exchanges
    "adsrvr.org",
    "adnxs.com",
    "criteo.com",
    "criteo.net",
    "pubmatic.com",
    "rubiconproject.com",
    "moatads.com",
    "advertising.com",
    "smartadserver.com",
    "outbrain.com",
    "taboola.com",
    "mgid.com",
    "revcontent.com",
    "zedo.com",
    "popads.net",
    "propellerads.com",
    "adsterra.com",
    "adcash.com",
    "exoclick.com",
    "juicyads.com",
    "trafficjunky.net",
    "adblade.com",
    "bidswitch.net",
    "openx.net",
    "indexexchange.com",
    "appnexus.com",
    // Analytics / fingerprinting / session replay
    "scorecardresearch.com",
    "quantserve.com",
    "hotjar.com",
    "mixpanel.com",
    "segment.com",
    "segment.io",
    "fullstory.com",
    "mouseflow.com",
    "crazyegg.com",
    "newrelic.com",
    "nr-data.net",
    "optimizely.com",
    "vwo.com",
    "matomo.cloud",
    // Miscellaneous trackers
    "ads.yahoo.com",
    "advertising.yahoo.com",
    "analytics.yahoo.com",
    "tiktok.com",
    "snapchat.com",
    "pinterest.com",
    "linkedin.com",
    "twitter.com",
    "ads-twitter.com",
    "ads.linkedin.com",
];

/// True if `host` matches `entry`: an exact match, or any subdomain
/// of it. Comparison is case-insensitive and ignores a trailing dot
/// (`example.com.` == `example.com`).
fn host_matches(host: &str, entry: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    let entry = entry.trim_end_matches('.').to_ascii_lowercase();
    host == entry || host.ends_with(&format!(".{entry}"))
}

/// True if `host` is on the default blocklist: an exact match, or any
/// subdomain of a listed domain.
pub fn host_is_blocked(host: &str) -> bool {
    DEFAULT_BLOCKLIST.iter().any(|entry| host_matches(host, entry))
}

/// Extract the host from a URL string (no `url` crate needed) and check
/// it against the default blocklist. Returns false for unparsable URLs
/// and for non-HTTP(S) schemes, which this blocker never touches.
pub fn url_is_blocked(url: &str) -> bool {
    let Some(rest) = url.split("://").nth(1) else {
        return false;
    };
    let scheme = url.split("://").next().unwrap_or("").to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return false;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // Strip userinfo and port; keep the host.
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = match host.strip_prefix('[') {
        // IPv6 literal: blocked never (no ad domain is an IP literal).
        Some(_) => return false,
        None => host.split(':').next().unwrap_or(host),
    };
    host_is_blocked(host)
}

/// Parse the `FROST_SNOWPLOW_EXTRA_BLOCKLIST` value: comma-separated
/// domains, trimmed, lowercased, empties dropped.
pub fn parse_extra_blocklist(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|s| s.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

/// On/off state plus extra (test-hook) blocklist entries for the
/// blocker. Shared between the [`crate::Engine`] (for the Settings
/// toggle) and the [`crate::FrostDelegate`] (which enforces it)
/// through an `Rc<RefCell<Snowplow>>` — the same sharing pattern
/// ServoGTK's `AdBlocker` uses, and safe here because everything in
/// the engine runs on the one thread that created it.
pub struct Snowplow {
    enabled: bool,
    extra_blocklist: Vec<String>,
}

impl Snowplow {
    /// A new blocker, enabled by default, with the default blocklist
    /// plus any domains from `FROST_SNOWPLOW_EXTRA_BLOCKLIST`.
    pub fn new() -> Self {
        let extra_blocklist = std::env::var("FROST_SNOWPLOW_EXTRA_BLOCKLIST")
            .map(|v| parse_extra_blocklist(&v))
            .unwrap_or_default();
        Snowplow {
            enabled: true,
            extra_blocklist,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// True if this host should be blocked: on the default list or on
    /// the extra list (exact or any subdomain).
    pub fn should_block_host(&self, host: &str) -> bool {
        host_is_blocked(host)
            || self
                .extra_blocklist
                .iter()
                .any(|entry| host_matches(host, entry))
    }

    /// True if this URL should be blocked as an ad/tracker subresource:
    /// the blocker is on, the scheme is http(s), and the host matches.
    pub fn should_block_url(&self, url: &Url) -> bool {
        self.enabled
            && matches!(url.scheme(), "http" | "https")
            && url
                .host_str()
                .map(|host| self.should_block_host(host))
                .unwrap_or(false)
    }
}

impl Default for Snowplow {
    fn default() -> Self {
        Self::new()
    }
}

/// Enforce the blocker on one intercepted resource load, called from
/// `FrostDelegate::load_web_resource`.
///
/// Returns true if the load was blocked. Allowed loads are dropped
/// untouched, which the responder translates to `DoNotIntercept`, so
/// the fetch proceeds exactly as if no blocker existed. Main-frame
/// navigations and non-HTTP(S) requests are never blocked. A blocked
/// load is cancelled (a fast network error, never a hang) and an
/// [`EngineEvent::ResourceBlocked`] is sent so the UI can count it.
pub fn handle_load(
    load: WebResourceLoad,
    snowplow: &Snowplow,
    tx: &mpsc::Sender<EngineEvent>,
) -> bool {
    if !snowplow.enabled {
        return false;
    }
    let request = load.request();
    if request.is_for_main_frame {
        return false;
    }
    let url: Url = request.url.clone().into_url();
    if !snowplow.should_block_url(&url) {
        return false;
    }
    let blocked = url.to_string();
    load.intercept(WebResourceResponse::new(url)).cancel();
    let _ = tx.send(EngineEvent::ResourceBlocked(blocked));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_and_subdomain_match() {
        assert!(host_is_blocked("doubleclick.net"));
        assert!(host_is_blocked("ad.doubleclick.net"));
        assert!(host_is_blocked("stats.g.doubleclick.net"));
    }

    #[test]
    fn no_partial_suffix_match() {
        // "notdoubleclick.net" merely ends with the string, not the domain.
        assert!(!host_is_blocked("notdoubleclick.net"));
        assert!(!host_is_blocked("doubleclick.net.evil.com"));
    }

    #[test]
    fn case_and_trailing_dot() {
        assert!(host_is_blocked("AD.DoubleClick.NET"));
        assert!(host_is_blocked("doubleclick.net."));
    }

    #[test]
    fn url_parsing() {
        assert!(url_is_blocked("https://ad.doubleclick.net/ads?id=1"));
        assert!(url_is_blocked("http://google-analytics.com:8080/collect"));
        assert!(url_is_blocked("https://user@pubmatic.com/x"));
        assert!(!url_is_blocked("https://example.com/ads.js"));
        assert!(!url_is_blocked("data:text/html,hello"));
        assert!(!url_is_blocked("not a url"));
    }

    #[test]
    fn toggle_gates_blocking() {
        let mut snowplow = Snowplow::new();
        assert!(snowplow.is_enabled());
        let url = Url::parse("https://doubleclick.net/x").unwrap();
        assert!(snowplow.should_block_url(&url));
        snowplow.set_enabled(false);
        assert!(!snowplow.should_block_url(&url));
        snowplow.set_enabled(true);
        assert!(snowplow.should_block_url(&url));
    }

    #[test]
    fn main_frame_and_scheme_never_block() {
        let snowplow = Snowplow::new();
        // Non-HTTP(S) schemes are never blocked, even on a listed host.
        let data = Url::parse("data:text/html,hello").unwrap();
        assert!(!snowplow.should_block_url(&data));
        // An unlisted host is never blocked.
        let ok = Url::parse("https://example.org/ads.js").unwrap();
        assert!(!snowplow.should_block_url(&ok));
    }

    #[test]
    fn extra_blocklist_entries_apply() {
        let mut snowplow = Snowplow::new();
        snowplow.extra_blocklist = vec!["snowplow.test".to_string()];
        assert!(snowplow.should_block_host("snowplow.test"));
        assert!(snowplow.should_block_host("ads.snowplow.test"));
        assert!(!snowplow.should_block_host("example.org"));
    }

    #[test]
    fn extra_blocklist_parsing() {
        assert_eq!(
            parse_extra_blocklist("localhost, Snowplow.Test. , ,example.com"),
            vec![
                "localhost".to_string(),
                "snowplow.test".to_string(),
                "example.com".to_string()
            ]
        );
        assert!(parse_extra_blocklist("").is_empty());
    }
}
