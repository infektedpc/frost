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

---

# Round 26 — FIX: the missing feed APIs (IntersectionObserver, requestIdleCallback, ResizeObserver, windowproxy)

Fix round, 2026-11-18. Joel chose it by tap ("Round 26: fix the
missing feed APIs"). All four Round 25 fix-plan items shipped
(F-017 in Freezer). **The feed is STILL stalled** — the R25
hypothesis (observer/idle APIs gate the browse) is refuted as the
sole cause. This section records what shipped, what each gate
showed, and the new leads, so Round 27 starts from evidence, not
from re-running Round 26.

## What shipped (F-017, freezer commit — see round notes)

1. **IntersectionObserver exposed to page JS — one-line pref
   flip.** The investigation found the R25 diagnosis was wrong
   about the mechanism: the `.webidl` WAS there all along
   (`freezer/components/script_bindings/webidls/IntersectionObserver.webidl`;
   R25 looked under `dom/webidl/`, which does not exist), the
   bindings ARE generated, and the interface IS in the global
   map. The only blocker was the pref gate: the generated
   `ConstructorEnabled` requires
   `pref!(dom_intersection_observer_enabled)`, whose default in
   `freezer/components/config/prefs.rs:467` was `false`. Flipped
   to `true`. The Rust impl was already complete
   (`Observe` / `update_intersection_observations_steps` /
   `invoke_callback_if_necessary`, driven from the script-thread
   rendering loop). No stub, no new code — the observer fires
   real entries.
2. **requestIdleCallback implemented for real.** It did not exist
   anywhere in Freezer. New `IdleDeadline.webidl`
   (`IdleDeadline` with `timeRemaining()` + `didTimeout`,
   `IdleRequestCallback`, `IdleRequestOptions` with `timeout`,
   `partial interface Window` with `requestIdleCallback` /
   `cancelIdleCallback`); new DOM type
   `components/script/dom/idledeadline/`; Document storage
   mirroring rAF (`idle_callback_ident`, `idle_callback_list`,
   `request_idle_callback` / `cancel_idle_callback` /
   `has_pending_idle_callbacks` / `run_idle_callbacks`);
   Window methods in `dom/window/window.rs`; script-thread idle
   runner `maybe_run_idle_callbacks` in
   `event_loop/script_thread.rs`, called at the end of
   `handle_messages` (after font/screenshot steps, before
   `maybe_schedule_rendering_opportunity_after_ipc_message`, so
   DOM changes from idle callbacks still schedule a frame). The
   deadline is a shared 50 ms idle-period budget
   (`CrossProcessInstant`, clamped ≥ 0); `timeRemaining()` returns
   a sane value just under 50 ms on a fresh idle period.
   WebIDL `double` maps to `Finite<f64>` in the generated traits
   — the `TimeRemaining` return type and the non-throwing Window
   signatures had to match exactly (three compiler rounds to get
   there).
3. **ResizeObserver delivery — NO engine change needed.** The R25
   "never fires" observation does not reproduce: with no code
   change at all, the p7b probe's RO callback FIRED, and a new
   probe `p7c-ro-resize.html` showed both the initial observation
   (`entries=1 w=200`) and the resize observation (`entries=1
   w=300` after resize to 300×150). Likely a stale-session/timing
   artifact in R25's probe run. Reported honestly: nothing was
   fixed because nothing was broken.
4. **windowproxy.rs:950 defensive fix.** The assertion
   `parent_proxy.currently_active().is_some()` in
   `parent_origin_and_internal_ancestor_origin_objects_list()`
   is now a graceful `None` return with a comment, matching the
   sibling `frame_element()` branch at line 934. Three panics on
   Joel's box is enough evidence; no reproduction was needed.

New test: `observer_apis_enabled_by_default` in
`freezer/components/config/prefs.rs` (asserts both observer prefs
default true). Test suite: **88/88** in the frost workspace
(frost bin 54 / frozen 18 / extensions 8 / inspector 8), plus
**servo-config 3/3** in the Freezer workspace including the new
test. Note for future rounds: the two GL-probe tests in the frost
bin (`gl_adapter_probe_finds_the_sandbox_gl_adapter`,
`gl_backend_default_respects_user_value`) REQUIRE the vendored
Mesa EGL env vars (the launchers' `LIBGL_ALWAYS_SOFTWARE=1`,
`__EGL_VENDOR_LIBRARY_DIRS`, `LIBGL_DRIVERS_PATH`,
`VK_ICD_FILENAMES`, mesa-egl on `LD_LIBRARY_PATH`) — a plain
`sourced r17-build-env.sh` run fails exactly those two with "no
GL adapter found" (first R26 run: 52/54, EXIT=101, both failures
environmental, no code touched). The 88/88 was verified by
running the cargo-built test binaries directly with that env
(the sibling PornBrowser Round 2 builder was holding the
`~/workspace/frost/target` build lock with its own `cargo test`,
so a `cargo test` re-run could not start).

## Per-API gates (Snowplow OFF, errcap launcher, one at a time)

