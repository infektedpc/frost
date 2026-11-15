# Frost architecture

## Workspace

| Crate | Role | Depends on |
|---|---|---|
| `frost` | libcosmic app: window, toolbar, address bar, hamburger menu, titlebar (Round 2); tab strip later | `frost-engine`, `libcosmic` (optional, git) |
| `frost-engine` | Servo embedding: engine, webview per tab, event pump, frame readback | fork `servo/components/servo` (path dep) |
| `frost-extensions` | Extension host: manifests, content scripts, `frost.*` bridge trait, stub host | `serde`, `serde_json`, `url` |
| `frost-inspector` | Inspector: DOM snapshot types, walker script (`walk.js`), `snapshot_from_json`, `DOM_WALK_SCRIPT` | `serde`, `serde_json` |

Rules:

- `frost-engine` never imports the UI. It is headless by design — that is
  what makes the smoke test possible and keeps the engine testable without
  a window.
- `frost-extensions` and `frost-inspector` never import Servo. They define
  traits the engine/UI implement later, so the browser is never rewritten
  around them.
- The `frost` binary is the only crate that knows about libcosmic.

## Engine / UI split

```
┌────────────────────────────────────────────────────────────┐
│ frost (libcosmic window)                                   │
│  toolbar · address bar · menu · tab strip · content area   │
│  owns: Engine (one per process), FrostWebView (one/tab)    │
└───────────────┬────────────────────────────────────────────┘
                │  load/go_back/go_forward/reload/resize
                │  send_input(FrostInputEvent)
                │  render() -> Frame { rgba }
                │  EngineEvent channel (title/url/load/frame/crash)
┌───────────────┴────────────────────────────────────────────┐
│ frost-engine                                               │
│  ServoBuilder → Servo → WebView + SoftwareRenderingContext │
│  spin_event_loop() driven by the UI loop (Round 2)         │
└────────────────────────────────────────────────────────────┘
```

One engine per process; one `FrostWebView` per tab. Each webview has its
own `SoftwareRenderingContext` (offscreen, software raster). The UI pumps
`Engine::spin()` on its loop (every frame while loading/animating, on
demand otherwise) and drains the `EngineEvent` channel for title, URL,
and load state.

## How offscreen rendering feeds the libcosmic window (Round 2)

1. The UI creates a `FrostWebView` at the content area's size.
2. Each UI frame (or on `EngineEvent::FrameReady`), the UI calls
   `webview.render()` → `SoftwareRenderingContext::read_to_image` →
   RGBA pixels — but only once no load is in flight (`awaiting_load`
   is clear) and no forced repaint is running. Rendering during a load
   paints a partial frame (see below).
3. The pixels are uploaded into an iced texture (`cosmic::iced::widget::image`
   backed by a `Handle`) and drawn in the content area.
4. `resize()` follows the content area; per the servo-de Round 8 finding,
   `WebView::resize` alone is enough in 0.8.0 — the rendering context
   follows the webview.
5. Input: libcosmic/winit pointer and key events map to
   `FrostInputEvent` and go in through `webview.send_input`. Keyboard
   mapping is a Round 2 concern; note the servo-de `+8` xkb fix was for
   Wayland seat input (evdev keycodes) and does NOT apply here — Frost
   keys come from winit/libcosmic. Round 3 prefers the key event's
   produced text for page text input (typing works, proven by the
   keytest and settings fields), but modifier chords did not reach the
   page the same way: Ctrl+A select-all produced no selection in a
   page input under the harness, so typed text appended instead of
   replacing. Round 4 found it in the app: iced/winit reports the
   produced text for Ctrl+A as `"\x01"`, and `map_key_with_text`
   preferred it, so Servo got `Key::Character("\x01")` and the
   `ShortcutMatcher` (which compares against `'A'`) never matched —
   the engine side is sound (`editing_action_from_keyboard_event`
   maps Ctrl+A to `EditingAction::SelectAll` on keydown). The fix:
   `map_key_with_text` takes the modifiers and returns the logical
   key when CONTROL or META/LOGO is held — shortcut chords ignore
   produced text. Gate (type "hello", Ctrl+A, type "X" → `val="X"`)
   passes. Alt alone is left alone (AltGr text entry on non-US
   layouts; Ctrl+Alt chords already include Control).

