# Round 25 — DIAGNOSIS: the shared JS-feed gap (YouTube homepage + Yahoo homepage)

Diagnosis round, 2026-11-18. No behaviour fix ships in this round.
No code change to Frost or Freezer was needed to see the cause — the
diagnostic tool is a content-script extension,
`extensions/r25-errcap/` ("R25 Error Capture"), which injects into
`*.youtube.com` and `*.yahoo.com` pages and surfaces the page's own
JS errors, unhandled rejections, console output, and every fetch/XHR
URL in an on-page banner + `document.title`. It is shipped in the
tree so Round 26 can reuse it. There is no F-017: the engine was not
touched.

## What the user sees

- YouTube homepage: blank at 20 s, skeleton video tiles at 45 s,
  header at 90 s, feed area empty forever. Confirmed on Joel's box
  and in the sandbox (R24 tree, Snowplow ON and OFF).
- Yahoo homepage: with Snowplow OFF it renders fully styled with a
  real hero story, but the "For You" feed area still shows empty
  boxes. With Snowplow ON it renders unstyled raw HTML (R24) or
  fully styled (Round 25 re-run) — the feed area is empty either way.

## The feed request is never even attempted

- `curl` of youtube.com (976,437 B): `ytInitialData` IS present,
  `richGridRenderer` ×2, but `videoRenderer` 0, `videoId` 0 — the
  feed data is NOT in the initial HTML. It must come from the
  `youtubei/v1/browse` POST. That request fires **0 times** in every
  sandbox run (ON and OFF, 90–240 s).
- Yahoo HTML (690,899 B) DOES contain hero text server-side
  ("Strait of Hormuz" ×3, "Nvidia rules out" ×4, `ForYou` ×48) —
  Yahoo's feed is partly server-rendered, but its client "For You"
  area still stalls.
- The errcap extension on YouTube (180 s, Snowplow OFF): the page's
  JS **does** make API calls — a `youtubei/v1/player` POST via fetch
  and a `googleads.g.doubleclick.net/pagead/id` XHR — but never the
  `browse` call. On Yahoo (60 s): the extension is live, no errors
  captured. (Yahoo 120/180 s errcap shots were lost when the sandbox
  service restarted mid-run and killed Frost + Xvfb; the 60 s shot
  is the evidence.)

So the app is alive, fetching, and calling APIs — it just never
issues the one call the feed needs. The bootstrap step that triggers
the feed request never runs, or runs and silently does nothing.

## What is NOT the cause (refuted or ruled out)

- **Script-thread starvation (the R24 watch-page mechanism).**
  Refuted as the feed cause. Local probe p4: timers + rAF run fine
  (after 5 s: ticks 5, frames 298, chain 10) — the script thread is
  NOT starved on a local page. Yahoo (Snowplow ON, 240 s): Script#2
  CPU 25% at 60 s, 16% at 120 s, gone from the top list at 240 s —
  active then idle, never saturated; network activity continued to
  03:33:54Z. The R24 stall is a *media-event* delivery block; the
  feed gap is a different mechanism.
- **fetch / XHR / Promise broken.** Local probes pass: p1 fetch GET +
  render 6 cards; p5 skeleton→fetch→cards (the YouTube pattern);
  p6 XHR + render 6 cards; p8 POST fetch (see open item below).
  On the real pages, fetch and XHR demonstrably work (v1/player,
  googleads, `log_event` telemetry POSTs all go out).
- **Snowplow.** Ruled out for YouTube: the OFF run's 45 s skeleton
  shot is byte-identical to the blocker-on run, `browse` still 0.
  For Yahoo the OFF run renders better, but the feed is still empty,
  and the ON run's log shows zero Snowplow block lines in R24 (the
  improvement can't be cleanly credited to the blocker). Round 25's
  ON re-run DID engage the blocker (21 "Load cancelled" by 240 s)
  and the feed still stalled — without any panic.
- **The `?themeRefresh=1` reload.** Fires once per load (dark-theme
  config), restarts the slow load, not a loop. R24 already refuted it
  as the watch-page stall cause via local repro.
- **A page JS error.** The errcap extension captured **zero** page
  errors, zero unhandled rejections, zero console.error/warn on
  YouTube in 180 s and on Yahoo in 60 s. The app is not throwing —
  it is waiting on something that never happens, or swallowing the
  failure (a guarded call / a promise that never resolves produces
  no `window.onerror`).

## The prime suspect: missing / broken observer + idle APIs

Local API-surface probes (all gated in Frost, Snowplow OFF):

