# Frost roadmap

Small parts, every step committed, every round shippable. A round is done
when its gate passes in the sandbox AND (for UI/engine behavior) Joel has
had a chance to run it on his box.

## Round 1 — Prove the loop ✅ (this round)

- Fork Servo v0.8.0 shallow + 3 proven engine patches as fork commits.
- Cargo workspace: `frost` (libcosmic skeleton), `frost-engine` (Servo
  wrapper), `frost-extensions`, `frost-inspector` (scaffolding + tests).
- Headless smoke test: load example.org, write a non-blank PNG.
- Docs: README, ARCHITECTURE, ROADMAP, FORK.md, clone-fork.sh.
- Gate: `cargo check --workspace` green + `smoke.png` non-blank.

## Round 2 — The window goes live ✅ (2026-11-13, sandbox gates passed)

- Wire `frost-engine` into the libcosmic app: content area shows the live
  Servo frame (texture upload per `FrameReady`), `spin()` driven by a
  16 ms UI tick. Renders are gated: nothing uploads while a load is in
  flight (`awaiting_load`) or a forced repaint is running.
- Toolbar: Back / Forward (disabled when the engine says there is nothing
  to go to) / Reload / Stop / address bar / hamburger menu. Stop is live
  since fork patch F-001 landed in this round.
- Address bar navigates; bare hostnames get `https://`; engine URL
  updates show in the bar but never clobber mid-typing.
- winit/libcosmic → `FrostInputEvent` mapping (keyboard, mouse, wheel,
  scroll) in `crates/frost/src/keymap.rs`. No `+8` xkb fix — that was
  Wayland-seat-only.
- Frost draws its own titlebar (the built-in libcosmic headerbar renders
  no window-control buttons on this setup): "Frost — <page title>" with
  real minimize / maximize-restores / close buttons (real iced window
  commands, each gated under Xvfb). `libcosmic/wgpu` fixes the
  softbuffer/depth-32 visual panic — proven on Joel's box 2026-11-12.
- Hamburger menu opens as a dropdown; Settings / Print load placeholder
  pages through the engine (real `frost://` pages are Round 3); **Open
  is a real file browser** (listing generated app-side from `$HOME`,
  folder clicks re-list via intercepted `frost://open?path=...`
  navigations, file clicks open the file via `file://`); Exit quits.
- Loading state in the status line; window resize resizes the webview.
- The partial first paint is fixed by a two-phase resize jiggle after
  every load (see ARCHITECTURE.md) — a workaround for a
  damage/initialization gap in the engine; a proper full-repaint trigger
  in the fork is the follow-up.
- History traversals never fire `LoadFinished`; the 15 s load-timeout
  safety net now also clears the loading state and status line.
- Tab strip is NOT in this round (the brief wins over the earlier
  roadmap sketch) — tabs are a later round.
- Gate: window launches under Xvfb in the sandbox, screenshot shows
  toolbar + rendered example.org; address-bar navigation, back/forward,
  reload, menu (all four entries), resize, and click-a-link gates pass.

## Round 3 — Internal pages + extensions v1 ✅ (2026-11-13, sandbox gates passed, finish pass re-ran live)