### The partial first paint and the resize jiggle (Round 2; root cause found and fixed in Round 4 as F-007)

A load's first paint used to cover only a ~1024×512 region of a
1280×684 frame, and no later paint repaired it — not a render after
`LoadFinished`, not a reload. What repaired it was a real resize to a
different size (maximizing, restoring, even a 1 px window resize all
painted fully, and the repair persisted). So after every `LoadFinished`
(or `LoadStopped`) the app forced a full repaint with a two-phase
resize jiggle: resize 1 px wider, wait ~0.25 s, resize back to the
content size, wait ~0.25 s, then upload. The waits were timed, not
`FrameReady`-driven — advancing on a `FrameReady` resized straight
back on a stale (pre-resize) event, coalescing the two resizes into a
net no-op. The intermediate size was never uploaded, so the user never
saw it. History traversals (back/forward) restore from cache and
never fire `LoadFinished` — the 15 s load-timeout safety net clears
the stuck loading state instead.

Round 3 tried to retire the jiggle with fork patch F-005
(`WebView::refresh_viewport`: re-send the current viewport without
resizing). The finish pass PROVED it insufficient: 2 of 3 first loads
still painted partial and stayed partial for minutes.

**Round 4 found the real mechanism (F-007) — and the Round 2/3
theory was wrong.** The old theory said WebRender repaints fully
every render (`buffer_age` 0), so a partial frame must mean a partial
display list. Instrumented paint-thread logging (`frost-diag`
`debug!` lines in the fork, 2026-11-14 — the display-list and
root-wrapper lines are permanent; the per-frame dirty-rect logging
was removed after the diagnosis) proved the opposite:

- The pipeline's display lists arrive complete and on time: for the
  keytest page, epoch 1 carried 2046 bytes at viewport 1280×684
  (`first_reflow=true`), later epochs 34898→36474 bytes for a real
  page; a forced in-page reflow grew the list to 2356 bytes. Layout
  was also proven correct independently (the page's own script
  reported `h1=1240x43.58@20,20`, viewport 1280×684).
- The root wrapper's iframe rect is correct (1280×684).
- WebRender renders the full frame exactly once after a new display
  list, then reports only the dirty regions — e.g. a 70×18 px caret
  rect `(7,86)-(77,104)` re-rendered every second, and a
  `(0,0)-(1280,512)` rect after the forced reflow.

The cause: the fork's damage-rects patch (WayServo Round 17) set
WebRender's `max_partial_present_rects` to 1, enabling partial
present — WebRender then repaints only dirty regions. That contract
requires the embedder to keep its own full-frame backing buffers
and composite the delta onto them (what WayServo does). Frost does
not: its engine reads the GL back buffer wholesale every tick and
uploads it as the frame, and `Painter::render` clears that buffer
first. So the full scene is rendered once and lost; every later
frame is white except the dirty regions. Slow pages mask this
because their display lists keep arriving (network, JS) and keep
re-rendering large regions after the jiggle; instant pages expose
it, because their single full-frame render is swallowed by the
jiggle's `FrameReady` suppression and never reaches the screen.

**F-007 (Round 4, gated 2026-11-14):** the fork sets
`max_partial_present_rects: 0` — full present, the upstream Servo
default ("a full screen present occurs at the end of the frame").
WebRender repaints the whole scene every frame, which is what Frost's
read path needs. The jiggle is RETIRED: `start_repaint_jiggle`,
`advance_repaint_jiggle`, and the `RepaintPhase` machinery are deleted
from the app, and the Tick renders on `frame_dirty &&
!awaiting_load`. Gate: the keytest page paints fully on load AND
reload with no jiggle (screenshots `r4-11-keytest-fixed.jpg`,
`r4-12-keytest-reload.jpg`). F-005 stays as a harmless viewport
re-send. Tradeoff, stated plainly: full present is more paint work
per frame under SwiftShader/llvmpipe than partial present — but
partial present never actually saved Frost anything, since the engine
reads the full framebuffer regardless.