| Gate | Result |
| --- | --- |
| p2 IO probe (IO only, pref flip) | **"IO FIRED: isIntersecting=true ratio=1"**, sentinel green |
| p7b (IO only) | IO defined, rIC UNDEFINED (expected), MO FIRED, **RO FIRED** |
| p7c RO resize (IO only) | initial observation FIRED entries=1 w=200; after resize FIRED entries=1 w=300 |
| p3 rIC probe (IO + rIC) | **"RIC FIRED: timeRemaining=49.667341"**; re-gated after the borrow fix: **49.698929** |
| YouTube, IO only (105 s) | banner `IO=function RIC=undefined`; feed skeleton-only; **`youtubei/v1/browse` 0×**. IO alone does NOT unlock the feed |
| YouTube, IO + rIC (105 s, fixed build) | banner `IO=function RIC=function`; feed still empty; **`browse` STILL 0×**. Both APIs present and firing, browse never fires |
| Yahoo, IO only (90 s) | styled, hero story + Trending render; "For You" below the fold, unverifiable from the shot (harness can't scroll) |
| Yahoo, IO + rIC (90 s, fixed build) | styled, hero story (Erewhon $749 Thanksgiving dinner) + Trending + weather card render; "For You" still below the fold |
| p8 POST fetch (R25 open item) | **RESOLVED as working.** POST to local server arrived (`POST /r25-feed/feed.json … 501`); page: "POST SENT, server responded HTTP 501 … body len 357". Re-run on the fixed build: same, no crash |
| p9 fetch-shapes probe (new) | all four shapes RESPONDED HTTP 501: plain JSON POST, innertube-headers POST (`X-Goog-Visitor-Id`, `X-Youtube-Client-Name`, `X-Goog-Api-Key`), `credentials:include` POST, abort-signal POST. Fetch POST works for every shape — the player-POST drop is NOT about request shape |

## The feed is still stalled — what the R25 hypothesis got wrong

Both APIs are present, defined, and firing on the real pages
(errcap banner: `IO=function RIC=function`), and the feed still
never renders. `youtubei/v1/browse` fires 0× in every run (IO
only, IO + rIC, Snowplow ON and OFF). The R25 hypothesis is
refuted as the sole cause: the browse trigger is not (only) the
observer/idle APIs.

New evidence from the fixed-build YouTube run (105 s,
`r26-gate-yt3-errcap.log`):

- The app now fetches `googlevideo.com/videoplayback` (16
  engine-log lines, itag=18 progressive MP4) — behaviour not
  seen in R25 — yet the feed never renders. The app is doing
  *more* work with the APIs present, but the feed path still
  doesn't run.
- **Player POSTs attempted but never sent.** The errcap banner
  (page-JS fetch wrapper) shows `FETCH: POST
  .../youtubei/v1/player?key=...` repeatedly, but the engine log
  (`net::http_loader` INFO lines, the authoritative request
  record) has ZERO `youtubei/v1/player` requests — same in R25.
  By contrast guide POSTs (fetch, 2×, 200), `log_event` POSTs
  (XHR, 6×), and videoplayback GETs DO reach the network. p9
  proves fetch POST works for every request shape, so the drop
  is specific to YouTube's player requests — their context
  (which script/frame/worker issues them), not their shape.
  Unresolved: whether the app ever *attempts* `browse` at all —
  it never appears in the banner, so the app never decides to
  fetch it. The browse trigger (not the fetch) is the open
  question.
- **8 transient TLS-EOF WARNs** in the yt3 log
  (`hyper::Error(Io, … "peer closed connection without sending
  TLS close_notify")` on reused connections). NOT the R17 h2
  `PROTOCOL_ERROR` — the page loads fine, 0 "error loading
  page". The WARN lines don't name the URL, so they can't be
  attributed to the player POSTs — but a player POST sent on a
  connection the peer then closes without close_notify would
  reject the fetch promise exactly this way, and the app retries
  (consistent with the repeated attempts in the banner). This is
  a hypothesis, not a finding.

## A new script-thread panic found and fixed during this round

The first Phase B build (IO + rIC + windowproxy) panicked on the
YouTube run:

```
thread 'Script#2' panicked at freezer/components/script/event_loop/script_thread.rs:3743:14:
  RefCell already borrowed
```

Line 3743 is `self.documents.borrow_mut().insert(...)` inside
`fn load` (navigation/document creation). Suspected cause: the
original `maybe_run_idle_callbacks` held `self.documents.borrow()`
while iterating AND calling `document.run_idle_callbacks(cx)`
(arbitrary page JS); JS that synchronously creates/destroys a
document (YouTube churning iframes) would hit exactly this. The
fix (correct regardless of whether the theory is proven — never
hold the documents borrow across JS) collects rooted documents
first, mirroring `update_the_rendering`'s convention:

```rust
let documents: Vec<DomRoot<Document>> =
    self.documents.borrow().iter().map(|(_, document)| document).collect();
```

then iterates the Vec. (First fix attempt failed to compile:
`DocumentsIter::Item` is `(PipelineId, DomRoot<Document>)` by
value, already rooted — no dereference needed.) Rebuilt
(11m37s, binary 522,361,240 B) and re-gated with
`RUST_BACKTRACE=1`: p3 still fires (49.698929), YouTube 105 s
shows **0 panics**, p8 re-run shows no crash. The panic's stack
was never captured (RUST_BACKTRACE wasn't set on the first run),
so the borrow theory is unproven — the fix is sound, and the
re-gate is the verification. The p8 screenshot in the *first*
Phase B run showed "Servo crashed! … Send failed
(Disconnected)" — the page crashed during/after that
navigation; the fixed build does not crash on the same path.

## Regression (fixed build, Snowplow OFF)

- R18 probe: all visible checks PASS, "probe done", no FAIL
  (incl. `isTypeSupported webm is false (F-015 steer)` and
  `av01 is false (F-015 steer)` — the steer is intact).
- R19: buffered [0.00–10.00], readyState 4, position to 10.07.
- R20 real DASH ("Me at the zoo"): buffered [0.00–18.93] /
  [0.00–19.06], position to 19.03.
- youtube.com: page loads, 0 PROTOCOL_ERROR / 0 "error loading
  page" (8 transient TLS-EOF WARNs, see above).
- Local H.264 to end: PositionChanged 5.00 + EndOfStream.
- Plain WebM (`test.webm`): PositionChanged to 3.04.
- example.org renders (dark theme, all languages); command
  palette (Ctrl+K) renders its full list; hamburger menu renders
  all rows; split view shows two panes both rendering
  example.org; Freeze wrote
  `example-domain-20261118-0849.html` (2,639 B) + index.json
  (2 entries, correct title/URL); Settings shows Snowplow: Off
  and Save persists (`settings.json` still `snowplow: false`
  after Save); Inspector (F12) captured example.org, **33
  nodes** (R25 saw 35 on the same gate — likely page-load
  variance; the tree is complete and correct for example.org).

## Leads for Round 27 (not diagnosed in this round)

1. **Log the request URL in the http_loader network-error WARN**
   (`net::http_loader`, the `SendRequest` / TLS-EOF path), then
   re-gate YouTube: are the player POSTs the ones dying with
   TLS EOF on reused connections? If yes, the fix is at the
   connection-reuse / retry layer (or the fetch layer retrying
   failed POSTs), not in the page APIs.
2. **What triggers `browse`?** The app never attempts it. The
   trigger is upstream of fetch: candidates are
   `scheduler.postTask` (YouTube's modern scheduler — check
   whether it exists in Freezer), `navigator.serviceWorker`,
   `document.visibilityState` / idle signals, or a promise in
   the bootstrap chain that never resolves because of another
   missing API. Instrument the errcap extension to log
   `typeof scheduler`, `scheduler.postTask`, serviceWorker
   registration, and whether the polyfilled IO's callbacks fire
   on the real page (Round 25 plan item 6, still undone).
3. **The R24 watch-page stall and metadata flake** are unchanged
   by this round (no fix was attempted for them here) and still
   await the backtrace on Joel's box.


# Round 27 — chase the browse trigger: URL in the network-error WARN (F-018) + what triggers `browse`

Joel chose this round 2026-11-18 ("Onto round 27?"). Both Round 26
leads were worked, in order. Headline: **the feed is STILL stalled
— `youtubei/v1/browse` fires 0× in every run, and the trigger is
still not found.** But both leads produced firm, checkable results:
the WARN now names the URL (verified live), `scheduler.postTask`
is ruled out by YouTube's own code, and the one remaining anomaly
is that IntersectionObserver delivery is *slow* (seconds, not
milliseconds) on every page — though 105 s of waiting still does
not produce a feed, so latency alone is not the cause.

## What shipped

- **F-018 (freezer, one line + comment):**
  `freezer/components/net/http_loader.rs:732`, inside
  `obtain_response()` — the WARN `warn!("network error:
  {error:?}")` now reads `warn!("network error for {}:
  {error:?}", url.as_str())`. The URL was in scope (`url:
  &ServoUrl` param) and is now in the log line. No behaviour
  change.
- **R27 errcap (frost-side, `extensions/r25-errcap/`, v0.2.0,
  no rebuild needed — the extension loads from disk):** the
  banner's first line now reports the Round 26 lead-2 candidates
  live on the page — `IO=`, `RIC=`, `SCH=<typeof scheduler>`,
  `postTask=`, `SW=<typeof navigator.serviceWorker>`,
  `VIS=<document.visibilityState>`; it wraps
  `navigator.serviceWorker.register` to log `SWREG` attempts;
  it runs a live IO delivery probe (a fixed in-viewport
  sentinel, logs `IO-CB: fired …` or `IO-CB: NOT FIRED after
  5s`) and a live rIC delivery probe (logs `RIC-CB: fired
  timeRemaining=…` or `RIC-CB: NOT FIRED after 10s`); it logs
  `READY readyState=…` at install. New local probe
  `r13-test/r25-feed/p10-io-latency.html` (timestamps IO
  callback delivery; also re-tests after moving the sentinel
  out of viewport).

## Lead 1 — the WARN URL, and are the player POSTs dying?

**The WARN works, verified live.** Navigating the R27 binary to
`https://nonexistent.invalid/` produced, in `r27-gate-yt.log`:
`[WARN net::http_loader] network error for
https://nonexistent.invalid/: hyper_util::client::legacy::Error
(SendRequest, hyper::Error(Io, Custom { kind: UnexpectedEof,
error: "peer closed connection without sending TLS
close_notify: ..." }))`. The URL is in the line. The binary
carries the format string (verified: `strings` count 1 for
`"network error for "`; binary mtime 2026-11-18 10:37 UTC,
fresh from the R27 build).

**But the YouTube re-gate could not re-test the TLS-EOF
hypothesis: this run had ZERO network errors.** The 105 s
post-build YouTube gate (`r27-gate-yt.log`, Snowplow OFF,
R27 errcap) shows 0 `network error`, 0 `close_notify`, 0
`SendRequest` errors — the R26 run's 8 transient TLS-EOF WARNs
did not recur. The TLS-EOF is transient per run, not a
per-run constant, so the "player POSTs die on reused
connections" hypothesis is **untested, not refuted** — it needs
a run in which the WARN actually fires with a player URL in it.
No fix was attempted at the connection-reuse / retry layer:
there is no evidence in this round pointing at it, and the
fix would be a guess.

Side observation from the WARN probe: the error for a
nonexistent host is a TLS UnexpectedEof on a *fresh*
connection, not a DNS failure — the sandbox's network path
returns TLS-EOF for unreachable hosts generally. That weakens
the R26 "reused connections" reading of the TLS-EOFs (they may
just be this sandbox's generic unreachable-host failure), but
it does not decide it: the R26 YouTube TLS-EOFs happened on
requests to a host that *does* answer (youtube.com), so a
per-request cause is still open. Recorded here so Round 28
does not re-litigate it from scratch.

## Lead 2 — what triggers `browse`?

**`scheduler.postTask` is RULED OUT, by YouTube's own code.**
Live-fetched YouTube JS (2026-11-18, ~13 MB total, files in
`/tmp/r27-yt-*.js` + `r27-yt-home.html`): 0 occurrences of
"postTask" in all 5 bootstrap scripts (scheduler.js 8,197 B —
it uses `requestIdleCallback` 3×, `requestAnimationFrame` 2×,
`setTimeout` 7×, `MessageChannel` 0, `queueMicrotask` 0,
`IntersectionObserver` 0), in player base.js (2,684,806 B —
its "scheduler" hits are all YouTube's own
`yt.scheduler.instance.addJob` / `a.scheduler.ak(...)`, not
the browser API), and in the main app bundle kevlar_base +
kevlar_main (9,792,592 B). The HTML's "scheduler" hits are
feature flags (`transport_use_scheduler`,
`mweb_watch_stop_scheduler_on_player_response`,
`kevlar_tuner_scheduler_soft_state_timer_ms`). YouTube's own
scheduler is built on rIC / rAF / setTimeout — all present in
Freezer. **Do NOT implement `scheduler.postTask` for this.**
(Caveat: the check covered ~13 MB of fetched JS; no other
chunk was found unexamined.)

**`serviceWorker` is effectively ruled out for the feed.**
Runtime: `SW=undefined` on both YouTube and Yahoo (pref
`dom_serviceworker_enabled` defaults false, `prefs.rs:479`).
No `SWREG` lines in any gate. YouTube desktop web does not
register a service worker for feed loading.

**`visibilityState` is ruled out.** Runtime: `VIS=visible`
on both sites.

**IntersectionObserver and requestIdleCallback both fire on
the real pages — slowly.** Post-build YouTube 105 s gate
(`r27-yt-105s.jpg`): banner `IO=function RIC=function
SCH=undefined postTask=n/a SW=undefined VIS=visible`;
`RIC-CB: fired timeRemaining=47.306497 didTimeout=false`;
`IO-CB: fired entries=1 isIntersecting=true ratio=1`. Yahoo
90 s gate (`r27-yahoo-90s.jpg`): same first line;
`RIC-CB: fired timeRemaining=47.865036 didTimeout=false`;
`IO-CB: NOT FIRED after 5s` **then** `IO-CB: fired entries=1
isIntersecting=true ratio=1` — it fired, but after the 5 s
probe deadline. Pre-build YouTube gate showed the same
pattern (NOT FIRED at 5 s, then fired). Local probe p10
(`r27-p10-latency.jpg`): IO callback fired **2,069 ms**
after observe() on a local page (read from document.title;
the probe page's sentinel/status divs never painted — stale
frame, DOM updated, compositor did not repaint; the title
is chrome, not page content, so the 2069 ms number stands).
**IO delivery is slow everywhere (2 s local, >5 s on real
pages under load) — the one observed anomaly — but 105 s is
far longer than the latency, so latency alone does not
explain the stall. Something else upstream of `browse` is
still missing or broken.**

**The app is alive and talking — it just never asks for the
feed.** Post-build YouTube engine log: `youtubei` appears
8×, all `log_event` (3 POSTs, all 200) and `guide` (1 POST,
200); `browse` 0, `player` 0. googleads XHRs go out. The
page finishes everything it will do and sits on skeleton
tiles. Yahoo: the app makes real fetch calls the errcap sees
(`signal-service.pbs.yahoo.com/v1/signal/refresh`,
`s.yimg.com` prebid configs, `api.rlcdn.com` identity
envelope, `match.adsrvr.org` tracker, `rp.liadm.com`) and
renders styled + hero + Trending; its "For You" area is
below the fold and unverifiable (the harness cannot scroll).
No page JS errors on either site (errcap, 105 s / 90 s).
No panics in either gate log.

**A discrepancy to note for Round 28:** this run's YouTube
errcap banner shows NO fetch/XHR lines at all, while the
engine log shows guide + log_event POSTs going out — so the
app's network calls are not going through the window fetch /
XHR the errcap wraps, or they fire before the errcap injects
(at DocumentIdle / LoadFinished). In R26 the banner DID show
player POST attempts. The errcap's fetch view is therefore
timing-dependent and is not a complete record of the app's
network activity; the engine log is the ground truth for what
reaches the network. The R26 "player POSTs attempted but never
reach the engine" finding stands as an R26 observation, not
reproduced in this round's gates (this round's engine log
shows no player attempts at all — the app never got that far,
or never tried).

## The feed is still stalled — what is still unknown

- What actually triggers `youtubei/v1/browse` in YouTube's
  bootstrap chain. All four R25/R26 candidate APIs are now
  either present and firing (IO, rIC) or ruled out
  (scheduler.postTask, serviceWorker, visibilityState). The
  trigger is something else: a promise in the bootstrap chain
  that never resolves, another missing API, or a fetch the app
  makes through a path the engine drops silently.
- Whether the player POSTs die with TLS EOF when they do get
  attempted (needs a run where the F-018 WARN fires with a
  player URL).
- The R24 watch-page stall and metadata flake are unchanged
  by this round (out of scope) and still await the backtrace
  on Joel's box.

## Build incident — stale servo-script bindings (fixed)

The first full R27 build failed: `could not compile
servo-script (lib) due to 5 previous errors` — `E0432:
unresolved import IdleDeadlineBinding` (document.rs:102,
idledeadline.rs:19, window.rs:121) + `E0407:
RequestIdleCallback` / `CancelIdleCallback` not members of
trait `WindowMethods`. Root cause, found firsthand: the
generated bindings OUT_DIR
(`target/debug/build/servo-script-bindings-fa3fc5b7e27907ff/
out/`, timestamped 2026-11-18 09:28:08 UTC) contained 490
Bindings files but NO `IdleDeadlineBinding.rs`, and
`InterfaceObjectMapData.json` had 0 IdleDeadline mentions —
yet a manual codegen run produces `IdleDeadlineBinding.rs`.
The R26 tree is fine (codegen globs all `webidls/*.webidl`).
The stale OUT_DIR was clobbered during the shared-target-dir
period, when the PornBrowser builder's cargo regenerated
script bindings from its R24 tree (which has no
`IdleDeadline.webidl`) into Frost's shared target dir, and
cargo then reused the stale OUT_DIR instead of rerunning the
build script. **Fix: `cargo clean -p servo-script-bindings`**
(removed 3,079 files, 8.0 GiB). Build 2 succeeded
(`Finished dev profile in 16m 56s`, log `r27-build2.log`).
The regenerated OUT_DIR now contains `IdleDeadlineBinding.rs`.

## Test-suite incident — stripped test binary hangs at startup

The full suite is **88/88** (frost bin 54/54, frozen 18/18,
extensions 8/8, inspector 8/8) + servo-config 3/3 — but getting
there took a detour, and the detour is worth recording because
it will bite again if the round build keeps using
`-C link-arg=-Wl,--strip-debug` for test binaries.

- The build-2 test binary `frost-20b78d5bb5f965eb` (522 MB,
  stripped) **hangs at startup**: `--list` never prints in 60 s,
  a single-threaded run prints zero test lines in 180 s, and a
  600 s run (killed at ~6 min) also printed nothing. It burns
  CPU the whole time (futex spin; strace shows ~90% of time in
  `futex`, plus `epoll_wait`/`ppoll`/`madvise` churn, and the
  only stacks sampled are library constructors — libglib
  `g_once_init`, libstdc++ locale init, lavapipe libLLVM pass
  registration — i.e. it never gets past early startup). The
  R26 test binary `frost-1cc4e912b267895d` (1.5 GB, unstripped)
  lists 54 tests in 0.09 s under the same env and the same
  PornBrowser contention, so the hang is specific to the
  stripped build-2 test binary, not the source and not the
  machine. The stripped *gate* binary (`target/debug/frost`,
  same flag) starts and paints fine and ran every gate in this
  round — the hang is specific to the test-harness binary.
- The frozen (18/18), extensions (8/8), and inspector (8/8)
  test binaries from build 2 ran fine (log `r27-test.log`),
  as did servo-config 3/3 (`r27-test-config.log`).
- The frost-bin 54/54 was verified on a **plain (unstripped)
  rebuild** of the test binary: `cargo test -p frost --bins
  --no-run` (log `r27-test-norun2.log`, `NORUN2-EXIT=0`,
  finished in 4m 58s once the machine was idle), then
  `./frost-1cc4e912b267895d` → `test result: ok. 54 passed;
  0 failed; finished in 0.95s` (log `r27-test-frost.log`).
  The frost crate source is identical to R26 (no frost-crate
  change in this round), so the rebuilt binary has the same
  hash as R26's.
- A runtime restart mid-round killed the first plain rebuild
  (`cargo test --workspace --lib --bins --no-run`, started
  10:53, killed ~12:06 with its rust-lld at 21:53 CPU time
  and a 1.5 GB `.tmp` link output lost); the targeted
  `-p frost --bins` rebuild above is the one that completed.
  The PornBrowser cargo was also gone after the restart, which
  is why the targeted rebuild finished in under 5 minutes.
- **Lesson for future rounds: do not apply
  `-C link-arg=-Wl,--strip-debug` to test binaries.** The flag
  is for the gate binary only (where it is needed — the plain
  2 GB link OOM-kills). Build test binaries plainly
  (`cargo test --no-run` without the link arg) and run them
  directly.

## Regression (post-build binary, Snowplow OFF)

- R18 probe: all visible checks PASS, "probe done", no FAIL
  (incl. `isTypeSupported webm is false (F-015 steer)` and
  `av01 is false (F-015 steer)` — the steer is intact).
- R19: buffered [0.00–10.00], readyState 4, position to 10.07.
- R20 real DASH ("Me at the zoo"): buffered [0.00–18.93] /
  [0.00–19.06], position to 19.06.
- youtube.com: page loads, 0 PROTOCOL_ERROR / 0 "error loading
  page" (0 network errors of any kind this run — see Lead 1).
- Local H.264 to end: PositionChanged 5.00 + EndOfStream.
- Plain WebM (`test.webm`): MetadataUpdated 320x240
  format=Matroska, DurationChanged 3.049s, PositionChanged to
  3.04.
- example.org renders (dark theme, all languages); command
  palette (Ctrl+K) renders its full list; hamburger menu renders
  all rows; split view shows two panes both rendering
  example.org; Freeze wrote
  `example-domain-20261118-1147.html` (2,639 B) + index.json
  (correct title "Example Domain" / URL this time — the R24
  stale-title wart did not recur); Settings shows Snowplow: Off
  and Save persists (`settings.json` still `snowplow: false`
  after Save, "Settings saved." shown); Inspector (F12)
  captured example.org, **33 nodes** (same as R26).

## Leads for Round 28 (not diagnosed in this round)

1. **Re-gate YouTube until the F-018 WARN fires with a player
   URL** — the TLS-EOF / player-POST question is still open;
   if the WARN names a player URL, the fix belongs at the
   connection-reuse / retry layer.
2. **Find the `browse` trigger by instrumenting the bootstrap
   chain, not by adding APIs.** The next step is to watch what
   the app does between `guide` (which fires) and `browse`
   (which never fires): log every fetch/XHR the app makes
   from *inside* the engine (http_loader already logs POSTs
   at info level — the gap is page-side calls that never reach
   it), and instrument the errcap to run at document-start so
   it wraps fetch/XHR before the app's bootstrap caches its
   own references. Candidates to check next: `fetch` called
   via a cached reference, `sendBeacon`, `navigator.sendBeacon`,
   or a `Promise` in the bootstrap that never resolves because
   of a missing API not yet catalogued (audit the app bundle
   for `typeof` guards on browser APIs the way postTask was
   audited).
3. **IO delivery latency** (2 s local, >5 s on real pages) is
   worth one focused look: find where the 2 s goes (observer
   steps run from `script_thread.rs:1252` / `document.rs:3648`
   — check the scheduling interval). It is probably not the
   feed cause, but a 2 s observer delay is a real engine
   defect on its own.
4. **The R24 watch-page stall and metadata flake** are unchanged
   by this round (no fix was attempted for them here) and still
   await the backtrace on Joel's box.

# Round 28 — bootstrap instrumentation (document-start errcap) + IO latency fix (F-019) + re-gate for the F-018 WARN

## What shipped

- **F-019 (freezer, `intersectionobserver.rs`) — the IO latency fix.**
  `maybe_compute_intersection_output` now returns
  `Option<IntersectionObservationOutput>`: `None` means layout
  data is unavailable yet (the target has no box — the document
  is still being parsed). The caller no longer queues the
  `default_skipped()` value as a false "not intersecting" and no
  longer touches `previous_*`; while `document.readyState ==
  loading` it re-adds
  `RenderingUpdateReason::IntersectionObserverStartedObservingTarget`
  so the steps re-run at the next rendering opportunity (~20 ms
  later). Once parsing is done a missing box is permanent (e.g.
  `display:none`) and the skipped value stands — no infinite
  retry. Also fixed the Step 2 delay check: the spec says skip
  further processing *for target*; the old `return` aborted the
  steps for every remaining target of the observer (now
  `continue`).
- **Document-start errcap (frost + frozen + extensions).**
  New `EngineEvent::HeadParsed(WebViewId)`, mapped from
  `LoadStatus::HeadParsed` (was dropped in `crates/frozen`).
  `app.rs` injects DocumentStart scripts on HeadParsed via new
  `inject_start_scripts` / `inject_content_scripts_filtered`
  (the finish pass skips start scripts already run at HeadParsed,
  re-runs them as fallback if HeadParsed never fired).
  `extensions/r25-errcap/` v0.3.0, `run_at: "DocumentStart"`,
  name "R28 Error Capture": content.js timestamps every line
  with ms since install, logs `readystatechange` transitions,
  wraps `navigator.sendBeacon`, and logs fetch/XHR outcomes
  (`FETCH-OK` / `FETCH-ERR` / `FETCH-THROW` incl.
  `signal.aborted`, `XHR-OK/ERROR/ABORT/TIMEOUT`). One new unit
  test (`document_start_run_at_parses`) — suite is now **89/89**
  (frost bin 54, frozen 18, extensions 9, inspector 8) +
  servo-config 3/3.

## Lead 3 — IO latency: FIXED, and the mechanism is proven

Probes `r13-test/r25-feed/p11-raf-io.html`, `p11b-raf-io.html`
(timed, counts firings). Pre-fix (R27 binary, 4 runs): rAF
fires promptly (~90–173 ms) in the first
`update_the_rendering`; the first IO callback fires late and
reports a **false** `isIntersecting=false` for a visible target
(R27: 2,069 ms; R28 run 3: 2,015 ms; R28 run 1: never by 12 s),
and the TRUE value only arrives at ~2,078 ms (15 ms after the
2,000 ms setTimeout fires) — count=2, i.e. the app gets a false
initial entry and then a correction. A second observer created
after settle fires in 15–79 ms. Root cause, traced in code: at
the first update layout isn't ready,
`border_box_without_reflow()` returns None,
`maybe_compute_intersection_output` returned
`default_skipped()` (NotMatching/false), which differs from the
registration's initial (None/false), so step 18 queues a FALSE
initial entry; the reason bit was already cleared, so nothing
re-runs the IO steps until an unrelated event (timer/DOM
change) triggers another rendering update.

Post-fix gate (F-019 binary, `r28-p11b2.jpg` + Inspector
`r28-p11b2-insp5.jpg`, log `r28-gate-p11b2.log`): **IO1 fires
#1 at 97.5 ms with `isIntersecting=true ratio=1`** — the false
initial entry is gone, first notification is true and lands in
the same rendering update as rAF (RAF1 89.6 ms, RAF2 90.2 ms).
The second IO1 firing at 2,068 ms is legitimate: the page's
own 2,000 ms timer moves the sentinel below the fold
(`marginTop 900px`), so true → false is a real transition;
IO2 (created at 2,005 ms) fires at 2,071 ms. IO latency:
**2,069 ms → ~97 ms**, and correct. One run-to-run variance
remains unexplained: the pre-fix run 1 never fired IO1 by
12 s while run 4 fired false at 191 ms — first-load observer
delivery is racy in a way the counted run does not fully
explain; F-019 removes the false value, but the never-fired
case is not proven fixed.

## Lead 1 — F-018 WARN re-gate (pre-build, R27 binary)

YouTube 300 s, Snowplow OFF, log `r28-gate-yt-pre.log`,
shots `r28-yt-30s/105s/300s.jpg`: **0 network errors in
300 s.** The TLS-EOF / player-POST hypothesis remains
UNTESTED (R26's 8 WARNs recurred in neither R27's 105 s nor
this 300 s run). Engine log: browse 0, player 0, guide 2,
log_event 10. But the banner showed a player POST attempt
(`FETCH: POST https://www.youtube.com/youtubei/v1/player?key=...`)
— the R26 finding reproduced: the fetch is attempted
page-side but never reaches http_loader. Sharpened by this
run: videoplayback GETs DO reach the engine (22 hits, 403
responses — YouTube bot-checking the sandbox IP),
fonts.gstatic 333, googleads 9. **The player POST is the
ONLY request type that never arrives.** guide/log_event
POSTs arrive; videoplayback/fonts GETs arrive. The R28
errcap's new FETCH-ERR outcome logging is the decisive probe
for whether the player fetch rejects (e.g. AbortError) or
never settles.

## Lead 2 — document-start errcap gate: WORKS, feed still stalled

YouTube 300 s, Snowplow OFF, F-019 binary + R28 errcap, log
`r28-gate-yt2.log`, shots `r28-yt2-105s/300s.jpg`. The
document-start injection is proven working: the banner shows
`R28-ERRCAP active. IO=function RIC=function SCH=undefined
postTask=n/a SW=undefined VIS=visible`, `+5ms READY
readyState=loading`, `+18ms RIC-CB: fired
timeRemaining=49.779749 didTimeout=false` — injection lands
before the app's bootstrap, and rIC fires promptly. The banner
now captures the app's calls with outcomes from the start:
`FETCH-OK: 200` lines for gstatic/fonts icon SVGs (a batch at
~+136 s), `XHR-OK 200: /youtubei/v1/log_event?alt=json` as the
latest line (+205 s). Yahoo 90 s (`r28-yahoo-90s.jpg`, log
`r28-gate-yahoo.log`) likewise: banner shows FETCH/XHR lines
with `signal.aborted=false` for the app's ad-tech POSTs
(pbs.yahoo.com cookie_sync + openrtb2/auction,
hbopenbid.pubmatic.com, udc.yahoo.com, trc.taboola.com
FETCH-OK 200, securepubads.g.doubleclick.net FETCH-OK 200) —
all succeeding; page renders styled with a real hero + Trending.

**But the feed is still stalled on both sites.** YouTube at
300 s: browse 0, player 0, guide 4, log_event 16, feedback 2
(engine log). Feed area empty, skeleton tiles only. Yahoo:
hero + Trending render; "For You" below the fold,
unverifiable (harness can't scroll) — same as R26/R27.

**New, sharp data point — the IO probe still fails on the
real page:** the errcap's own IntersectionObserver probe (a
fixed 50×50 div at top:100px, observed at install) logs
`+5010ms IO-CB: NOT FIRED after 5s (observer delivery broken
on this page)` on YouTube — even WITH F-019, which fixed the
local case to 97 ms. So the real-page IO stall is a different
mechanism from the false-initial-entry bug: on the real page
the observer callback does not fire at all within 5 s (whether
it fires later was not established — the banner's later lines
could not be read: the Inspector snapshot wedged on YouTube's
DOM, "Capturing page structure..." for 3+ min, frame stale).
Candidates: rendering updates not running the IO steps while
the page is still loading, layout never becoming available
during the long parse, or the re-added reason not waking the
update loop. This is the Round 29 lead.

**What the app is waiting on is still unknown.** The
document-start errcap proves the app's fetch/XHR layer works
(guide/log_event/feedback + icons all 200) and that rIC fires
promptly — so the stall is not a missing idle API and not a
dead fetch layer. The app simply never attempts `browse` in
300 s, and in this run no player POST attempt appeared in the
banner's visible window (the R28-pre run DID show one while
the engine log showed player 0 — whether it was attempted in
this run is NOT established, because the banner keeps only the
last 40 lines and the full banner could not be read).

## Lead 1 — F-018 WARN re-gate (F-019 binary): FIRED, but on videoplayback, not the player POST

The 300 s YouTube run produced **6 network errors**, all
naming `https://rr5---sn-hgn7rnll.googlevideo.com/videoplayback?...&itag=18&...`:
4× `SendRequest, hyper::Error(Io, Kind(ConnectionReset))` and
2× `SendRequest, ... UnexpectedEof, "peer closed connection
without sending TLS close_notify"` (14:41:49–14:42:41Z).
**This REFUTES the R26 hypothesis as the player-POST
mechanism** — the dying requests are videoplayback GETs
(itag 18, the progressive MP4 the app fetches), not the player
POST, which never reaches http_loader at all and therefore
cannot be dying there. The videoplayback deaths are a real,
separate connection problem (YouTube's googlevideo servers
resetting connections from the sandbox IP, or a reuse bug at
the hyper/rustls layer) — worth a Round 29 look at
connection reuse / retry for media GETs, but it is NOT the
feed cause: the feed needs `browse`, which the app never
attempts. The 105 s checkpoint had 0 network errors; the
pre-build 300 s run (R27 binary) also had 0 — the WARN is
intermittent, as R26's 8 WARNs were.

## Regression (F-019 binary, all PASS)

- R18 probe: **ALL PASS** (29/29, incl. F-015 steer lines).
- R19 probe: **ALL PASS** (buffered [0.00–10.00], playing,
  currentTime 1.50, decoded frame).
- R20 real DASH ("Me at the zoo"): **ALL PASS** (buffered
  [0.00–18.93]/[0.00–19.06], playing, currentTime 0.00→2.56,
  decoded frame).
- youtube.com: document `got 200`, **0 SendRequest on the
  document request** (the 6 SendRequest errors are all
  videoplayback subresources — see Lead 1).
- Local H.264 to end: PositionChanged 0.50→5.00 +
  EndOfStream, controls 00:05/00:05.
- Plain WebM: **ALL PASS** (duration 3.049, currentTime
  0.00→3.01).
- Homepage/banner: example.org + "Frost sample extension is
  live".
- Palette (Ctrl+K): all commands listed; "Split view" chosen
  from it.
- Split view: two panes, both example.org.
- Menu: Settings / Print / Open file / Inspector / Command
  palette / Close split view / Freeze / Archived / Exit.
- Freeze: archive `example-domain-20261118-1459.html`
  (2,947 B) + index.json correct (title "Example Domain",
  correct URL/filename — the R24 stale-title wart did not
  recur); status "Frozen: Example Domain".
- Settings persist: Snowplow toggled Off, "Settings saved.",
  settings.json confirms `"snowplow": false`.
- Inspector (F12): example.org, **35 nodes** (R25: 35,
  R26: 33 — page-load variance; tree complete).

Tests: **89/89** (frost bin 54, frozen 18, extensions 9,
inspector 8) + servo-config 3/3. No panics in any gate log
(YouTube 300 s, Yahoo 90 s, all regression runs). No
`.deb`, no push in this round (per brief).

Incidents: the VM rebooted mid-round (~14:16 UTC), killing
a rebuild mid-link and Xvfb + the test servers; harness was
rebuilt (Xvfb:97 + 18080/18099/18101, all serving r13-test)
and the build resumed incrementally. The servers needed
restarting with a subshell `(cd ... && ...)` — a bare `cd X
&& cmd1 & cmd2 &` leaves cmd2 in the old cwd (18101 served
/home/hatch and 404'd until fixed). The Inspector snapshot
wedged on YouTube's DOM ("Capturing page structure..." 3+
min, stale frame) — it cannot read the full errcap banner on
a large page.

## Leads for Round 29 (not diagnosed in this round)

1. **IO delivery on the real page still doesn't fire within
   5 s despite F-019** — the errcap probe proves it on
   YouTube. Find why `update_the_rendering` doesn't deliver
   the observer steps while the page loads (is the reason
   re-add waking the loop? does layout stay unavailable for
   the whole parse? do rendering updates even run during the
   heavy bootstrap?). Instrument the IO steps (log when they
   run per document, and what `maybe_compute_intersection_output`
   returns) rather than guessing.
2. **What triggers `browse` is still unknown.** The
   document-start errcap proves fetch/XHR + rIC all work, so
   the app is waiting on something else that never resolves.
   Next: audit the app bundle for `typeof` guards the way
   postTask was audited, and log the bootstrap promise chain
   (which fetches the app makes in its first 30 s, in order —
   the errcap's 40-line cap loses early lines, so raise the
   cap or log to the engine).
3. **videoplayback GETs die with ConnectionReset / TLS-EOF**
   (6× in 300 s) — a real connection problem at the
   hyper/rustls layer, separate from the feed. If it is
   connection reuse, a retry-on-reset for idempotent media
   GETs is the fix; if it is YouTube resetting the sandbox
   IP, it may not reproduce on Joel's box.
4. **The R24 watch-page stall and metadata flake** are
   unchanged by this round (no fix was attempted for them
   here) and still await the backtrace on Joel's box.

## Round 29 — IO-step instrumentation (F-020) + typeof audit + videoplayback diagnosis

Joel resumed this round 2026-11-18 ("im home lets do this")
after parking it that morning. Headline: **the feed is STILL
stalled — `youtubei/v1/browse` fires 0x in 300 s — but all
three Round 28 leads closed with hard evidence, and the
watch-page gate (added this round, see below) shows the
reloaded player plays through in the pipeline with
burst-delivered position updates — the R24 event-delivery
block, caught on the reloaded player.** Full gate logs in `~/workspace/harness-tmp/frost/`
(`r29-gate-yt.log`, `r29-gate-yahoo.log`, `r29-gate-watch.log`,
`r29-gate-p11b.log`, `r29-regress-*.log`).

Shipped (F-020, Freezer): `maybe_compute_intersection_output`
now also names the missing piece (`root_bounds` / `target_rect`
/ `root`); `update_intersection_observations_steps` counts
targets/computed/none_*/fallback/delay_skip/readded/queued per
document, and `document.rs` logs one `IO: steps doc=<host>
readyState=<rs> targets=… computed=… none_tr=… none_rb=…
none_root=… fallback=… delay_skip=… readded=… queued=…` line
(first run always, then at most 1/s while interesting; clean
runs silent); `script_thread.rs` logs `IO: skip doc=<host>
readyState=<rs> render_blocked=true rb_count=<n>
io_observers=<n>` when a render-blocked document with observers
is skipped at step 3. Shipped (frost-side):
`FrostDelegate::show_console_message` is implemented
(ERRCAP-prefixed lines go to `log::info!("CONSOLE: …")`,
everything else to debug), so the errcap's full bootstrap
chain now lands in the engine log; the errcap is v0.4.0 "R29
Error Capture" (banner cap 40 → 200 lines, every line also
`console.log('ERRCAP ' + …)` — no recursion, console.log is
not wrapped).