| API | Result |
| --- | --- |
| `fetch`, `Promise`, `XMLHttpRequest`, `MutationObserver`, `ResizeObserver` (constructor), `customElements`, `queueMicrotask`, `URL`, `AbortController` | defined |
| `MutationObserver` callback | FIRED |
| `customElements.define` / `attachShadow` | OK |
| `IntersectionObserver` | **UNDEFINED** (`typeof` is `undefined`) |
| `requestIdleCallback` | **UNDEFINED** |
| `ResizeObserver` callback | **NEVER FIRED after 3 s** (defined, constructor OK, observer attached, no callback) |
| `IntersectionObserver` in Freezer code | Rust code exists (`freezer/components/script/dom/intersectionobserver/`, `update_intersection_observer_steps` called from `script_thread.rs:1252` and `document.rs:3648`) but it is **not exposed to page JS** (no `.webidl` found under `dom/webidl/` — that directory does not exist in this tree) |
| `requestIdleCallback` in Freezer code | **ZERO hits** anywhere in `freezer/components/` (.rs or .webidl) — it does not exist |

Corroborating evidence from the real pages:

- YouTube's own log shows it fetches `intersection-observer.min.js`
  — a polyfill. The app expects IntersectionObserver and ships its
  own when the browser lacks it.
- The errcap banner at 120 s on YouTube read `IO=function`:
  the polyfill **installed** the constructor — yet the browse call
  still never fired. So the polyfill covers the constructor but the
  trigger still doesn't happen: either the polyfill's own
  intersection detection never reports the feed element as
  intersecting in this engine, or the browse trigger is scheduled
  via `requestIdleCallback` (absent, no polyfill observed) rather
  than via IntersectionObserver.
- Yahoo fetches a `polyfills-` chunk — same pattern (the app
  paper-overs a missing API).

Leading hypothesis: **the feed bootstrap is gated on observer/idle
delivery that Freezer does not provide correctly.** The two concrete
candidates are (a) IntersectionObserver missing natively — the
polyfill's callbacks may never fire because its intersection math
depends on layout/scroll signals this engine reports differently —
and (b) requestIdleCallback absent entirely, so any work the app
schedules "when idle" (YouTube's scheduler defers the browse this
way) never runs; if the call is guarded or inside try/catch, it
fails silently, which matches the zero-errors observation.
ResizeObserver's defined-but-never-firing callback is a third
instance of the same delivery gap and strengthens the hypothesis
that the observer *delivery* path, not just the constructors, is
broken or missing.

## The windowproxy.rs:950 panic — a second, separate defect (his-box-only)

Three script-thread panics on Joel's box, same assertion, all on
Yahoo:

```
thread 'Script#112' panicked at servo/components/script/dom/window/windowproxy.rs:950:13:
  assertion failed: parent_proxy.currently_active().is_some()
thread 'Script#145' panicked at the same line.
thread 'Script#97'  panicked at the same line (2026-11-18T03:47:55Z).
```

The third was immediately preceded by a rustls TLS failure:
`cs.openwebmedia.org` presenting a certificate valid only for
`*.ascendeummedia.com`. That rejection is correct behaviour — the
ad network's certificate is misconfigured, NOT a Frost bug — but the
sequence (failed subresource/iframe load → teardown → assertion)
fits the trigger: an ad iframe being torn down while a child
document initializes.

Mechanism (code, `freezer/components/script/dom/window/windowproxy.rs`):
line 950 is in `parent_origin_and_internal_ancestor_origin_objects_list()`,
in the `else if let Some(parent_proxy) = self.parent()` branch:
`assert!(parent_proxy.currently_active().is_some());` then it calls
`parent_proxy.document_origin_and_internal_ancestor_origin_objects_list()`.
The sibling `frame_element()` branch (line 934) instead checks
`parent_document.is_fully_active()` and returns `None` gracefully.
Callers: `servoparser/mod.rs:2451` (`internal_ancestor_origin_objects_list_creation_steps`,
called from `initialize_document_object` at line 1086 during new-document
init — i.e. iframe navigation), `document.rs:2490`, `script_thread.rs:2810`;
`csp.rs:219` calls the same function but handles `None` by breaking.
`currently_active` is `None` when the document is in another script
thread or after `unset_currently_active()` (line 1048).

Sandbox reproduction — **NOT reproduced**, despite:
- Yahoo with Snowplow ON, 240 s (21 "Load cancelled", incl.
  `securepubads.g.doubleclick.net/tag/js/gpt.js`,
  `cdn.taboola.com/libtrc/yahooweb-network/loader.js`,
  `opus.analytics.yahoo.com/tag/opus.js`): 0 panics, 0 windowproxy
  mentions. The feed stall reproduces without any panic.