Standing repro, now FIXED (finish pass, 2026-11-13; F-007 is the fix,
gated in Round 4): a tiny instant-loading local HTML page
(`file://…/keytest.html`, 529 B, a script focuses an input on load)
never painted its body — not on load, not on reload. The page itself
was alive (title known, input focusable, typed text lands in it), and
interaction painted only the tiles it touches (typed `abc` appeared
alone on a blank page). It painted fully exactly once, transiently,
after a click. Real pages (network, loopback, file listings,
Markdown) painted fully. The jiggle repaired loads whose display list
was partial; this page's display list was complete but its first paint
never reached the back buffer — the partial-present mechanism above
was the cause, and full present is the fix.

Later rounds may switch the context to a GPU path (the dmabuf fork patch
exists for exactly this), but the `render() -> Frame` contract does not
change — the UI never sees how the pixels were produced.

### The flashing on load and scroll (Round 6, 2026-11-14; app-side, no fork patch)

Joel's verdict on the 0.4.0 .deb (2026-11-14 ~07:43): "the browser
works but its drawing isnt entirely smooth theres alot of flashing
going on." He added (2026-11-14 ~18:41) that the flashing is mainly
on load and scroll, not on a still page. Round 4's F-007 (full
present) was the prime suspect — it repaints the whole scene every
frame — but the investigation proved the engine innocent:

- A headless probe (`crates/frost-engine/examples/frameprobe.rs`)
  rendered the engine's frames during a 12-step scroll down and back
  up a 4000 px test page: all 24 renders were full frames
  (223–923 distinct colors, center pixel tracking the bands), never
  a solid or blank frame. `render()` returns complete pixels during
  scroll; the engine does not produce blank frames.
- The fault is in the APP's upload path, in iced 0.14 (vendored in
  the libcosmic checkout Frost builds against). `Handle::from_rgba`
  mints a fresh unique id per call (`iced/core/src/image.rs`), so
  the app's one-new-handle-per-frame meant a brand-new texture every
  frame. iced's wgpu image cache (`upload_raster`,
  `iced/wgpu/src/image/cache.rs`) uploads a raster image
  synchronously only when it is under `MAX_SYNC_SIZE` (2 MiB);
  anything larger is handed to a worker thread and `upload_raster`
  returns `None` — and the image pipeline (`State::prepare`,
  `iced/wgpu/src/image/mod.rs`) adds no instances for it, so the
  widget draws NOTHING for that frame: the content area shows the
  chrome background. The worker requests a redraw when the upload
  lands, so the frame appears one draw later — but by then the app
  has usually replaced it with a newer handle. Frost's full frame
  (1280x684x4 = 3.5 MB) is over the line, so every frame during load
  and scroll cost one blank (dark) draw: the page vanished while
  scrolling/loading and popped back when activity paused. A still
  page reuses the same handle — already uploaded — so it never
  flashes. That matches Joel's symptom exactly, including "not on a
  still page".
- The atlas-full theory was checked and ruled out: `Atlas::allocate`
  grows a new layer instead of failing for Frost's size, so no
  `OutOfMemory` blank comes from there.