Lead 1 (IO delivery on the real page): **EXPLAINED — it is
render-blocking, not a bug, and delivery is correct once the
document unblocks.** YouTube 300 s gate (Snowplow OFF):
537 `IO: skip` lines, first 00:33:26, last 00:35:00 —
`rb_count=1` for 535 of them (2 at 3) — the document is
render-blocked by parser-inserted scripts at ~20–27 skips/s
for ~12 s per document load. The errcap probe fires
`isIntersecting=true ratio=1` at +27.6 s (doc A) and +12.9 s
(doc B, after the themeRefresh reload); Yahoo fires at +14.8 s
(328 skips over ~11 s). After `readyState=Complete` the steps
run freely (YouTube: up to 49 targets, all computed, entries
queued and delivered). F-019's re-add works live (`readded=2`
in the Loading runs). The `IO: steps` summaries also show
`none_tr` > 0 with `fallback` == `none_tr` and `none_rb` == 0
everywhere — targets whose rect is unavailable during load get
the skipped/false fallback, never a missing root. The 5 s
"NOT FIRED" is the probe's arbitrary threshold firing before
the document unblocks, not a defect. **There is no IO delivery
bug left on the real page.** Local re-gate (p11b): F-019
intact — `RAF1=124 RAF2=124 IO1=199 IO1x2=2019 IO2=2020
count=2` (IO1 true at 199 ms, same frame as rAF; the 2,019 ms
second firing is the legitimate true→false transition), and
the F-020 lines log live (`IO: steps … Loading targets=1
computed=1 … queued=1`, then `Complete targets=2 computed=2
… queued=2`, 0 skips).

