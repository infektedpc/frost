# Frost architecture

## Workspace

| Crate | Role | Depends on |
|---|---|---|
| `frost` | libcosmic app: window, toolbar, address bar, hamburger menu, titlebar (Round 2); tab strip later | `frost-engine`, `libcosmic` (optional, git) |
| `frost-engine` | Servo embedding: engine, webview per tab, event pump, frame readback | fork `servo/components/servo` (path dep) |
| `frost-extensions` | Extension host: manifests, content scripts, `frost.*` bridge trait, stub host | `serde`, `serde_json`, `url` |
| `frost-inspector` | Inspector: DOM snapshot types, picker state, `Inspector` trait, stub | `serde`, `serde_json` |

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
   keys come from winit/libcosmic.

### The partial first paint and the resize jiggle (Round 2)

A load's first paint covers only a ~1024×512 region of a 1280×684
frame, and no later paint repairs it — not a render after
`LoadFinished`, not a reload. What repairs it is a real resize to a
different size (maximizing, restoring, even a 1 px window resize all
paint fully, and the repair persists). So after every `LoadFinished`
(or `LoadStopped`) the app forces a full repaint with a two-phase
resize jiggle: resize 1 px wider, wait ~0.25 s, resize back to the
content size, wait ~0.25 s, then upload. The waits are timed, not
`FrameReady`-driven — advancing on a `FrameReady` resized straight
back on a stale (pre-resize) event, coalescing the two resizes into a
net no-op. The intermediate size is never uploaded, so the user never
sees it. This works around a damage/initialization gap in the engine
(same family as the WayServo ghost finding that dirty rects
under-report); a proper full-repaint trigger in the fork is the
follow-up. History traversals (back/forward) restore from cache and
never fire `LoadFinished` — the 15 s load-timeout safety net clears
the stuck loading state instead.

Later rounds may switch the context to a GPU path (the dmabuf fork patch
exists for exactly this), but the `render() -> Frame` contract does not
change — the UI never sees how the pixels were produced.

## Internal pages

Every menu action that is not navigation opens as a page *inside* the
browser, not as a native dialog. The pages are served from the embedder's
own loopback HTTP server, following the servo-de `/daemon/*` proxy
pattern: the engine runs a tiny HTTP server on 127.0.0.1 and the webview
loads it like any other URL.

| URL | Page | Round |
|---|---|---|
| `frost://settings` | Settings (theme, homepage, search engine, privacy) | 3 |
| `frost://print` | Print preview + print-to-PDF | 3–4 (needs fork patch F-003 if Servo's print pipeline cannot be driven headlessly) |
| `frost://open` | Open a local file (file picker page, then the file renders in a tab) | 2 — working now (see below); the Round 3 loopback server takes over serving it |
| `frost://exit` | Confirm-exit page (with "don't ask again") | 3 |
| `frost://inspect` | The HTML inspector (DOM tree, computed style, box model, picker) | 4 (needs fork patch F-002 for the DOM snapshot hook) |
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
file opens it in the browser. Settings and Print are still placeholder
`data:` pages until the Round 3 server lands.

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

`frost-inspector` defines the snapshot contract now:

- `DomSnapshot`: an owned tree (`DomNode` with tag, attributes, computed
  style, box model) — no live handles into Servo, so it serializes cleanly
  to the `frost://inspect` page.
- `Inspector::inspect(url) -> DomSnapshot` and the element picker
  (`pick_element` maps a viewport point to the deepest node id).
- The real implementation needs fork patch F-002 (a constellation request
  that serializes a DOM subtree), because Servo exposes no DOM snapshot
  API today. The stub returns an empty document so the UI can be built
  against the trait now.