**Round 6 fix (app-side, no fork patch): split the frame into
horizontal strips, each under iced's 2 MiB synchronous-upload line.**
`Frame::split_strips` (`frost-engine`, `MAX_SYNC_UPLOAD_BYTES` =
1.9 MB with margin) tiles the frame exactly — same width, heights
summing to the frame height, rows in order — and the app uploads
one `Handle` per strip, drawn as a column of images at 1:1. Every
strip takes the synchronous path and is drawn the same frame it
arrives: no worker hop, no one-frame delay, no blank draw. The
strip count adapts to the viewport (1280x684 → 2 strips; a
1920x984 content area → 4). Unit tests prove the tiling and the
under-the-limit invariant for the sizes Frost renders at. No fork
change, so no F-number; F-007 (full present) stays — reverting it
would bring back the partial-paint loss.
**Gates (sandbox, 2026-11-15, Xvfb :97 + software GL):** build green
(38m 33s); tests **52/52** (frost bin 29, engine 7 incl. the 3 new
strip tests, extensions 8, inspector 8); example.org paints fully on
load; a 10-band tall page scrolls fully painted at every sampled
step (PageDown ×2, Space, rapid-fire burst mid-scroll — no blank
frame); reload repaints fully. Regression: settings persist across
relaunch, banner on example.org (absent on example.com), Back/Forward
on pages and across file-browser folders, all five menu entries,
Inspector via menu + F12, file-browser folder/file clicks, print Save
to ~/Downloads. Screenshots `r6-09`–`r6-39` in
`~/workspace/harness-tmp/frost/`.

### The loading flag and Stop (Round 4, F-008 app-side)

The Stop button (✕) is live only while `self.loading` is true, and
`loading` used to be set only by the `LoadStarted` engine event.
Servo reports `LoadStatus::Started` from
`Document::set_document_readiness_to_loading_for_initialization` —
which runs when a new Document is initialized, i.e. when the first
response bytes arrive. A server that never responds at all therefore
never fires `LoadStarted`: the app never learns a load is in flight,
the Stop button stays dead, and the hang has no escape hatch. (The
15 s safety net eventually clears `awaiting_load`, but it does not
stop the load.) Since Round 4 the app sets `loading = true` at the
moment a navigation is requested — address-bar submit, Back,
Forward, Reload, and `load_loopback` — so Stop is live from the
request, not the first response. `LoadStarted`/`LoadFinished`/
`LoadStopped` and the 15 s net manage the flag consistently
afterward. The fork side of F-008 (`handle_stop_msg` also draining
pending session-history changes) is what makes the resulting Stop
actually abort a stuck navigation; the app-side flag is what makes
Stop reachable for one. See FORK.md.

## Internal pages

Every menu action that is not navigation opens as a page *inside* the
browser, not as a native dialog. The pages are served from the embedder's
own loopback HTTP server, following the servo-de `/daemon/*` proxy
pattern: the engine runs a tiny HTTP server on 127.0.0.1 and the webview
loads it like any other URL.

| URL | Page | Round |
|---|---|---|
| `frost://settings` | Settings (theme, homepage, search engine, privacy) | 3 |
| `frost://print` | Print page: a real preview of the current page (viewport screenshot) + "Save as PNG" to ~/Downloads; says plainly it is not paginated print layout (Servo 0.8.0 has no print pipeline — F-003) | 4 |
| `frost://open` | Open a local file (file picker page, then the file renders in a tab) | 2 — working now (see below); the Round 3 loopback server takes over serving it |
| `frost://exit` | Confirm-exit page (with "don't ask again") | 3 |
| `frost://inspector` | The HTML inspector (DOM tree, computed style, box model; snapshot model — see Inspector section) | 5 (no fork patch — script-injection via `evaluate_javascript`) |
| `frost://extensions` | Extension list, permissions, enable/disable | 3 |

`frost://` URLs are intercepted by the engine before the network stack
sees them (a custom scheme handler in the fork or, first, the loopback
server — the loopback server is the Round 3 implementation; a real scheme
handler is the later, cleaner one).