- Local nested-teardown repro (`r-panic-top.html`: 25 cycles, mid
  iframe with a slow inner iframe via drip server, removed at
  900 ms): 0 panics.
- Local nav-away repro (`r-panic-top2.html`: parent navigates away
  950 ms after load, inner responds at ~1000 ms, 30 attempts): 0.
- Jittered-timing reruns of both (removal/nav at 800–1100 ms
  random, inner delay 0.9/0.95 s, 50 cycles/attempts each):
  0 panics. The first attempts had removal strictly *before* the
  response (the cancel always won cleanly); the jittered runs
  overlapped the response with the discard — still 0.
- The sandbox never even reaches the ad domains from Joel's logs:
  zero `openwebmedia`, zero rustls/cert errors, zero panics in
  `yahoo-gate.log`, `yt-off-gate.log`, `r25-gate-errcap-yahoo.log`,
  `r25-gate-errcap-yt.log`, `r25-gate-probes2.log`. Yahoo serves
  different ad content per run, or the sandbox load stalls before
  the ad-iframe stage.

Cause vs consequence is UNDETERMINED: the constellation
`Disconnected` errors (02:57:03Z) came around when Joel hit stop,
so the panic may be a teardown artifact rather than the stall's
cause — and the feed stall reproduces in the sandbox with no panic
at all, so the panic is NOT required for the feed gap. It is a
real, separate defect: three panics on the user's box is enough
evidence the assertion is reachable in the wild, and a dead script
thread stops the page's JS, which fits the feed never loading *on
his box*. Whether Snowplow was OFF on his box for the third panic
is an open question (asked); if it still panics with Snowplow off,
the "Snowplow cancel causes it" hypothesis weakens and the trigger
is iframe churn / failed loads in general.

## Open items / limits of this round

- p8 (POST fetch): the 8 s screenshot still showed "POST
  PENDING...". python http.server answers 501 to POST, so a
  completed fetch should have shown a status — inconclusive,
  needs a re-check with a longer window.
- The Yahoo errcap 120/180 s observations were lost to a sandbox
  service restart (Frost + Xvfb died mid-run); only the 60 s shot
  survived (banner live, no errors).
- Harness scrolling does not work (keyboard PageDown and the
  `xwheel` helper both fail to move the page) — probe result tails
  below the fold had to be confirmed from the log instead.
- No F-017 logging change: the errcap extension gave the needed
  visibility without an engine rebuild.
- No gdb in the sandbox, so the R24 stall backtrace is still
  awaiting Joel's box (unchanged).

## Fix plan for Round 26

1. **Expose IntersectionObserver to page JS.** Wire the existing
   Rust implementation (`freezer/components/script/dom/intersectionobserver/`,
   `update_intersection_observer_steps`) into the DOM global the way
   the other observer types are exposed. Gate: the p2 probe's
   sentinel turns green / the callback fires; then YouTube/Yahoo
   homepages — the `browse` request must fire.
2. **Implement `requestIdleCallback`.** No code exists today.
   Minimal spec-compliant version: invoke the callback during idle
   periods with a `deadline` object (`timeRemaining()`,
   `didTimeout`); a `setTimeout(…, 1)`-backed fallback is acceptable
   if the engine cannot compute idle time. Gate: `typeof
   requestIdleCallback === 'function'` on a local page, and the
   browse trigger fires on YouTube if it is rIC-gated.
3. **Fix ResizeObserver delivery.** Defined but its callback never
   fires — find why observations are not delivered; it is likely
   the same delivery path as (1). Gate: the p7/p7b probe's
   ResizeObserver callback fires within 3 s.
4. **Defensive fix at `windowproxy.rs:950`** (regardless of
   reproduction — three panics on the user's box is enough):
   replace the `assert!(parent_proxy.currently_active().is_some())`
   with a graceful `None` return, matching the sibling
   `frame_element()` branch at line 934. This is a no-behaviour-change
   hardening for the normal case; it only stops a torn-down parent
   proxy from killing the script thread. Gate: the panic-repro
   pages (r-panic-top/top2, jittered) + a Yahoo Snowplow-ON run
   must show zero panics.
5. **Re-gate the real pages:** YouTube homepage (browse fires, feed
   renders), Yahoo homepage ("For You" fills), plus the full
   regression set. Keep the errcap extension as a diagnostic tool.
6. **Decide by experiment which gate actually triggers the browse:**
   expose IO and rIC one at a time and re-gate YouTube; also wrap
   the polyfilled IO in the errcap script to log whether the
   polyfill's callbacks fire on the real page. The answer decides
   whether (1) or (2) is the feed fix.