Lead 2 (what triggers `browse`): **the typeof audit rules
out every guard, and the bootstrap chain is fully captured —
but the player POST is now a sharper mystery, not a closed
lead.** Live JS audit (~14 MB: homepage 978,348 B + base.js
2,701,764 B + kevlar.js 9,803,874 B + 6 bootstrap scripts):
`typeof` counts — IntersectionObserver 1, ResizeObserver 1,
MutationObserver 1, Promise 1, MessageChannel 1, Worker 3,
AbortController 2, AbortSignal 2, URL 3, TextEncoder 6,
PerformanceObserver 1; requestIdleCallback / scheduler / fetch
/ XHR / queueMicrotask / customElements / indexedDB /
localStorage / BroadcastChannel / SharedWorker /
OffscreenCanvas / visualViewport / matchMedia /
requestAnimationFrame / IntersectionObserverEntry /
document.fonts / navigator.serviceWorker all 0; no `'X' in
window` hits. The single IO typeof is a perf-monitor path that
takes the IO branch since R26; the ResizeObserver typeof is a
component guard. **No typeof guard gates the feed.** The feed
path has an offline-fallback layer (`Dat` class):
`localFallbackInnertubeResponse` for `FEwhat_to_watch` awaits
an uplift promise then returns store entities or an empty-ish
response; `shouldPreferLocalResponse` is false for
FEwhat_to_watch, so the app prefers the NETWORK response —
but the live gate shows it never issues the call. The console
mirror captured the full first-30-s chain in the engine log
(427 ERRCAP lines): rIC fires at +199 ms, IO fires at +27.6 s,
`readyState=complete` at +27.8 s, player POST at +28.0 s,
then guide / log_event / feedback / icon SVGs / ads — and
`browse` never appears in 300 s. **First genuine page JS
error ever captured on YouTube:** `ERR: this.shownItems.some
is not a function @ …kevlar_base…` at +109,396 ms (doc B) —
recorded, not yet attributed (could be the app running
degraded, or a real-browser error too; unverified either way).
**The player POST, sharpened:** the app attempts it page-side
(15 banner lines) and it resolves 200 with real network
latency (164 ms–1.5 s), yet **0 reach http_loader** (11 POSTs
total in the log: log_event 7, guide 2, feedback 1,
GenerateIT 1; player 0, browse 0; 0 OPTIONS preflights).
guide/log_event/feedback POSTs DO reach http_loader and return
200s, so fetch POSTs in general work and log. The signed
videoplayback URLs the app then fetches can only come from a
genuine player response (the curl'd homepage HTML contains 0
`videoplayback` and 0 `streamingData`). Ruled out from code:
HTTP cache (`construct_response` returns None for non-GET),
disk cache (no `cache.sqlite3` anywhere; tempdir per run),
embedder intercept (Frost's delegate only cancels or drops),
preload (GET-only, step 10 requires GET), ServiceWorker
(undefined), and every fetch path funnels through
`http_network_fetch`, whose `info!("{:?} request for {}")`
line is unconditional. **The mechanism by which the player
POST is answered with a real 200 + real streaming data while
leaving no trace in Servo's network stack is UNRESOLVED.**
Round 30 lead: log method+URL at `http_fetch` entry — if the
player fetch never enters the net crate, the break is between
`window.fetch` and the net crate; if it enters, find the
early exit.

Lead 3 (videoplayback deaths): **REFUTED as connection
reuse — it is the server (or path) resetting fresh
connections from the sandbox IP; no retry fix warranted.**
YouTube 300 s gate: F-018 fired 6x, all `videoplayback`
`itag=18` to `rr1---sn-aigzrnld.googlevideo.com`, all
`ConnectionReset` on SendRequest — same count/shape as R28.
Page-side they surface as `FETCH-ERR: TypeError: Network
error: CORS check failed` (the fetch fails before any
response exists, so the CORS check has nothing to check —
a symptom, not a policy issue). The other host,
`rr2---sn-q4flrnel`, answered all 5 of its videoplayback
GETs with 403 (same client, same pool — connections work,
the server refuses at the application layer). Crucially, the
rr1 resets hit on FIRST contact with that host (no prior rr1
connection could have been pooled), and the app already
retries the same URL (twice at 00:35:37–38, again at
00:36:38 — all reset). Curl control probe (fresh connection,
different TLS stack): the exact failing rr1 URL → exit 52,
empty reply, 0.8 s; the rr2 URL → 403, 0.62 s. Same failure
shape from curl rules out both stale pooled connections and
our rustls/hyper stack. On Joel's box (different IP, not
bot-flagged) this likely does not reproduce — consistent with
his box playing video while the sandbox cannot fetch itag-18
segments.