**Round 2 already ships a working `frost://open` without the server:**
the app generates the directory listing as HTML (`crates/frost/src/
file_browser.rs`, starting at `$HOME`, hidden entries skipped,
directories first) and loads it through the engine as a `data:` page.
Folder links point at `frost://open?path=<url-encoded path>`; the app
intercepts those navigations — on the `UrlChanged` engine event, and
also by polling `webview.url()` each tick in case the engine emits no
event for an unknown-scheme load — and re-lists the folder. File links
are plain `file://` URLs, which Servo loads natively, so clicking a
file opens it in the browser. Settings and Print were placeholder
`data:` pages in Round 2; the Round 3 server serves them for real now
(`frost://settings`, `frost://print`). Print is a real preview in
Round 4 (F-003): the app captures the current page as a PNG before
opening the page, the server serves it as `/print/preview.png`, and
"Save as PNG" writes it to ~/Downloads. It says plainly it is a
viewport screenshot, not paginated print layout — Servo 0.8.0 has no
print pipeline, so there is no print-to-PDF or printer output.

## Extensions

`frost-extensions` defines the contract now so Round 3+ wires it in
without restructuring:

- Manifests (JSON, close to WebExtensions `manifest.json` minus the parts
  Servo will never have).
- Content scripts matched by Chrome-style match patterns against page
  URLs; the engine asks the host `content_scripts_for(url)` on every
  navigation and injects the scripts at `run_at`.
- The `frost.*` bridge (`FrostApi` trait) is the only way extension JS
  talks to the browser: namespaced storage, tab queries, tab creation.
  Page JS never sees it.
- Snowplow (tracker/ad blocking, ROADMAP) is built on the `WebRequest`
  permission — same hook, not a separate system.

## Inspector

`frost-inspector` defines the snapshot contract, and Round 5 fills it
in — with a walker script, NOT a fork patch (F-002 is not needed; see
FORK.md):

- `DomSnapshot`: an owned tree (`DomNode` with tag, attributes, computed
  style, box model) — no live handles into Servo, so it serializes cleanly
  to the `frost://inspector` page. `DomNode` fields match the walker's
  JSON exactly (`node_id, tag, attributes, text, children,
  computed_style, box_model`); `snapshot_from_json` parses and validates
  it (root must be `#document`).
- The walker (`crates/frost-inspector/src/walk.js`, included into the
  crate with `include_str!` as `DOM_WALK_SCRIPT`) is an IIFE that walks
  the live DOM and returns `JSON.stringify({url, capturedAt, truncated,
  nodeCount, root})`. Caps: 8000 nodes, 64 deep, text 200 chars, attr
  values 256 chars; skips whitespace-only text nodes and comments;
  includes `getComputedStyle` (color, background, font, display,
  position, width, height) and `getBoundingClientRect` + margin/padding/
  border. Node 0 is `#document`; element ids count from 1.
- The engine ferries the string: `FrostWebView::evaluate_javascript_string`
  wraps the fork's `WebView::evaluate_javascript(script, callback)` —
  the callback receives `Result<JSValue, JavaScriptEvaluationError>` on
  the app thread during `spin_event_loop`, so the same mpsc-channel
  pattern as `request_screenshot_png` works. Only `JSValue::String` is
  accepted back (a pure `js_string_result` helper, unit-tested).
- **Snapshot model (the single-webview answer).** Frost has one webview,
  so navigating to `frost://inspector` would replace the page being
  inspected. The menu entry / F12 captures the DOM FIRST (walker runs
  in the current page), stores the snapshot in the loopback server's
  shared state, and only then opens `frost://inspector`, which fetches
  `/inspector/snapshot.json` and renders the tree. The page is labeled
  "Snapshot of <url>, captured <time>, N nodes — not a live view", with
  "Back to the page" and "Re-capture" links; "Re-capture" sets a flag
  and 303s back to the inspected page, and the app captures again once
  it loads. Opening `frost://inspector` with no snapshot shows an honest
  "No page inspected yet" page. A failed capture leaves the current
  page up and reports in the status line.
- The inspector page renders the tree with JS +/− toggles (not
  `<details>` — the toggle is Frost's own code); clicking a node fills
  the details pane (tag, full attributes, text, computed style, box).
- What v1 does NOT do: no live editing, no live updates, no
  click-to-inspect from the page (that needs a docked panel inside the
  live page — v2), no console, no network panel, no shadow DOM / iframe
  sub-documents / comments.
