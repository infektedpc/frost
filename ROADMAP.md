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
  fallback. Then: Snowplow network-layer blocking, split view,
  Deep Freeze offline archive, Frostbite tab hibernation (needs a
  tab system first), Snowdrift workspaces (needs tabs + sessions +
  persistence — biggest). Inspector v2 (docked panel,
  click-to-inspect, live updates, console) stays unqueued. (Print-
  to-PDF stays out until Servo gains a real print pipeline —
  F-003's preview + PNG save is the honest scope.)

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

One tab, two webviews side by side (or stacked), with a draggable
divider. Each half is a full page: its own URL, its own back/forward.
Built for comparing pages, reading docs beside code, watching a video
while reading. The engine already supports many webviews per process —
split-view is mostly UI.

### Snowplow — tracker/ad blocking at the network layer

Blocking that happens before the request leaves the machine, in the
engine's network stack — not a content script racing the page. Rules are
compiled to a fast matcher; the `WebRequest` extension permission is the
same hook, so blocklists and extensions share one path. The UI shows a
per-page count of blocked requests ("plowed").

### Deep Freeze — offline page archive

Save any page as a frozen archive: the full DOM + resources, stored
locally, re-openable without the network. Not a screenshot and not a
single-file HTML dump — a real archive Frost can re-render. For research,
receipts, and pages that disappear.

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
