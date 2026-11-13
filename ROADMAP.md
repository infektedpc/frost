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

## Round 3 — Internal pages + extensions v1

- Embedder loopback server serves `frost://settings`, `frost://print`,
  `frost://exit` (later: a real scheme handler). `frost://open` already
  works as an app-side listing (Round 2); the server takes over serving
  it when it lands.
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

## Round 5 — Print

- Print: `frost://print` preview; print-to-PDF if F-003 lands, otherwise
  paginated-image export.
- (F-001 — the Stop button — was pulled forward and landed in Round 2.)

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
| No `WebView::stop` | Fork patch F-001 — LANDED in Round 2 (real `stop()`; the Stop button is enabled while loading). Limitation: a navigation stuck at the constellation is not aborted (needs an `AbortLoadUrl` message). |
| No WebGL in the sandbox feature set | Deliberate Round 1 cut (no libclang for mozangle bindgen, no gstreamer). Revisit when the build machine has them; Joel's box may already. |
| No media (audio/video) in the sandbox feature set | Same cut (no gstreamer). Same revisit. |

## Non-goals

- Firefox extension compatibility (no WebExtensions runtime exists in
  Servo; Frost extensions are Frost-native by design).
- A second engine. Servo is the renderer, full stop.
- Mobile. Desktop Linux first (Debian 13 / KDE is the reference box).