Watch-page gate (ADDED this round, themeRefresh lead):
`watch?v=jNQXAC9IVRw`, 180 s, Snowplow OFF. Initial GET
00:52:57, **themeRefresh reload GET 00:54:10 (73 s in)**,
player 1 attached 00:53:36, player 2 attached 00:55:17.
**CORRECTION (Doc verification 2026-11-19: the builder's
report said neither player ever got `MetadataUpdated` and
there were zero MSE drains — the log shows the opposite, so
that claim is withdrawn):** the UI at 180 s does show
`0:00 / 0:19` (screenshot `r29-watch-180s.jpg`) and no
position update arrived inside the 180 s window — but the
log kept running, and ~52–78 s after the window the
reloaded player (player 2) got `MetadataUpdated`
(320x240, ISO fMP4, 00:56:49), `DurationChanged 19.064s`,
6 drains all `pushed=1, dropped=0` with buffered full
[(0.0, 19.06)] audio / [(0.0, 18.93)] video (00:56:31–37),
`StateChanged Playing` (00:56:55), positions 0.50→3.51 in
one burst (00:56:55) then 17.10→18.60 + 290.82 in a second
burst (00:57:15), and `EndOfStream` (00:57:15). **The
reloaded player DOES start and DOES play to the end in the
pipeline — what is broken is live event delivery: position
updates arrive in bursts, the R24 event-delivery block,
not the never-starts flake.** Joel's VepOepAA7_U shot (0:00
+ spinner after the reload, Firefox playing it fine) is
consistent with the same shape caught earlier — the UI
frozen at 0:00 while the pipeline is behind — though a
genuine never-starts flake (R24's second bug) still cannot
be ruled out on his box from one shot. The app DID get
player POST 200s page-side (4 attempts, 2 resolved 200) and
videoplayback 200s (SABR POSTs, `sabr=1`); one
videoplayback fetch was aborted (`AbortError` at +53,106 ms,
reloaded doc). IO-CB fired at +27.2 s (doc A) and ~+26 s
(doc B); 1,169 IO skips. No crash, no panic; Frost stayed up.
Reading for the themeRefresh question: the reload discards
the in-progress player and forces a full re-bootstrap in
every run (homepage: reload GET 31 s in, new document
commits ~55 s later; watch: 73 s in). On Joel's faster box
the same sequence lands mid-playback (~0:22) and the
reloaded player then plays with burst-delivered events
(this gate's shape, the R24 block) — or, on other runs,
never starts at all (the metadata flake); the reload is
the trigger, the flake/block is the mechanism. The R24 local repro refuted only the narrow
claim "discarded player poisons the next"; it did not test
the real YouTube reload. **The R24 stall + metadata flake
are still OUT OF SCOPE for a fix here and still await the
backtrace on Joel's box** (0.22.0 is fully stripped; Joel
approved a symbolized build 2026-11-18 ~17:37 — package
from the Round 29 tree once delivered).

Full test suite 89/89 (frost bin 54 / frozen 18 / extensions
9 / inspector 8) plus servo-config 3/3 — log
`~/workspace/harness-tmp/frost/r29-test.log`, EXIT=0. (The
two `^error` lines in the log are the benign
`XDG_RUNTIME_DIR is invalid` warnings from a GL-probe test,
not failures.) Regression ALL PASS: R18 probe ALL PASS
(F-015 steer intact), R19 buffered [0.00–10.00] +
loadedmetadata + readyState 4, R20 real DASH video
[0.00–18.93] / audio [0.00–19.06] + Playing + positions to
18.67, youtube.com document 200 with 0 SendRequest errors,
local H.264 to end (00:05/00:05 + EndOfStream), plain WebM to
3.04, homepage + banner, palette, split view (opened, closed
via palette), menu, Freeze archive
`example-domain-20261119-0106.html` (2,947 B) + index.json
correct ("Frozen: Example Domain"), Settings Snowplow: Off
saved and persisted in settings.json, Inspector F12 35 nodes.
Zero panics in any gate log. (The R19/R20 logs each contain
one unrelated `Failed to create PipeWire event context`
ALSOFT line and R20 one `Failed to initialize ClientStorage`
line — sandbox environment noise, not probe failures.)

Incidents: a service restart SIGTERM'd the full test suite
mid-link (attempt 2, after attempt 1 was OOM-killed by
running concurrently with the PornBrowser builder); attempt 3
resumed incrementally and passed. The harness (Xvfb:97 +
18080/18099/18101) died mid-round — the YouTube-gate Frost
exited silently with it (last log line 00:38:52, no panic,
no process at the next check; a relaunch then panicked in
winit `Create event loop` until the harness was rebuilt).
Harness lesson: verify Xvfb + servers immediately before
each gate launch; never `cd X && cmd &` (the `cd` is
backgrounded with the job — use a subshell). Menu-row
coordinates in the harness notes drifted ~22 px (Split view
is at y≈309 now, not 287) — measure from the screenshot,
don't trust the old numbers. Cosmetic: direct mp4/webm
navigation renders the video frame doubled (two offset
copies); the H.264/WebM clips still play to the end.

## Leads for the next YouTube round (were Round 30's leads; grid jumped ahead 2026-11-19, Joel)

1. **Player POST mechanism (sharpest):** add an `info!` log
   at `http_fetch` entry (method + URL) and re-gate YouTube.
   If the player POST never appears there, the fetch never
   enters the net crate — look at script-thread fetch
   dispatch. If it appears, trace the early exit. Second
   probe: in the errcap fetch wrapper, log
   `r.clone().text()` length for `youtubei/v1/player`
   responses only — does the page-side 200 carry real
   streamingData or an empty/error body?
   **DONE in Round 31 — see the Round 31 section:** the
   player POST never appears at `http_fetch` (0 entries in
   300 s while 11 page-side 200s carry bodies up to
   262 KB); full dispatch audit leaves no in-crate path.
   Next lead is the Round 32 probe (log `ofetch.toString()`
   from page JS).
2. **Watch-page event delivery:** the reloaded player
   appends and drains fine (9 appends, 6 drains, all pushed,
   buffered full) and plays to EndOfStream — but position
   updates arrive in two bursts well after the fact, and the
   UI sits at 0:00 meanwhile. Instrument the media-event
   path (what holds `PositionChanged` / `MetadataUpdated`
   between the player and the script thread, and what
   releases the burst) — the R24 block, now reproduced on
   the reloaded player. (The earlier draft of this lead said
   0 drains; Doc verification found 6 drains in
   `r29-gate-watch.log` — that claim is withdrawn.)
3. **What triggers `browse` is still unknown.** IO, rIC,
   fetch/XHR all work; the app progresses (guide, log_event,
   feedback, player) and never attempts `browse` in 300 s.
   The typeof audit and the bootstrap chain rule out missing
   APIs. Next: look at what the app does with the player
   response — if the player response is real, why does the
   feed never follow? (May be the same root as lead 1.)
4. **The R24 watch-page stall and metadata flake** are
   unchanged by this round (no fix was attempted for them
   here) and still await the backtrace on Joel's box — from
   the symbolized build, once packaged from the Round 29
   tree.

## Round 30 — CSS Grid / aspect-ratio (F-021)

Joel re-sequenced this himself 2026-11-19 ~08:58: grid jumps
ahead of the YouTube work (he is not home to help with the
YouTube on-box testing). The YouTube leads from Round 29
carry over to the next YouTube round, after grid.

Headline: **CSS Grid itself already worked — the real gap
was `aspect-ratio` on non-replaced (flow) boxes.** The survey
proved it with screenshot-verified probes on the Round 29
binary, before any fix:

- P2 explicit tracks (`200px 1fr 100px`, rows 80/120, gap
  10px, auto-placement): PERFECT. P3 explicit placement
  (`grid-column: 1/3`, `grid-row: 2`) + nested grid: CORRECT.
  P4 `grid-template-areas`: CORRECT. P6 grid items with
  `aspect-ratio: 16/10`: WORKS (~293x183 boxes) — Taffy
  0.15.1 applies the ratio to grid items. P7 = P1 with
  `aspect-ratio` replaced by `height:112px`: PERFECT (20 grey
  tiles, 5 per row, 4 rows).
- P1 (verbatim PornBrowser Round 5 home page: grid +
  `.thumb { aspect-ratio:16/10 }` inside each tile): BLANK —
  reproduces the 2026-11-19 ~00:13 on-box report exactly.
  P5 isolated: red div (`width:200px; aspect-ratio:16/10`)
  INVISIBLE; green control (`height:125px`) renders 200x125.
  P8 flex items (`width:100px; aspect-ratio:1/1`): BLANK.

So the brief's premise ("the old flow layout has no grid
support at all") was wrong: Freezer's layout crate uses
Taffy 0.15.1 for grid (`taffy::compute_grid_layout`,
`GridContainerStyle`/`GridItemStyle` in
`components/layout/taffy/`), and the PornBrowser side chat's
"no grid support" reading came from grepping `flow/mod.rs`
only. The collapse was `aspect-ratio` on flow-laid-out
boxes: `tentative_block_content_size_with_dependency`
(`formatting_contexts.rs`) returns `Some` only for
`Replaced` contents, so an auto-height flow box with a
definite width and `aspect-ratio` resolved its block size
from content (zero for empty boxes).

## What shipped (F-021, freezer)

Three parts, all in `freezer/components/layout/`:

1. **The ratio never reached non-replaced boxes
   (root cause).** `IndependentFormattingContext::
   preferred_aspect_ratio` returned `None` for everything
   except replaced elements (`// TODO: support preferred
   aspect ratios on non-replaced boxes`). It now returns
   the style's ratio (`style.preferred_aspect_ratio(None,
   ...)` — non-replaced boxes have no natural ratio),
   mirroring the broken-image arm of the replaced path.
   The tentative block content size still ignores the ratio
   for flow boxes, so this does not feed a stretch-derived
   size into intrinsic sizing.
2. **The transfer itself.** New helper
   `block_size_with_aspect_ratio` (`style_ext.rs`):
   css-sizing-4 §2.2 — when a box has a preferred aspect
   ratio and its block size is automatic, the block size is
   calculated from the used inline size through the ratio,
   handed to the sizing machinery as the content-size
   fallback so min/max block sizes still clamp it. Applied
   at the final block-size resolution in: flow in-flow
   blocks (both branches of
   `layout_in_flow_block_level_sequentially` + the
   independent BFC-root layout + the float/atomic layout),
   same-formatting-context blocks (ratio resolved from the
   style directly — these have no
   `IndependentFormattingContext`), absolutely positioned
   boxes (`positioned.rs`), flex items (hypothetical cross
   size + used cross size), and the Taffy grid-item leaf's
   PerformLayout fallback.
3. **A placement bug the transfer exposed.**
   `PlacementState::place_fragment` returns early for a
   first in-flow child whose margins collapse through it,
   assuming such a fragment is ~zero height — but an
   aspect-ratio box has a nonzero height with zero content,
   so the following sibling was placed on top of it (the
   P5 red div was laid out at 125 px yet painted invisible
   under the green div). The early return now still
   advances the block position by the fragment's size.
   The `collapsed_through` flag itself is left alone: per
   CSS 2.1 §8.3.1 the margins of a computed-`auto`-height
   box do collapse through it; only the position advance
   was missing.

Known deviation, kept deliberately: intrinsic keywords in
the block sizing properties (`min-content`, `max-content`,
`fit-content`) also resolve through the ratio-derived size.
That matches the long-standing replaced-element behaviour
(the ratio-derived size is the tentative block content size
there); the strict spec reading applies the transfer to
automatic sizes only.

## Gates (Round 30 binary, screenshot-verified firsthand)

All 12 grid/aspect probes pass (pages in
`r13-test/r30-grid/`, shots `r30-p*-fix*.jpg`):

- P1 PornBrowser home page: 20 grey tiles, 5 per row, 4
  rows — the exact page that was blank on Joel's box.
- P2 explicit tracks, P3 explicit placement + nested grid,
  P4 template areas: still correct (unchanged from survey).
- P5 flow aspect-ratio: red 200x125 renders above the green
  control (verified also by JS: `offsetHeight=125`,
  `getBoundingClientRect` 200x125).
- P6 grid-item aspect-ratio, P7 grid + explicit height:
  still correct.
- P8 flex items (stretch), P9 flex items
  (`align-items:flex-start`): three 100x100 squares each.
- P10 floated aspect-ratio box: 200x125 float, text beside
  it. P11 aspect-ratio box with 300 px-tall content: outer
  keeps the ratio height, content overflows. P12
  absolutely positioned aspect-ratio box: 200x125.

Regression (same binary): R18 MSE probe ALL PASS (30/30
checks); R19 buffered [0.00-10.00]; R20 real YouTube DASH
segments buffered [0.00-18.93] video / [0.00-19.06] audio;
youtube.com loads with 0 PROTOCOL_ERROR (document 200,
skeleton — the feed gap is unchanged, as expected);
local H.264 plays to end (PositionChanged 5.00 +
EndOfStream); plain WebM ALL PASS (duration 3.049, 0.00 to
3.01); example.org renders; command palette opens with the
full command list; hamburger menu opens; split view shows
two panes; Freeze writes `example-domain-20261119-1722.html`
+ index.json correct ("Example Domain"); Settings Snowplow
toggles Off (persists `snowplow: false`) and back On
(persists `true`); Inspector F12 shows 35 nodes on
example.org. Zero panics in any gate log.

Tests: **89/89** (frost bin 54, frozen 18, extensions 9,
inspector 8) + Freezer servo-config **3/3** — full suite run
to completion by the finisher (the first builder's detached
run survived its end; the frost test-binary link took ~23 min
in the sandbox, `rust-lld` at ~32% CPU the whole way — slow,
not stuck).

## What remains (grid / aspect-ratio)

- `repeat(auto-fill/auto-fit)`, `minmax()`, `subgrid`,
  and `auto` track-sizing edge cases were not probed —
  Taffy owns them; only explicit tracks, gap,
  auto-placement, explicit placement, areas, and nesting
  were verified.
- Inline content sizes of an aspect-ratio box with text
  content do not yet account for the ratio (the ratio is
  not fed into `ConstraintSpace` for same-formatting-
  context blocks); definite-width and stretch cases are
  unaffected.
- Percentage-height children of an aspect-ratio box still
  resolve against an indefinite tentative block size (the
  tentative size is ratio-unaware); the box's own height is
  correct, the children's `%` heights are not.
- Column-flex aspect-ratio transfer (cross = inline size
  from definite main/block size) is not implemented; row
  flex is.
- Buttons with `aspect-ratio` are not covered (the button-
  centering resolve at `flow/mod.rs` was left alone).
- The Taffy leaf's ComputeSize (measurement) path was
  deliberately left to Taffy — P6 proves it applies the
  ratio there; only the PerformLayout fallback was patched.

## Build incidents this round

- `rustfmt` run without `--check` reformatted 12 extra
  files (it follows `mod` declarations from `flow/mod.rs`);
  the tree was restored with `git checkout` and the F-021
  edits re-applied cleanly. Lesson: never run bare
  `rustfmt` on a file in this tree; the installed rustfmt
  also disagrees with the tree's pinned style (135
  pre-existing diffs on the pristine tree).
- First F-021 build gated P1/P5/P8 as still failing: the
  resolve-site patches were no-ops because the ratio was
  `None` at every site (the `preferred_aspect_ratio` TODO
  stub). The flex sites worked first (they read the ratio
  through the fixed method), which is what localized the
  stub. A second miss: plain in-flow divs go through
  `SameFormattingContextBlock::layout_in_flow_non_replaced_
  block_level`, not the `IndependentFormattingContext`
  path — that site was added after the flex result pointed
  at it. A third miss: the collapsed-through placement
  early return, found by pixel-sampling (0 red pixels) plus
  a JS `offsetHeight` probe (125) proving layout was right
  and painting/placement was wrong.
- The full test suite's first run was SIGTERM'd after
  ~20 min with no output (piped through `tail`); the
  re-run writes to `r30-cargo-test.log` via a detached
  `setsid nohup` so progress survives.

## Round 31 — player POST, traced to the net-crate door (F-022) + PLAYER-BODY probe

Gate: 300 s YouTube homepage, Snowplow OFF, R31 binary
(522,385,544 B, `-C link-arg=-Wl,--strip-debug`), errcap v0.5.0.
Log: `r31-gate-yt.log` (44,147 lines). Screenshots:
`r31-yt-105s.jpg`, `r31-yt-300s.jpg`. Tests: 89/89 +
servo-config 3/3 (see Build incidents: the plain `cargo test`
run stops at the first failing package, so the frost-bin GL
probes were re-run directly with the launcher Mesa env).

### What shipped

- **F-022 (freezer, `components/net/http_loader.rs:900-906`):**
  one `info!` log at `http_fetch` entry —
  `info!("http_fetch {:?} {}", request.method, request.current_url());`
  — so every fetch that enters the net crate is visible, and
  the decisive comparison is F-022 vs the existing
  `http_network_fetch` log (`"{:?} request for {}"`,
  line 2190): enters `http_fetch` but not
  `http_network_fetch` = cache/preflight early exit inside
  the net crate; never appears at `http_fetch` = never
  enters the net crate at all.
- **Errcap v0.5.0 (`extensions/r25-errcap/`, "R31 Error
  Capture"):** all R29 markers → R31, plus the PLAYER-BODY
  probe: in the fetch `.then` success handler, for URLs
  containing `youtubei/v1/player`, `r.clone().text()` logs
  `PLAYER-BODY len=<n> <first 120 chars>` (the clone keeps
  the app's stream untouched).

### The decisive result: the player POST never enters the net crate

Page-side (errcap) vs engine-side (F-022 / netfetch), 300 s:

| layer | youtubei/v1/player | youtubei/v1/log_event | browse |
|---|---|---|---|
| errcap FETCH/XHR attempts | 11 POST (fetch) | 17 (XHR) | 0 |
| errcap FETCH-OK / XHR-OK 200 | 11 | — | 0 |
| F-022 `http_fetch` entries | **0** | 5 POST | 0 |
| `http_network_fetch` entries | **0** | 5 POST | 0 |

Other POSTs reach the net crate fine (`generate_204` 6,
`api/stats/atr` 6, `GenerateIT` 1 at `http_fetch`). Only the
player POST is missing — under any method: `http_fetch GET
youtubei` is also 0, and no `http_fetch` line contains
`player` except `cplayer=UNIPLAYER` params and `/s/player/`
static assets (`www-player.css`, `player_es6.../base.js`).

The F-022 log itself is proven live by the gate: 315
`http_fetch` entries, 260 `http_network_fetch` entries, and
the unique-URL sets are IDENTICAL (156/156) — every fetch
that entered `http_fetch` also reached `http_network_fetch`.
The 315-vs-260 count gap is repeat requests for the same
URLs, not an early exit. So there is no gap between
`http_fetch` and `http_network_fetch` to hide in: the player
POST is absent at the net crate's front door.

PLAYER-BODY (11 lines, pairing 1:1 with the 11 FETCH-OKs;
no FETCH-ERROR anywhere in 300 s; the app stops calling
player after ~45 s):

```
[2026-11-21T07:16:06Z INFO  frozen] CONSOLE: ERRCAP +27592ms PLAYER-BODY: len=328 {"adPlacements":true,"playerAds":true,"playerResponse":{"adPlacements":true,"playerAds":true,"playerConfig": {"audioConf
[2026-11-21T07:16:18Z INFO  frozen] CONSOLE: ERRCAP +19711ms PLAYER-BODY: len=96 [{"adPlacements":true,"playerAds":true,"playerResponse":{"adPlacements":true,"playerAds":true}}]
[2026-11-21T07:16:41Z INFO  frozen] CONSOLE: ERRCAP +42523ms PLAYER-BODY: len=61773 {"responseContext":{"serviceTrackingParams":[{"service":"GFEEDBACK","params":[{"key":"kc","value":"66"},{"key":"cw","val
[2026-11-21T07:16:43Z INFO  frozen] CONSOLE: ERRCAP +44134ms PLAYER-BODY: len=262568 {"responseContext":{"serviceTrackingParams":[{"service":"GFEEDBACK","params":[{"key":"ipcc","value":"0"},{"key":"is_view
```

The bodies are NOT empty/error bodies: 5 early ones are
small ad-slot stubs (len 328 ×3, len 96 ×2), and 6 later ones
are full-size payloads (len 61,773 ×2, 61,792 ×2, 262,568
×2 — exact duplicates in pairs, the signature of request
dedup). Only the first 120 chars were logged, so whether
they contain `streamingData` / `playabilityStatus` is NOT
established — that is a Round 32 probe (below). What IS
established: the page-side 200s carry substantial bodies,
while the engine never sees the requests.

### Code audit: no path returns a real https response without http_fetch

Every branch of the dispatch was read this round
(`components/net/fetch/methods.rs`, `fetch_params.rs`,
`request_interceptor.rs`, `resource_thread.rs`,
`protocols/mod.rs`, `http_loader.rs`):

- `fetch()` → `main_fetch()` step 12 is the only dispatch.
  For an https CORS POST the branches are: preload
  candidate (GET-only — `fetch()` step 10 gates the whole
  preload lookup on `method == GET`, and
  `consume_preloaded_resource` is keyed on
  url/destination/mode/credentials, never consulted for a
  POST); same-origin + Basic tainting → `scheme_fetch`;
  `data:` / custom fetchable protocols → `scheme_fetch`;
  Navigate/WebSocket → `scheme_fetch`; CORS preflight or
  plain CORS → `http_fetch` (lines 632 / 648). And
  `scheme_fetch` for http/https ALSO calls `http_fetch`.
  No branch returns a real https response without it.
- Embedder intercept (`request_interceptor.rs`,
  `main_fetch` step 11): runs before step 12 and CAN
  synthesize a response (Start + SendBodyData +
  FinishLoad) — but Frost's `handle_load`
  (`crates/frozen/src/snowplow.rs`) returns false
  immediately with Snowplow OFF, dropping the load, which
  the responder translates to DoNotIntercept; and
  `FROST_BLOCK_URL_SUBSTRING` was unset in this gate.
  Ruled out by the code and by the gate config.
- HTTP cache / disk cache: consulted only inside
  `http_network_or_cache_fetch`, whose only call sites are
  in `http_loader.rs` (inside `http_fetch` /
  `http_redirect_fetch`). A cache hit would still log at
  F-022. Ruled out by call-site enumeration.
- `res_init_` / `FetchRedirect` path
  (`resource_thread.rs:865`): only for redirect replays,
  not initial fetches. Ruled out.
- Custom protocols: only `data`, `blob`, `file` are
  registered; `http`/`https` are in FORBIDDEN_SCHEMES.
  Ruled out.
- ServiceWorker: undefined on the page (errcap banner:
  `SW=undefined`). Ruled out (again).
- The script thread has no HTTP client of its own (no
  hyper/reqwest in `components/script`; the `http` crate
  there is types only). Page JS can only reach the network
  through the resource thread → net crate → `http_fetch`.

Conclusion: **the player POST never reaches the net
crate's `fetch()` — it is answered upstream of the
resource thread, in page-JS / script-thread land.**
The engine is exonerated as the place the request dies;
the request never arrives.

### Leading hypothesis (unproven): YouTube's own network layer serves it

The errcap installs at HeadParsed (document-start), but
YouTube's network libraries load as `<head>` scripts and
run BEFORE that: `spf.js`, `scheduler.js`, and
`network.js` all appear in the gate log as early GETs, and
the page's own feature flags include `web_unified_fetch`,
`kevlar_prefetch`, `web_prefetch_preload_video`. If
`network.js` (YouTube's unified-fetch layer) wraps
`window.fetch` before the errcap installs, then the
errcap's `ofetch` is YouTube's wrapper, not the engine's
fetch — and that wrapper can serve player calls from its
own in-memory/dedup state while passing other endpoints
(`generate_204`, `stats/atr`, `log_event`) through to the
real engine fetch, which is exactly the split the gate
shows. The paired duplicate bodies and the synthetic
ad-stub bodies fit a client-side player-response cache;
the 262 KB bodies do NOT match the embedded
`ytInitialData` (116,848 bytes in a fresh curl of the
homepage, and it contains no `videoRenderer` / `videoId` /
`streamingData` — the server serves an empty feed to this
client), so they are not the initial HTML either.

This hypothesis is NOT proven: it needs the Round 32
probe (below). What would falsify it: the player POST
appearing at a script-thread / resource-thread entry log.

### Feed: still stalled

`youtubei/v1/browse` fires 0× in 300 s. Both screenshots
show the same dark skeleton (grey boxes, no tiles) with the
address bar at `?themeRefresh=1` (the reload happened
again, ~20 s after nav; 3 errcap installs = initial doc +
reload + one more document). Pixel-diff 105 s vs 300 s:
page body below the banner 1.48% differing (frozen
skeleton; the banner region is 29.8% different only
because its log text scrolls). F-018 network errors: 0 in
300 s (no videoplayback deaths this run). IO fired at
+7.4 s (doc A) after the usual render-blocking delay;
rIC fired at +13 ms. `SCH=undefined postTask=n/a`,
`SW=undefined`, `VIS=visible` on the banner — same as R27.

No fix shipped this round: the cause is narrowed to
upstream of the net crate, not nailed down.

### Build incidents this round

- `cargo rustc -p frost --bins -C link-arg=...` fails on
  cargo 1.99.1 (`unexpected argument '-C'`): the rustc
  flag needs `--` first —
  `cargo rustc -p frost --bins -- -C link-arg=-Wl,--strip-debug`.
  The round docs (DIAGNOSIS/plan notes) wrote the command
  without `--`; the gate binary was built with the `--`
  form (1m 42s, EXIT=0, 522,385,544 B).
- `cargo test --workspace` aborts the whole workspace run
  at the first failing package: the frost-bin GL probes
  fail under plain `cargo test` (no Mesa env), so the log
  held only the frost result (52 passed, 2 failed). The
  remaining packages were verified by running the
  cargo-built test binaries directly with the launcher Mesa
  env (`T=~/workspace/harness-tmp`, `DISPLAY=:97`):
  frost bin 54/54, frozen 18/18, extensions 9/9,
  inspector 8/8, freezer servo-config 3/3 — **89/89 + 3/3**,
  baseline preserved. (The 2 failures are the known
  env-sensitive GL probes, not a regression.)

## Leads for Round 32 (not diagnosed in this round)

1. **Sharpest: prove or kill the unified-fetch hypothesis
   from page JS, no engine rebuild.** In the errcap fetch
   wrapper, log `Function.prototype.toString.call(ofetch)
   .slice(0, 200)` once at install: a native engine fetch
   prints `function fetch() { [native code] }`; YouTube's
   wrapper prints minified JS. If it is YouTube's wrapper,
   the player POST never was an engine fetch — and the
   feed question becomes what YouTube's wrapper does with
   player responses (and why `browse` is never attempted).
   Second probe in the same errcap change: log more of the
   PLAYER-BODY (first 500 chars + whether it contains
   `streamingData` / `playabilityStatus` / `videoDetails`)
   to settle whether the 262 KB bodies are real player
   responses or browse-shaped payloads.
2. **Belt-and-braces engine-side confirmation (only if
   probe 1 is inconclusive):** add an `info!` log at the
   resource-thread `fetch()` entry (`resource_thread.rs`)
   and at the script-thread `Fetch` entry. If the player
   POST appears at neither, it never reaches the engine;
   if it appears at the script log but not the resource
   log, it dies between script and resource thread.
3. **Feed trigger still unknown** (lead 3 from the R29
   list, unchanged): the app has the empty feed in
   `ytInitialData` and never attempts `browse`. The
   player-response question (lead 1) may be the same root.
4. **Watch-page event delivery (R24 block) and the
   metadata flake** are unchanged by this round and still
   await the backtrace on Joel's box.

## Round 32 — fetch-wrapper probe: fetch is native at install (errcap v0.6.0), PLAYER-BODY settled

Gate: 300 s YouTube homepage, Snowplow OFF, **R31 F-022 binary
REUSED** (522,385,544 B — no Rust changed this round, no engine
rebuild, no freezer commit). Log: `r32-gate-yt.log` (44,472
lines). Screenshots: `r32-yt-105s.jpg`, `r32-yt-300s.jpg`.
Tests: 89/89 + servo-config 3/3 (test binaries run directly
with the launcher Mesa env; no Rust changed, baseline holds).

### What shipped (errcap v0.6.0, `extensions/r25-errcap/` only)

- **FETCH-IDENT:** at install, BEFORE wrapping fetch, log
  `Function.prototype.toString.call(fn).slice(0, 200)` for
  `window.fetch`, `XMLHttpRequest.prototype.open`, and
  `XMLHttpRequest.prototype.send` — one line each. The
  identity of `ofetch` is fixed at install: it is the
  reference this wrapper will call for the whole page
  lifetime.
- **PLAYER-BODY extended:** first 500 chars (was 120) plus
  `streamingData=` / `playabilityStatus=` presence flags,
  for `youtubei/v1/player` responses only.

### Decisive result 1 — FETCH-IDENT kills the wrapper-before-install hypothesis

Two documents installed the errcap (initial 07:40:45Z, and
the `themeRefresh` reload 07:41:30Z). In BOTH, at +3–6 ms
after install:

```
[2026-11-21T07:40:45Z INFO  frozen] CONSOLE: ERRCAP +5ms FETCH-IDENT: fetch=function fetch() {
        [native code]
    }
```

Same for `xhr.open` (`function open() { [native code] }`)
and `xhr.send` (`function send() { [native code] }`), in
both documents (6 FETCH-IDENT lines total). So
`window.fetch` at errcap install **IS the native engine
fetch** — YouTube's `network.js` had NOT wrapped it before
the errcap installed. The R31 leading hypothesis is dead
in the form stated.

**Reading trap for future rounds:** SpiderMonkey prints a
native function's `toString` across THREE physical log
lines (`function fetch() {` / `    [native code]` / `}`),
so a single-line grep looks truncated at the `{` — read
the continuation lines before concluding anything about
identity.

### Decisive result 2 — PLAYER-BODY settles the bodies

6 lines, pairing 1:1 with the 6 FETCH-OKs:

```
len=328 streamingData=false playabilityStatus=false   (x2)
len=96  streamingData=false playabilityStatus=false
len=61773  streamingData=true playabilityStatus=true
len=61792  streamingData=true playabilityStatus=true
len=262464 streamingData=true playabilityStatus=true
```

The 3 small bodies are ad-slot stubs (no streamingData,
confirmed as stubs). The 3 big bodies **ARE real player
responses containing `streamingData` and
`playabilityStatus`**. (R31's 262,568 B vs R32's 262,464 B:
different content, same shape.)

### The split reproduces exactly (300 s)

| layer | youtubei/v1/player | log_event | guide | feedback | browse |
|---|---|---|---|---|---|
| errcap FETCH/XHR attempts | 7 POST | 8 (XHR) | 2 | 1 | 0 |
| errcap FETCH-OK / XHR-OK 200 | 6 | 8 | 2 | 1 | 0 |
| F-022 `http_fetch` entries | **0** | 8 | 2 | 1 | 0 |
| `http_network_fetch` entries | **0** | 8 | 2 | 1 | 0 |

7th player attempt never settled: it fired at doc A
+43828 ms and the `themeRefresh` reload (~45 s) killed
the document before it resolved — no hung fetch, fully
reconciled. 0 FETCH-ERR, 0 FETCH-THROW, 0 F-018 network
errors. `http_fetch` total 514, netfetch 282, unique-URL
sets identical 263/263. `SW=undefined` (banner firstLine,
both screenshots; 0 SWREG lines). Screenshots: 105 s
skeleton with the native FETCH-IDENT lines visible in the
banner; 300 s page body fully black — the skeleton
placeholders disappear with no content arriving.

### The paradox sharpens

The app calls `window.fetch` — proven, the errcap wrapper
logs the attempts. The wrapper calls `ofetch`, which IS
the native fetch — proven, FETCH-IDENT. The promise
resolves 200 with a real `streamingData` body — proven,
FETCH-OK + PLAYER-BODY. Yet `http_fetch` sees nothing.
So the break is NOT page-JS wrapping before install, and
NOT the net crate's front door. It is somewhere between
the native fetch invocation and `http_fetch` entry — or
the response is produced without the network at all.
R31's audit conclusion ("no path returns a real https
response without passing `http_fetch`") is now in tension
with the data and must be re-examined: the audit covered
`fetch()` → `main_fetch()` step 12 → `http_fetch` and
`scheme_fetch` routing, but the script-thread Fetch API
implementation itself — where `window.fetch` builds the
request and hands it to the resource thread — was NOT
instrumented. That is the next bracket.

## Leads for Round 33 (not diagnosed in this round)

1. **Sharpest: bracket the break with logs at the two
   remaining dispatch points.** (a) Script-thread fetch
   entry — the `window.fetch` binding / FetchContext
   creation in `components/script` (log method + URL when
   a fetch is initiated from script). (b)
   Resource-thread `fetch()` entry
   (`components/net/fetch/mod.rs` — log method + URL).
   If (a) shows the player POST but (b) doesn't, it dies
   in the script→resource-thread handoff. If (b) shows it
   but `http_fetch` doesn't, it dies in `scheme_fetch`
   dispatch. This is the R32 leads item 2, now promoted.
2. **Cheap errcap-side addition in the same round:** log
   the fetch init's `mode` / `credentials` / `cache` /
   `referrerPolicy` for player calls in the wrapper — an
   unusual init could route differently. (Attempts all
   show `signal.aborted=false`; the init is otherwise
   unexamined.)
3. **Feed trigger still unknown** (browse 0× in every
   300 s gate, R25–R32). The player-response question may
   share the root, or the feed trigger is separate.
4. **Watch-page event delivery (R24 block) and the
   metadata flake** are unchanged by this round and still
   await the backtrace on Joel's box.