- Embedder loopback server serves `frost://settings`, `frost://print`,
  `frost://exit`, `frost://open`, `frost://extensions`, and
  `frost://md` (Markdown files render as formatted HTML instead of
  Servo's "Unknown content type" page). The address bar shows the
  `frost://` URL, never a `data:` URL.
- Settings page: theme (applies immediately), homepage, default search
  engine, "ask before quitting" — persisted to
  `$XDG_CONFIG_HOME/frost/settings.json` and honored on next launch
  (proven: saved homepage + Light theme survived a restart); invalid
  homepages are rejected with a plain message.
- Extension host v1: load a manifest from disk, inject content scripts
  on navigation (re-injected after reload — the `Reload` message
  clears the injection guard, since Servo 0.8.0 `WebView::reload()`
  never notifies the delegate); a sample banner extension runs on
  example.org; a bad manifest is skipped without crashing;
  `frost://extensions` lists loaded and failed extensions. The
  `frost.*` bridge exists host-side (`FrostApi` trait: in-memory
  storage, tab query/create against a single-tab stand-in); page JS
  does not see a `frost.*` object yet — that is a later round.
- Menu height fixed: all four entries fully visible, no clipping.
- F-005 (`WebView::refresh_viewport()`) landed in the fork but was
  PROVED INSUFFICIENT as the full-repaint repair — the Round 2 resize
  jiggle is restored and stays. The real fix is fork patch F-007
  (Round 4: the partial paint was never a reflow problem — it was
  partial present; F-007 restores full present and the jiggle retires).
- Known limits (see README Round 3): tiny instant-loading local HTML
  pages may never paint their body (keytest page is the standing
  repro); Ctrl+A select-all does not work in page text inputs under
  the harness; transient blank frames can appear mid-typing and
  recover on their own.
- Gate: sample extension runs on a real page (banner visible on
  example.org, survives reload); settings change visibly + persist
  across a restart; menu unclipped; Markdown renders; file browser
  end to end; print/exit pages load and exit quits the app.

## Round 4 — finish the Round 3 leftovers (DONE 2026-11-14)

No new features — this round finishes what Round 3 left open:

- **F-007 — the partial-paint root cause, found and fixed.** The
  keytest page never painted its body because the fork's damage-rects
  patch set WebRender `max_partial_present_rects: 1` (partial present)
  while Frost's engine reads the GL back buffer wholesale — the full
  scene rendered once and was lost. The fork now uses full present
  (`0`, the upstream default; commit `f4886bf57`). The Round 2 resize
  jiggle is RETIRED: keytest paints fully on load AND reload with no
  jiggle. F-005 stays as a harmless viewport re-send.
- **F-006 — fork-level reload fix** (commit `fba674cc3`): `reload()`
  now notifies the delegate via `set_load_status`, so the engine
  reports `LoadStarted` on reload and the sample banner re-runs. The
  app-side Reload workaround is retired.
- **F-003 — printing for real, as far as Servo 0.8.0 allows.** Servo
  has NO print pipeline (no `WebView::print`, no PDF path), so
  paginated printing and print-to-PDF are genuinely unbuildable. What
  shipped: a real print preview — the engine captures the current
  page as a PNG (`request_screenshot_png`, via `take_screenshot`)
  before `frost://print` opens, the server serves it as
  `/print/preview.png`, and "Save as PNG" writes it to ~/Downloads
  for printing from any viewer. The page says plainly it is a
  viewport screenshot, not paginated print layout.
- **Ctrl+A select-all fixed (app-side).** The key path preferred the
  key event's produced text, and iced/winit reports Ctrl+A's text as
  `"\x01"` — Servo got `Key::Character("\x01")` and the shortcut
  matcher (which compares against `'A'`) never matched. Control/Meta
  chords now use the logical key and ignore produced text
  (`keymap.rs`). Gate: type "hello", Ctrl+A, type "X" → `val="X"`.
- **F-008 — Stop aborts a pending navigation** (commit `f9462dbb7`,
  stretch): `handle_stop_msg` also drains the webview's pending
  session-history changes and closes their pipelines, mirroring the
  script-side `handle_abort_load_url_msg`. No new message type.
- Gate: keytest paints on load + reload (no jiggle); banner survives
  reload; print preview renders and saves; Ctrl+A replaces; Stop
  aborts a hanging load; settings, file browser, menu, back/forward
  still work.

## Round 5 — Inspector v1 (DONE 2026-11-14)

- **Script-injection path, NOT fork patch F-002.** The fork's
  `WebView::evaluate_javascript` already returns values, so a DOM
  walker script (`crates/frost-inspector/src/walk.js`) serializes the
  live DOM to JSON and the app parses it. F-002 is closed as not
  needed; a devtools server is a multi-round job, not v1.
- `frost://inspector`: snapshot model (capture the DOM first, then
  open the inspector page — Frost has one webview, so navigating to
  the inspector would replace the inspected page). Expandable tree,
  click a node for attributes/text/computed style/box, "Back to the
  page" + "Re-capture" links, snapshot URL + time + node count in the
  header, honest "No page inspected yet" state. Menu entry + F12.
  Click-to-inspect from the live page is v2 (needs a docked panel),
  not faked in v1.
- Wart fixes in the same round: file-browser history (only the
  `frost://open` form is intercepted now), print save creates
  `~/Downloads` first and is a real `/print/save` server route
  (no more "Not found" flash after Save).
- Gate: inspect example.org, see the real current DOM (`html > head
  (meta, link, meta, title, style) > body (div#frost-sample-banner,
  style, svg, p ×5)`, 35 nodes — the live page has no `<h1>` anymore),
  click a node for its details; folder clicks then Back/Forward move
  correctly; print Save works with `~/Downloads` deleted first and
  never flashes "Not found"; settings, banner, back/forward, menu
  still work. Tests 49/49 green (frost 29, engine 4, extensions 8,
  inspector 8).

## Round 6 — flashing fix (DONE 2026-11-15)

- **Root cause: iced's 2 MiB synchronous-upload line, not the engine.**
  Every new frame minted a fresh texture handle; a 3.5 MB frame went
  to a worker thread and drew nothing for one frame — the flash Joel
  saw on load and scroll. The engine was exonerated by a headless
  probe (`frameprobe.rs`, kept as a diagnostic example): all 24
  scroll renders were full frames.
- **Fix (app-side, no fork patch):** `Frame::split_strips` tiles each
  frame into horizontal strips under 1.9 MB; the app draws one image
  per strip, so every strip takes the synchronous path and draws the
  same frame it arrives. F-007 (full present) stays.
- Gate: build green (38m 33s); tests **52/52** (frost 29, engine 7
  incl. 3 new strip tests, extensions 8, inspector 8); example.org
  paints fully on load; tall-page scroll fully painted at every
  sampled step (incl. a rapid-fire burst mid-scroll); reload repaints
  fully; settings persist across relaunch, banner, back/forward,
  menu, Inspector (menu + F12), file browser, print Save all still
  work. The sandbox cannot catch a one-frame flash directly
  (SwiftShader paints ~1 s/frame) — Joel's box is the real verdict.

## Round 7 — native windows, loopback server deleted (DONE 2026-11-15)

- **Joel's direction: menu pages pop up in their own libcosmic
  windows; Frost runs no internal web server.** `server.rs` and all
  `frost://` handling are deleted. Settings, Print, Open file,
  Inspector, and Exit are native windows
  (`crates/frost/src/app/windows.rs`); Extensions moved into the
  Settings window; Markdown renders natively in the Open window
  (pulldown-cmark → iced widgets). Typing a `frost://` URL fails
  plainly with an honest status message. No fork patch.
- **Multi-window mechanics:** the `multi-window` libcosmic feature is
  enabled; popups open with `window::open(Settings)` +
  `exit_on_close_request: false`, `set_window_title` batched with the
  open task; `event::listen_with` routes Opened / Closed /
  CloseRequested per window id. Single-instance per kind (re-open
  focuses). The `app` module moved to `src/app.rs` so `windows.rs`
  can live under it (`src/app/windows.rs`).
- Gate: build green (serialized); tests 45 (baseline 52/52 +
  new file-browser + windows tests); one firsthand-read screenshot
  per new window; loopback-gone gate (no server listening, `frost://`
  fails plainly); regressions (settings persist, banner,
  back/forward, F12 Inspector, print Save, strip-upload scrolling).

## Round 8 — Firefox-style hamburger dropdown (DONE 2026-11-15)

- **Joel's direction (from a Firefox application-menu screenshot):
  the hamburger menu drops down like Firefox's** — a compact 300 px
  panel anchored top-right under ☰, not the full-width bar of
  Rounds 2–7. Same five items, same native windows; separators,
  F12 next to Inspector, hover highlight, closes on pick / Escape /
  click-outside / ☰ again. No fork patch.
- Gate: build green (serialized); tests 49/49 (frost bin 26, engine
  7, extensions 8, inspector 8); firsthand-read screenshots per
  behavior; regressions (settings persist, banner, back/forward,
  Print Save, F12 Inspector, strip-scroll, frost:// fails plainly,
  no listener).

## Round 9 — command palette (DONE 2026-11-15)

- **VS Code-style palette, Ctrl+K or the ☰ menu's "Command
  palette…" entry (Ctrl+K shown right-aligned).** Centered overlay
  (560 px, input on top, filtered list below) reusing the Round 8
  overlay pattern (Stack + dimmed click-catcher + panel); fuzzy
  subsequence match, case-insensitive; arrows move the highlight
  (wraps), Enter runs, Escape closes, click outside closes, row
  click runs, Ctrl+K toggles. Only real, working actions, with
  unavailable ones hidden (Go back/Go forward need history, Stop
  loading needs a load in flight). Follows Dark/Light. Opening the
  palette closes the menu and vice versa. No fork patch.
- **Gate found and fixed a real bug:** libcosmic's text_input
  captures every KeyPressed while focused, so a Ctrl+K with the
  palette input focused never reached the toggle — fixed in the
  Captured-event handler. Gate: build green (serialized); tests
  53/53 (frost bin 30, engine 7, extensions 8, inspector 8);
  firsthand-read screenshots per behavior (open, filter "set",
  arrow, Enter runs Inspector, Escape, click-outside, toggle,
  menu entry, palette-closes-menu, row click, Focus address bar);
  regressions (settings save + persist, banner on .org / absent
  on .com, back/forward, Print Save, F12 Inspector, strip-scroll,
  frost:// fails plainly, no listener, menu opens/closes).

## Later rounds (Joel's order, settled 2026-11-15 — easiest first)

- Round 10 (done 2026-11-16, ahead of the order at Joel's pick):
  WM titlebar with GNOME fallback — the window manager draws the
  titlebar by default; Frost's own bar survives only as the GNOME
  fallback.
- Round 11 (done 2026-11-16, ahead of the order at Joel's pick):
  merged reload/stop button (⟳ idle, ✕ loading, flips back on
  finish or stop) + round toolbar icons (square 32 px bounds).
- Round 12 (done 2026-11-16, at Joel's pick): toolbar glyphs
  bigger + centered (20 px text in a centered container, ❮/❯
  chevrons) — fixed his 0.11.0 icon verdict.
- Round 13 (done 2026-11-16, step 2 in the order): Snowplow v1 —
  embedder-side tracker/ad subresource blocking (62-domain
  blocklist, suffix matcher), Settings toggle default on,
  per-page blocked count in the status line. No fork patch.
- Round 14 (done 2026-11-16, step 3 in the order): Split view
  v1 — two webviews side by side, fixed 50/50, each with its
  own URL, history, and Snowplow count; active pane wears a
  2 px border; menu + Ctrl+K toggle with a dynamic label.
  No fork patch (every `EngineEvent` now carries a
  `WebViewId` so the UI can route events to panes). No
  draggable divider yet — that is the v1 limit, not a bug.
- Round 15 (done 2026-11-16, step 4 in the order): Deep Freeze
  v1 — freeze the active pane's page into a single
  self-contained HTML archive (live DOM, absolute
  subresource URLs, `<base>` tag) under
  `$XDG_DATA_HOME/frost/archives/` with an `index.json`;
  "Archived pages…" native window lists archives with
  Open/Delete; menu (9 items) + Ctrl+K entries. No fork
  patch. Honest v1 limits: heavy JS apps may not replay
  perfectly (scripts re-run against the static DOM), no
  incremental updates — re-freeze to refresh.
- Round 16 (done 2026-11-17, JUMPED the order ahead of
  Frostbite on Joel's "lightweight next" 2026-11-16):
  hardware rendering. UI side: `WGPU_BACKEND=gl` baked in
  as the default in `main.rs`, but only when a wgpu probe
  finds a GL adapter first (a retry was considered and
  rejected — on libcosmic's multi-window path the app's
  init runs before the compositor, so a second `app::run()`
  would panic in the engine's one-shot `setup_logging()`).
  Engine side: fork patch F-009 adds
  `HardwareRenderingContext` (same offscreen surfman shape
  as the software context, `Connection::create_adapter()`
  instead of `create_software_adapter()`), tried first with
  a software fallback and a `FROST_RENDERER=software`
  escape hatch. Honest limit: the sandbox has no GPU, so
  no CPU win was measured there — Joel's box is the
  verdict. Packaging note: the GLES backend needs EGL +
  GLESv2 and `libGLESv2.so` (`libgles2-mesa`) is NOT in
  the 0.11.0/0.14.0 .deb Depends — the next packaging
  must add it.
- Round 17 (done 2026-11-17, JUMPED the order ahead of
  Frostbite on Joel's "YouTube next" 2026-11-16):
  YouTube playback, staged. Stage 1 DONE: youtube.com
  loads — fork patch F-010 raises the HTTP/2 client's
  max header list size to 256 KiB (h2's 16 KiB default
  made the local stack reset YouTube's oversize header
  block with PROTOCOL_ERROR — "client error
  (SendRequest)"). The media stack is enabled
  (`media-gstreamer` feature) and PROVEN working: a
  local H.264 + AAC MP4 plays with frames advancing
  (software decode `avdec_h264` in the sandbox).
  Stages 2–4 BLOCKED: Servo 0.8.0 has NO Media Source
  Extensions (`MediaSource: undefined`, `canPlayType`
  `probably` for all codecs, zero `videoplayback`
  requests, embed player Error 153) — YouTube's player
  requires MSE and refuses before fetching. Implementing
  MSE is a multi-round engine project, not attempted
  here. No .deb, no push.
- Round 18 (done 2026-11-17, MSE project — Joel's pick
  2026-11-16, ahead of Frostbite): MSE foundation,
  fork patch F-011. `MediaSource` + `SourceBuffer` +
  `SourceBufferList` WebIDL bindings with the full
  attribute/state-machine surface (`isTypeSupported`,
  `addSourceBuffer`/`removeSourceBuffer`,
  `endOfStream`, `setLiveSeekableRange`, `appendBuffer`
  with the `updatestart` → `update` → `updateend`
  sequence, `abort`/`remove`/`changeType`),
  `URL.createObjectURL(mediaSource)`, and attach to a
  `<video>` element (`closed` → `open` + `sourceopen`,
  detach → `closed` + `sourceclose`). Honest limit:
  `appendBuffer` QUEUES bytes — `buffered` stays
  empty, nothing is decoded, YouTube still does not
  play. The decode pipeline is Round 19.
- Round 19 (done 2026-11-17, MSE project): MSE
  slice 2, fork patch F-012. `appendBuffer` →
  media thread: new fMP4 parser (`mp4.rs`, 7
  fork-only unit tests), `SourceBuffer` drains queued
  bytes into the player via the normal fetch path,
  `buffered` [0.00–10.00] computed script-side,
  `loadedmetadata` + duration 10 s + `networkState`
  Idle + `activeSourceBuffers` length 1 from the
  media thread. Stretch result, gated live: the
  local segments decode and PLAY (`playing` fires,
  `currentTime` advances, frames render) — so
  Round 20 is re-scoped from "build an appsrc
  pipeline" to "feed YouTube's DASH segments
  through this one". Honest limits: YouTube still
  does not play, seeking not supported, duration is
  the demuxer's running estimate. Full suite 87/87;
  R19 probe ALL PASS; R18 probe 29/29; all
  regressions pass. No .deb, no push.
- Round 20 (done 2026-11-17, MSE project): MSE
  slice 3, fork patch F-013 — the remuxer
  (`mux.rs`, 7 fork-only unit tests). A real YouTube
  player appends one init segment per track, so the
  single `ServoSrc` byte stream had TWO `moov`
  boxes and two tracks both numbered 1, and
  `qtdemux` could not negotiate it (`not-negotiated
  (-4)`). The remuxer merges the buffers' streams
  before pushing: one buffer passes through
  byte-identical (Round 19 behavior); two or more
  buffers hold inits + media until every buffer has
  its init, then emit ONE combined init (fresh
  `moov`, reassigned track IDs, one `trex` per
  track) followed by media in presentation-time
  order with `tfhd` track IDs rewritten and `mfhd`
  sequence numbers renumbered. Gated live: real
  YouTube DASH segments ("Me at the zoo", itag 134
  + 140) play end to end through the MSE pipeline —
  video `buffered` [0.00–18.93], audio
  [0.00–19.06], `playing` fires, `currentTime`
  advances, decoded frame renders. Honest limits:
  the real watch page still does NOT play — its
  player requests progressive itag 18, YouTube
  answers HTTP 403 in the sandbox (bot-checked IP)
  and the player errors out; SABR/DASH POSTs return
  200 but playback never starts, and whether the
  player appends them to MSE is unproven (no MSE
  logging at info level). The audio in the R20 test
  segments is re-containerized (ffmpeg stream-copy
  of the downloaded itag 140 audio — AAC sample
  data byte-identical, box structure regenerated);
  the video is untouched real DASH. Seeking still
  not supported. Full suite 87/87; R20 two-buffer
  probe ALL PASS, probe3 ALL PASS, R18 probe 29/29,
  R19 probe ALL PASS; all regressions pass. No
  .deb, no push.
- Round 21 (done 2026-11-17, MSE project): YouTube playback
  end-to-end attempt — drive the SABR/MSE path on the real
  watch page. F-014 adds MSE append + player-event logging at
  info level (fork patch, fourteen patches total), and the
  embedder gains a `FROST_BLOCK_URL_SUBSTRING` diagnostic
  block (proven live by a local probe: itag=18 fetch fails
  fast, itag=134 control returns 200). What the gates proved:
  the player DOES initialize and append on the real watch page
  (59 `MSE:` lines, 23 appendBuffers, SABR 5× 200) — but it
  chooses WebM audio (Opus) + AV1/VP9 video, and the remuxer
  is fMP4-only, so every WebM chunk is silently dropped, the
  combined init never emits, `pushed=0` in all 15 drains, and
  the player starves and stalls at 0:00. The itag-18
  hypothesis is NOT supported by the run that reached the
  player (zero itag-18 requests); the bot check is flaky
  between runs (one run showed "Sign in to confirm you're not
  a bot" with no player at all). Label correction: `accepted:
  false` means DROPPED, never held. Honest limits: the watch
  page still does NOT play; seeking not supported. Full suite
  88/88; R18 probe 29/29, R19 probe ALL PASS, R20 probe ALL
  PASS; all regressions pass. No .deb, no push.
- Round 22 (done 2026-11-17, MSE project): steer the player
  to fMP4 — Joel chose this himself over parsing WebM/EBML
  into fMP4. F-015 makes `MediaSource.isTypeSupported`
  return false for `webm` and `av01` (pure predicate
  `mse_steer_blocks`, case-insensitive; everything else still
  delegates to `can_play_type()`), so the player falls back
  to itag 134 (avc1/fMP4) + itag 140 (mp4a/fMP4), PROVEN to
  play through the remuxer (R20). What the gates proved:
  on the real watch page the player picks fMP4 only (avc1
  + mp4a appends, drains all `pushed=1, dropped=0`,
  buffered to 18.93 / 19.06, zero webm/opus/av01 in either
  run's log), the decoded frame renders and currentTime
  advances — but the video freezes a few seconds in, both
  runs, with a full buffer and no error (new post-steer
  stall, not the R21 starvation; audio-sink stall refuted
  by an unmuted probe advancing past 7.5 s in real time).
  Plain `<video src>` WebM is unaffected (canPlayType
  "probably", plays 0.00 → 3.01). Scope check: plain
  `canPlayType` calls `can_play_type()` directly, so the
  steer is MediaSource-only. Honest limits: the watch page
  still does not play through; seeking not supported. Full
  suite 88/88; R18 probe 29/29, R19 probe ALL PASS, R20
  probe ALL PASS; all regressions pass. No .deb, no push.
- Round 23 (next, MSE project): diagnose the post-steer
  stall — why a fully-buffered MSE element stops advancing
  a few seconds in on the real watch page with no error.
  Then: Frostbite tab hibernation (needs a tab system
  first), Snowdrift workspaces (needs tabs + sessions +
  persistence — biggest). Inspector v2 (docked panel,
  click-to-inspect, live updates, console) stays
  unqueued. (Print-to-PDF stays out until Servo gains a real
  print pipeline — F-003's preview + PNG save is the honest
  scope.)

## Later rounds (in rough order)

- Frostbite tab hibernation, Snowplow network-layer blocking, Deep Freeze
  offline archive, command palette, Snowdrift workspaces, split-view,
  session restore, PDF viewer, spellcheck (F-004).

## Unique features (the Frost difference)

These are the ideas that make Frost worth building instead of using
Firefox. Each gets its own round; none is started before Round 2 lands.

### Frostbite — tab hibernation with live thumbnails

Idle tabs freeze: their process is suspended and their last frame is kept
as a live thumbnail in the tab strip. A frozen tab uses ~zero CPU and
almost no memory, but still *looks* alive — hover the tab and you see the
page as it was. Click to thaw (fast: the webview is still there, just
paused). Auto-freeze after N minutes idle, never the active or
audio-playing tab. This is the feature Servo's multiprocess design makes
cheap: freezing a tab is suspending its pipeline, not serializing it.

### Split-view — two pages in one tab

**v1 DONE (Round 14, 2026-11-16).** One tab, two webviews
side by side at a fixed 50/50 split. Each half is a full
page: its own URL, its own back/forward, its own Snowplow
count. The active pane wears a 2 px border (the inactive
pane's border is transparent at the same width, so
switching never shifts layout); click a pane to activate
it; the menu and the Ctrl+K palette toggle the split with
a dynamic label ("Split view" / "Close split view");
closing keeps the active pane. Built for comparing pages,
reading docs beside code, watching a video while reading.
The engine already supports many webviews per process —
split-view is mostly UI. v2 (unqueued): a draggable
divider, and stacked (top/bottom) splits.

### Snowplow — tracker/ad blocking at the network layer

**v1 DONE (Round 13, 2026-11-16).** Blocking that happens
before the request leaves the machine — at the embedder
delegate (`WebViewDelegate::load_web_resource`), not a
content script racing the page, and not a fork patch in
the engine's network stack. A 62-domain blocklist with a
suffix matcher; the `WebRequest` extension permission is
the same hook, so blocklists and extensions share one
path. The UI shows a real per-page count of blocked
requests in the status line. v2 (unqueued): main-frame
blocking, per-site allow/deny, list management in the UI.

### Deep Freeze — offline page archive

**v1 DONE (Round 15, 2026-11-16).** "Freeze this page…"
in the menu and the Ctrl+K palette saves the ACTIVE
pane's page as one self-contained HTML file: the live
DOM cloned via the Inspector's script-injection path,
relative subresource URLs rewritten to absolute, a
`<base href="original-url">` in the head, saved to
`$XDG_DATA_HOME/frost/archives/` with an `index.json`
(title, URL, date frozen, size). "Archived pages…"
opens a native window listing archives with Open (loads
the archive `file://` in the active pane) and Delete
(removes file + entry). Archives are read-only
snapshots — opening one never re-fetches the live page;
Snowplow-blocked resources simply are not in the
archive. Honest v1 limits: plain pages archive cleanly,
heavy JS apps may not replay perfectly (scripts in the
snapshot run against a static DOM — example.org's own
`s.js` re-runs on open and re-orders its paragraphs),
and there are no incremental updates (re-freeze to
refresh). v2 (unqueued): inlining subresources so an
archive needs no network at all, and per-archive
refresh.

### Command palette

`Ctrl+K` opens a fuzzy palette: switch tabs, open URLs, run settings,
toggle features, search history. Every action in the browser is
reachable by typing, not just by clicking.

### Snowdrift — workspaces

Named workspaces (Work, Research, Personal…) each with their own tabs,
pinned sites, and extensions. Switch workspaces and the whole tab strip
changes. Frostbite freezes the workspace you leave.

## Known Servo gaps (planned for, not feared)

| Gap | Frost answer |
|---|---|
| No WebExtensions API | We build our own host (`frost-extensions`); manifest is close to WebExtensions so authors can port with small edits. |
| No HTML inspector | We built our own (`frost-inspector` walker script + `frost://inspector` snapshot page, Round 5 — no fork patch; F-002 closed as not needed). Click-to-inspect is v2. |
| Limited print support | Fork patch F-003 or paginated-image export from the render path. |
| No PDF viewer | Later round: render PDFs via the engine (pdf.js-style, Servo-rendered) or a native viewer page. |
| No spellcheck | Fork patch F-004 or a pure-Rust spellcheck in the UI layer. |
| No `WebView::stop` | Fork patch F-001 — LANDED in Round 2 (real `stop()`; the Stop button is enabled while loading). F-008 LANDED in Round 4: Stop now also aborts a navigation stuck at the constellation (pending-change abort folded into `handle_stop_msg`). |
| No WebGL in the sandbox feature set | Deliberate Round 1 cut (no libclang for mozangle bindgen, no gstreamer). Revisit when the build machine has them; Joel's box may already. |
| No media (audio/video) in the sandbox feature set | Same cut (no gstreamer). Same revisit. |

## Non-goals

- Firefox extension compatibility (no WebExtensions runtime exists in
  Servo; Frost extensions are Frost-native by design).
- A second engine. Servo is the renderer, full stop.
- Mobile. Desktop Linux first (Debian 13 / KDE is the reference box).
