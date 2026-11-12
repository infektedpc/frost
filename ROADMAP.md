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

## Round 2 — The window goes live

- Wire `frost-engine` into the libcosmic app: content area shows the live
  Servo frame (texture upload per `FrameReady`), `spin()` driven by the UI
  loop.
- Address bar navigates; back/forward/reload call the engine for real.
- winit/libcosmic → `FrostInputEvent` mapping (keyboard, mouse, wheel,
  scroll). No `+8` xkb fix — that was Wayland-seat-only.
- Tab strip: new tab / close tab / switch tab (one `FrostWebView` each).
- Gate: browse real sites in the window on Joel's box; typing works.

## Round 3 — Internal pages + extensions v1

- Embedder loopback server serves `frost://settings`, `frost://print`,
  `frost://open`, `frost://exit` (later: a real scheme handler).
- Settings page: theme, homepage, default search engine, privacy toggles.
- Extension host v1: load a manifest from disk, inject content scripts on
  navigation, `frost.storage` + `frost.tabs` bridge live.
- `frost://extensions` management page.
- Gate: a sample extension (e.g. a dark-mode content script) runs on a
  real page.

## Round 4 — Inspector v1

- Fork patch F-002: constellation DOM snapshot hook.
- `frost://inspect`: DOM tree, computed style, box model, element picker
  with highlight overlay.
- Gate: inspect example.org, pick an element, see its real style + box.

## Round 5 — Fork patch F-001 (stop) + print

- Fork patch F-001: `WebView::stop()`; Stop button goes live.
- Print: `frost://print` preview; print-to-PDF if F-003 lands, otherwise
  paginated-image export.

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
| No HTML inspector | We build our own (`frost-inspector` + fork patch F-002). |
| Limited print support | Fork patch F-003 or paginated-image export from the render path. |
| No PDF viewer | Later round: render PDFs via the engine (pdf.js-style, Servo-rendered) or a native viewer page. |
| No spellcheck | Fork patch F-004 or a pure-Rust spellcheck in the UI layer. |
| No `WebView::stop` | Fork patch F-001; `stop()` is a documented `Unsupported` until then. |
| No WebGL in the sandbox feature set | Deliberate Round 1 cut (no libclang for mozangle bindgen, no gstreamer). Revisit when the build machine has them; Joel's box may already. |
| No media (audio/video) in the sandbox feature set | Same cut (no gstreamer). Same revisit. |

## Non-goals

- Firefox extension compatibility (no WebExtensions runtime exists in
  Servo; Frost extensions are Frost-native by design).
- A second engine. Servo is the renderer, full stop.
- Mobile. Desktop Linux first (Debian 13 / KDE is the reference box).
