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
