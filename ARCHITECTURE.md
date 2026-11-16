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

## Menu pages: native windows (Round 7)

Rounds 3–6 served every menu action as a page *inside* the browser,
from the embedder's own loopback HTTP server on 127.0.0.1 (the
servo-de `/daemon/*` proxy pattern). **Round 7 (Joel's direction,
2026-11-14) deleted that server entirely** — `server.rs` is gone, and
each menu item now pops up its own libcosmic window, drawn with iced
widgets in the same process (`crates/frost/src/app/windows.rs`). The
table below is the history of what the server served, and what
replaced it:

| Was (`frost://`) | Now (native window) | Round |
|---|---|---|
| `frost://settings` | Settings window: theme, homepage, search engine, "Ask before quitting" (draft + Save; theme applies on Save) | 3 → 7 |
| `frost://print` | Print window: viewport-screenshot preview + "Save as PNG" to ~/Downloads; still says plainly Servo 0.8.0 has no print pipeline (F-003) | 4 → 7 |
| `frost://open` | Open file window: app-side listing from `$HOME`, dirs first, sizes; folder clicks navigate, file clicks open via `file://`; Markdown renders natively (pulldown-cmark → iced widgets) | 2 → 7 |
| `frost://exit` | Exit confirmation dialog (only when "Ask before quitting" is on) | 3 → 7 |
| `frost://inspector` | Inspector window: the Round 5 snapshot model, unchanged — see Inspector section | 5 → 7 |
| `frost://extensions` | Extensions section inside the Settings window (loaded + failed lists) | 3 → 7 |

Typing a `frost://` URL in the address bar now fails plainly with an
honest status message and leaves the current page alone.
`resolve_address` no longer treats `frost` as a loadable scheme.
Search-engine routing for bare queries and `file://` opening are
unchanged.

Multi-window mechanics (libcosmic 1.0.0 / iced 0.14, vendored
checkout `ecc5fe4`): the `multi-window` feature is enabled on the
libcosmic dep — an empty flag that turns on the two-arg `title(id)` /
`set_window_title(title, id)` and `view_window(id)`. Popups open via
`window::open(Settings)` with `exit_on_close_request: false` (closing
a popup must never quit the browser) and `set_window_title` batched
with the open task; the app subscribes with `event::listen_with` and
routes Opened / Closed / CloseRequested per window id. Windows are
single-instance per kind — opening one that is already open just
focuses it (`window::gain_focus`). `handle_iced_event` and
`on_window_resize` are gated on the main window id, so popup
coordinates never reach the page. The main window's `view()` is
unchanged; `view_window(id)` dispatches to `view_popup(id)` for any
other window, and an unknown id gets an empty container, never a
panic. The `app` module moved from an inline block in `main.rs` to
`crates/frost/src/app.rs` so `windows.rs` can live under it as a
child module (`src/app/windows.rs`) and see `Frost`'s private fields.

**Popup transparency is load-bearing.** iced's wgpu compositor picks
one alpha mode at startup from the main window's surface
(PreMultiplied — libcosmic creates the main window transparent, so
its surface is a 32-bit visual) and applies that mode to every
window's surface. A popup opened opaque gets a 24-bit visual whose
surface supports only [Opaque, Inherit], and `surface.configure`
with PreMultiplied panics fatally, killing the browser (found
2026-11-15 on the first Settings-window open). Every popup is
therefore opened with `transparent: true`; its content paints fully
opaque, so it looks identical.

**Round 2 already shipped a working `frost://open` without the
server:** the app generated the directory listing as HTML
(`crates/frost/src/file_browser.rs`, starting at `$HOME`, hidden
entries skipped, directories first) and loaded it through the engine
as a `data:` page. Folder links pointed at
`frost://open?path=<url-encoded path>`; the app intercepted those
navigations — on the `UrlChanged` engine event, and also by polling
`webview.url()` each tick — and re-listed the folder. File links are
plain `file://` URLs, which Servo loads natively. In Round 7 that
listing became a native window (`file_browser.rs` rewritten to a
native API: `list_dir`, `DirEntry`, `human_size`, `read_markdown`,
`markdown_blocks`); folder navigation is window state now, not
webview history, so it can never disturb Back/Forward on pages.
Settings and Print were placeholder `data:` pages in Round 2; the
Round 3 server served them for real, and Round 7 turned them into
windows. Print is a real preview since Round 4 (F-003): the app
captures the current page as a PNG before opening the window, and
"Save as PNG" writes it to ~/Downloads. It says plainly it is a
viewport screenshot, not paginated print layout — Servo 0.8.0 has no
print pipeline, so there is no print-to-PDF or printer output.

## Hamburger menu: Firefox-style dropdown panel (Round 8)

Rounds 2–7 showed the hamburger menu as a full-width bar under the
toolbar — a row of links spanning the whole window that pushed the
content down (`MENU_HEIGHT`, subtracted in `content_rect`). **Round 8
(Joel's direction, 2026-11-15, from a Firefox application-menu
screenshot) replaced it with a Firefox-style dropdown panel:**
compact (300 px), anchored top-right directly under the ☰ button,
floating over the page with a border, instead of consuming layout
space. `content_rect` no longer subtracts anything for the menu.

There is no built-in dropdown to reach for: the libcosmic book
(15 pages, studied 2026-11-15) documents no dropdown/popover/
context-menu widget for desktop apps — the only anchored floating
panel is the applet popup, which is for panel applets, and
`menu::bar` is a top menu bar, not a hamburger dropdown. So the
panel is a custom overlay in `view()`: an iced `Stack` whose base
layer is the main column (titlebar / toolbar / content / status),
with two layers over it while the menu is open — a transparent
full-window click-catcher (any press outside the panel closes the
menu and does not click through to the page or toolbar underneath)
and then the panel itself. The Stack dispatches events top-first,
so the panel's entries win where they overlap the catcher. The
panel is built from the `MENU_ITEMS` table (label, shortcut,
message) so the spec and the rendering cannot drift; separators
fall after Settings and after Inspector. Its geometry comes from
`menu_panel_geometry(window_width)`: fixed 300 px wide, 8 px from
the right edge, 4 px below the toolbar; a window narrower than the
panel shrinks the panel instead of letting it run off-screen.
Each entry is a `mouse_area`, not a button (the raw iced button's
custom `.style()` closure cannot be used under cosmic's Theme, and
cosmic's buttons are label-only — a row with a right-aligned
shortcut cannot live inside one); hover highlight comes from
`menu_hovered` (`MenuHover`), and the panel/border/hover colors
come from `ChromeColors` (`menu_hover`, `menu_border`), so the
panel follows the Dark/Light theme. Escape closes the menu before
F12/page keys are considered, and keys are blocked while the menu
is open (typing at a menu-covered page would land invisibly).
The five entries and their messages are unchanged from Round 7 —
each opens its existing native window.

## Command palette (Round 9)

A VS Code-style command palette, opened with Ctrl+K (verified
unused in Frost — only F12 and Escape were browser shortcuts) or
from a "Command palette…" entry in the ☰ menu (Ctrl+K shown
right-aligned). It reuses the Round 8 overlay pattern: an iced
`Stack` whose base is the main column, with a dimmed full-window
click-catcher (black, alpha 0.4 — the page is visibly dimmed and
clicks outside close the palette without clicking through) and the
panel on top. The panel is 560 px wide, centered, 100 px from the
top (`palette_panel_geometry`); a narrow window shrinks it instead
of letting it run off-screen. The top row is a libcosmic text input
("Type a command", focused on open); below it, one `mouse_area`
row per matching command (the same panel/hover styles as the menu,
so it follows Dark/Light through `ChromeColors`).

Matching is `fuzzy_match` — a subsequence match, case-insensitive
("set" matches Open Settings and Open Inspector; "k" matches
nothing and the panel shows "No matching commands"). The list is
`palette_commands_for(can_back, can_forward, loading)`: only real,
working actions wired to existing messages, with unavailable ones
hidden rather than shown dead — Go back / Go forward only with
history, Stop loading only while loading; always present are Reload,
Open Settings, Print page…, Open file…, Open Inspector, Focus
address bar, Quit Frost. `move_selection` wraps at both ends.
Arrow Up/Down moves the highlight, Enter runs the highlighted
command (resolving against the CURRENT filtered list), Escape
closes, Ctrl+K toggles, clicking a row runs it. PaletteRun closes
the palette, then dispatches the real message. Opening the palette
closes the menu and vice versa; Escape closes whichever is open,
palette first.

The subtle part is the event path. While the palette is open the
page takes no keys: the Ignored-path KeyPressed handler swallows
everything except Escape/Up/Down/Enter and checks Ctrl+K first.
But a focused text input changes which path a key takes. A focused
text input swallows Escape — except libcosmic's text_input does NOT
capture Escape or Up/Down (those arms return early without
capturing), so they arrive Ignored and the handler sees them. It
DOES capture every other KeyPressed while focused (the vendored
`src/widget/text_input/input.rs` handler ends in an unconditional
`shell.capture_event()`), including Ctrl+K — which therefore never
reaches the Ignored-path toggle. Gated 2026-11-15: three Ctrl+K
presses with the palette open (fast chord x2, slow chord x1) all
left it open. The fix is in `handle_iced_event_captured`, which
the subscription feeds with Captured keyboard events: it now
toggles the palette on a Captured Ctrl+K exactly as the KeyPressed
handler does for an Ignored one (an event is Captured or Ignored,
never both, so it fires once). The same branch also makes Ctrl+K
work from the address bar, which captures it the same way. Enter
is captured too, but its `on_submit` publishes PaletteRunSelected,
so it runs exactly once per press.

## WM titlebar with GNOME fallback (Round 10)

By default the window manager draws the titlebar now, and Frost
draws no titlebar row of its own: the toolbar is the top row of
the window, and the content area starts at `TOOLBAR_HEIGHT`
instead of `TITLEBAR_HEIGHT + TOOLBAR_HEIGHT`. On GNOME — where
Mutter refuses server-side decorations, so a WM bar would mean no
titlebar at all — Frost keeps its own 40 px titlebar row (title
text plus minimize / maximize / close buttons, drag-to-move,
double-press to maximize), exactly as in Rounds 1–9. The mode is
chosen once at startup from the desktop environment and never
changes while the app runs.

The mechanism is libcosmic's own switch, verified in the vendored
source (`src/app/settings.rs`): `Settings` has a
`client_decorations: bool` field (default `true`), documented
"Whether the window should have a border, a title bar, etc. or
not", with a `derive_setters::Setters`-generated setter. The
non-Wayland path in `src/app/mod.rs` turns it into
`iced.window.decorations = !settings.client_decorations`; the
Wayland path passes it into `SctkWindowSettings`, where sctk
negotiates xdg-decoration with the compositor. The env check
matters most on X11, Frost's current path.

In `app.rs` the rule is `use_client_decorations(desktop: &str)
-> bool` — pure, unit-tested, case-insensitive substring match
on "gnome" (so "ubuntu:GNOME" matches too); `detected_desktop()`
resolves the string from `XDG_CURRENT_DESKTOP`, falling back to
`XDG_SESSION_DESKTOP`, empty when neither is set (which means the
WM bar). `run()` hands the flag to libcosmic
(`Settings::default().size(...).client_decorations(flag)`) and to
the app as its `Flags` (changed from `()` to `bool`), so the view
and the window cannot disagree. The `Frost` struct stores it as
`client_decorations: bool`; `titlebar_height()` returns 40 px in
the fallback and 0 in WM mode, and `content_rect()`, the initial
webview height, and the menu panel's top padding all read it —
nothing hardcodes the old 40 px offset any more. The view still
builds the titlebar row and its buttons, but only pushes the row
into the main column in fallback mode; the titlebar messages and
their handlers stay live behind the flag. `titlebar_text()` still
feeds `set_window_title` on every Tick, so the OS window title is
correct in both modes. The built-in libcosmic headerbar stays
hidden in both modes (`show_headerbar = false`).

Popups need no change: their `window::open` calls already use
`window::Settings` with `..Default::default()`, and iced's
default `decorations` is `true` — so the five native windows get
WM titlebars on decorating desktops in both modes, which fixes
the Round 7 wart of the Settings window having no in-content
Close button. `transparent: true` stays on the popups: it is
about the surface's alpha mode (Round 7 — iced picks one alpha
mode from the main window's surface; an opaque popup's 24-bit
visual panics on configure), while decorations are about the WM
frame; the two are orthogonal.

What the sandbox cannot prove: bare Xvfb has no window manager,
so the WM-drawn titlebar itself (its look, real min/max/close)
is unverifiable here — the window simply shows no titlebar under
Xvfb. Joel's KDE box is the verdict for the actual titlebar.

Gate results (2026-11-16, Xvfb :97): tests 56/56; build green
in 42m 53s serialized. WM mode (`XDG_CURRENT_DESKTOP=KDE`): no
client titlebar row, toolbar on top, OS title "Frost — Example
Domain", menu panel 4 px under the toolbar, all five popups +
the palette + F12 Inspector (35 nodes) + Print Save +
Back/Forward + strip-scroll all pass, `frost://` fails plainly,
no listener, settings persist across relaunch. GNOME mode: the
own titlebar row and its buttons are back, menu panel under the
toolbar. `XDG_SESSION_DESKTOP=GNOME` alone (current unset) also
restores the own bar. `transparent: true` popups and server-side
decorations do not conflict — every popup opened, rendered, and
closed cleanly.

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
  so an inspector that lived in it would replace the page being
  inspected. The menu entry / F12 captures the DOM FIRST (walker runs
  in the current page), and only then opens the Inspector window on
  the snapshot (Rounds 3–6 stored it in the loopback server's shared
  state and opened `frost://inspector`; Round 7 holds it in the
  `InspectorWindow` state — same model, no server). The window is
  labeled "Snapshot of <url>, captured <time>, N nodes — not a live
  view", with "Back to the page" and "Re-capture" buttons;
  "Re-capture" re-runs the walker on the live page (which never left
  the main window) and replaces the snapshot in place. A failed
  capture leaves the current page up and reports in the status line.
- The Inspector window renders the tree with +/− toggles (iced
  buttons; the toggle is Frost's own code); clicking a node fills
  the details pane (tag, full attributes, text, computed style, box).
- What v1 does NOT do: no live editing, no live updates, no
  click-to-inspect from the page (that needs a docked panel inside the
  live page — v2), no console, no network panel, no shadow DOM / iframe
  sub-documents / comments.

## Merged reload/stop button + round toolbar icons (Round 11)

The toolbar's reload and stop buttons are one button now, and all
four toolbar icon buttons are circles. Both changes live in
`crates/frost/src/app.rs` only; the fork is untouched.

The merged button is a pure function of the `loading` flag Frost
already tracks (true on navigation request, false on
LoadFinished / LoadStopped, Round 4 / F-008):
`reload_stop_button(loading)` maps idle → ("⟳", `Message::Reload`)
and loading → ("✕", `Message::Stop`). The view renders whichever
the flag says, so the icon flips back to ⟳ in both end states
without any extra bookkeeping. The command palette's Reload /
Stop loading entries already hide/show by the same flag and were
not touched; keyboard reload is untouched (F5 is forwarded to the
page via `send_key`, there is no app-level F5/Ctrl+R handler).

The roundness fix is a geometry fact, not a style change. The
pill shape came only from wider-than-tall bounds: a libcosmic
`button::standard` wraps its label in `space_s` (16 px) of
horizontal padding, so a 32 px tall button with a glyph was ~64 px
wide. The theme's button corner radius `radius_xl = [160.0; 4]`
(cosmic-theme `corner.rs`) is already far past half of a 32 px
side, so the radius was never the constraint — the bounds were.
`round_icon_button()` therefore does the whole job with square
32×32 bounds (`TOOLBAR_ICON_SIZE`, the toolbar inner height: the
48 px row minus 2×8 px row padding) and zero padding, keeping the
caller's style and `on_press`. No `Button::Custom` style was
needed. The GNOME-fallback titlebar's min/max/close buttons were
already circular and are untouched; the address bar is a text
field, not an icon button, and stays as-is. Menu geometry and
`content_rect()` go through `titlebar_height()` as before, so the
panel still anchors 4 px under the toolbar in both WM and
fallback modes.

History state, for the record: `can_back` / `can_forward` are
refreshed only in `handle_engine_event` on `EngineEvent::UrlChanged`,
read from `webview.can_go_back()` / `can_go_forward()` — which read
Servo's `back_forward_list_index`, updated in `set_history` before
`notify_url_changed` fires, so the ordering is correct at the
engine level. The palette hides "Go back" / "Go forward" when the
flags are false, which makes it the reliable read of the state
(the button's enabled/disabled shade does not visibly differ in
this theme). Round 11 gating found one session where the flags
stayed false after a stop-plus-same-URL-re-navigation-plus-Back-
during-load interleaving; the minimal hang → stop → navigate
sequence passes on the same binary, so the stuck state is a
pre-existing engine-side quirk (Round 4 F-008 abort + Servo 0.8
constellation session history), not something this round's
toolbar change can cause or fix app-side.

## Toolbar glyphs: bigger + centered (Round 12)

Round 11 made the toolbar buttons circles, but the glyphs
inside them were small (~10 px) and left-of-center — Joel's
0.11.0 verdict. The cause is in libcosmic's `TextButton`
(vendored source, pinned rev `ecc5fe4`, `src/widget/button/text.rs`):
the label lives in an inner row that is only
`.align_y(Alignment::Center)` — horizontal layout is
left-aligned by default — at the default 14 px font / 20 px
line height, with no horizontal-alignment control exposed on
the Builder. Square bounds + zero padding cannot center a
label that the widget itself left-aligns. (Read `ecc5fe4`,
not the stale `9c62f19` checkout in the same directory: in
`ecc5fe4` the style API is `button::ButtonClass` with the
widget method `.class()`; `button::Style` is the iced
catalog `Style` struct.)

The fix keeps the same `Button` widget and theme classes
and changes only the content: `round_icon_button()` builds
on `button::custom` with a 32×32 `container` holding
`text(label).size(TOOLBAR_GLYPH_SIZE)` (20.0) and
`.align_x(Alignment::Center)` + `.align_y(Alignment::Center)`,
outer button 32×32 + zero padding + `.class(class)`.
Hover/pressed/disabled rendering is unchanged because the
widget and the class are unchanged.

Two glyph facts, both measured from pixels:

- Back/forward used the single guillemets ‹/›, which are
  quotation marks: their ink is ~6×9 px at 20 px and sits
  ~3 px low in the em (4×8 px ink, dy +3.0 on the Round 11
  build). They are now the proper heavy chevrons ❮/❯
  (U+276E/U+276F), ~10×20 px ink at the same 20 px size.
  The chevrons come from the DejaVu Sans fallback font
  (Noto Sans, the sandbox's default sans, has no U+276E;
  the other toolbar glyphs ↻/☰/✕ fall back to DejaVu too,
  so the toolbar already mixed fonts — the swap makes
  back/forward match the others' source font as well).
- ❮/❯ ink still sits ~2 px low next to ↻/☰/✕ at the same
  size, so the chevron buttons pass
  `TOOLBAR_CHEVRON_NUDGE_UP = 2.0` to `round_icon_button`,
  which applies it as bottom padding on the glyph
  container: padding shrinks the centered area from the
  bottom, so the glyph sits higher by half the padding.
  Reload/stop and ☰ pass 0.0. Gated result on the final
  binary: every glyph's bbox center within 0.5 px of its
  button's center, in both WM mode and the GNOME fallback
  (toolbar offset 40 px). The GNOME titlebar's own
  min/max/close glyphs render centered and are untouched.

Correction to Round 11's note above: the enabled/disabled
shade DOES differ measurably — disabled buttons render
(58,57,65), enabled (72,71,78) — so the button shade is a
usable can_back read after all. Round 12 gating also
reproduced the stuck-history quirk in a hang → stop →
same-URL-reload → stop instance (palette hides "Go back" /
"Go forward", the following navigation never commits);
the clean sequence passes on the same binary, so the
history path is intact and the quirk is unchanged from
Round 11.

## Round 13 — Snowplow v1 (embedder-side blocking)

No fork patch. The 0.8.0 fork already exposes the hook
ServoGTK 0.7.0 uses: `WebViewDelegate::load_web_resource`
is called for every resource fetch the net layer is about
to start, with a `WebResourceLoad` the embedder can
cancel. `servo` re-exports `embedder_traits::*`, so the
engine crate reaches it with no new dependency. The
delegate implementation lives in `frost-engine`
(`crates/frost-engine/src/lib.rs:316`): it hands the load
to `snowplow::handle_load` with the shared state and the
event sender. `handle_load` (`snowplow.rs:245`) returns
`false` (→ `DoNotIntercept`, fetch proceeds untouched)
when Snowplow is off, when `request.is_for_main_frame`,
or when the URL is not blocked; otherwise it calls
`load.intercept(WebResourceResponse::new(url)).cancel()`
and sends `EngineEvent::ResourceBlocked(url)`. The cancel
produces a fast network error at the page ("Load
cancelled"), never a hang — the F-008 stop machinery is
never involved, and the gate log proves blocked requests
never reach `http_loader` (no `GET` for the blocked
hosts, only the script's cancel error).

State: the `Engine` owns one `Rc<RefCell<Snowplow>>`,
cloned into the delegate at construction, so the Settings
toggle (`Engine::set_snowplow_enabled`) and the delegate
read the same flag with no channel round-trip. The
blocklist is a `&[&str]` constant (62 entries, copied
from ServoGTK and deduped); `host_matches` is
exact-or-suffix on dot boundaries, lowercases, and strips
one trailing dot, so `ads.example.com` matches
`example.com` and `notexample.com` never does.
`FROST_SNOWPLOW_EXTRA_BLOCKLIST` (comma/space separated)
adds entries at construction — the gates block
`localhost` through it. `Settings.snowplow` defaults to
`true` via `#[serde(default)]`, so pre-Round-13 settings
files parse unchanged.

The per-page count lives in the app (`snowplow_blocked`,
incremented once per `ResourceBlocked` event, rendered
as "· Snowplow blocked N" only when N > 0). It resets on
`LoadStarted` AND on `UrlChanged`: history traversals
commit a new URL without a `LoadStarted`, and without
the second reset the previous page's count lingered on
the new page (found in gating, fixed in-round). The
count is intercepted requests, not unique URLs — Servo
offers a subresource to the delegate once or twice
(preload + real fetch) depending on timing, so the same
2-blocked-resource page showed 3 and 4 on different
loads. Same-document fragment navigations also reset
the count; accepted for v1.

What v1 deliberately does not do: main-frame blocking
(the hook could do it — `is_for_main_frame` is checked
and passed through — but blocking a navigation the user
explicitly typed is a v2 decision), per-site allow/deny,
or list management in the UI. The roadmap sketch's
"in the engine's network stack" phrasing is superseded:
v1 blocks at the embedder delegate, before the fetch
starts, which is the same observable point with no fork
patch to maintain.

Build note: the `target/` tree had grown past the
workspace's disk headroom (102 GB; the plain 2 GB debug
link died twice with `rust-lld` SIGBUS and a rebuild
attempt hit "Disk quota exceeded"). Deleting
`target/debug/incremental` + `target/debug/examples`
restored the link. Before the next big build, check `df`
— and never extract large things to /tmp (512 MB tmpfs)
or let `target/` grow unchecked.

## Round 14 — Split view (two webviews, one window)

The app owns one webview per pane, not one per window.
`Frost` holds `panes: Vec<Pane>`, `active_pane: usize`,
and `split: bool`. `Pane` (`crates/frost/src/app.rs:453`)
is everything that used to be app-level per page: its
`FrostWebView`, the Round 6 frame strips, `loading`,
`title`, `status`, the Round 13 `snowplow_blocked` count,
`can_back` / `can_forward`, `awaiting_load`,
`loading_since`, and `injected_for`. The single-view
path is the same shape with one pane, so no
single-view behavior changed shape — it just reads
through `active_pane()` / `active_pane_mut()` now. The
old `Frost::webview` / `loading` / `status` fields are
gone; `load_url_in_main` (`app/windows.rs`) loads in the
active pane and marks it loading via `Pane::mark_loading`
(Round 4, F-008: Stop stays live even if the load never
reports a start).

Routing: the engine tags every event with the webview it
came from. All `EngineEvent` variants carry a `WebViewId`
first (`TitleChanged`, `UrlChanged`, `LoadStarted`,
`LoadFinished`, `LoadStopped`, `FrameReady`,
`ResourceBlocked`, `Crashed`); the delegate reads it off
`webview.id()` in each notify callback, `FrostWebView::id()`
exposes it, and `servo::WebViewId` is re-exported for the
UI. The tick drains events and looks the id up in the
panes (a pane is found by matching webview id, not by
index — indices shift when a pane closes). Snowplow's
`handle_load` takes the id so `ResourceBlocked` lands on
the right pane's count; the status line shows the ACTIVE
pane's count only. The count resets per pane on
`LoadStarted` and `UrlChanged`, as in Round 13.

Geometry is pure functions, unit-tested (3 new tests:
`split_pane_rects_tile_the_content_area`,
`pane_at_point_picks_the_half`,
`pane_viewport_size_subtracts_the_border_when_split`).
`split_pane_rects` splits the content rect 50/50 (left
takes `floor(width / 2)`, right the rest — no lost pixel
on odd widths, no gap, no overlap). `pane_at_point`
picks the pane for a click (0 in single view).
`pane_viewport_size` is the pane rect minus 2×PANE_BORDER
(2 px) on each side while split — the border is
layout-neutral in iced, so the viewport math already
accounts for it; the single-view viewport is the whole
content area, as before. The active pane's border is the
theme border color; the inactive pane draws the same
2 px border transparent, so activation never shifts
layout. `resize_panes` re-sizes both webviews after any
split/close/resize.

Lifecycle: `toggle_split` opens the second pane on the
active pane's URL (homepage fallback) with the new
webview's theme set before first paint; closing keeps
the ACTIVE pane and `Vec::remove`s the other, whose
`FrostWebView` drop closes it in the engine
(`Drop for WebViewInner` sends `CloseWebView`) — no
pipeline survives a close. The menu entry (index 5 in
`MENU_ITEMS`, dynamic label via `SPLIT_MENU_INDEX`) and
the palette entry (`palette_commands_for` takes `split`)
both send `Message::ToggleSplit`.

Finisher fixes (gating finds, same patterns as the
surrounding code): the `ToggleSplit` arm now closes the
menu and the palette before toggling — it was the only
command arm that closed neither, so the dropdown stayed
open over the new split. And `handle_iced_event_captured`
gained an F12 branch mirroring the subscription's F12
handler: a focused text input captures every KeyPressed,
so F12 from the address bar never reached the
subscription (the Round 9 Ctrl+K shape, fixed the same
way); an event is Captured or Ignored, never both, so it
fires once. The stale "Capturing…" status after an
Inspector capture is still there (Round 7 wart).

Build note: the plain 2 GB debug link OOM-killed twice
in the sandbox this round (`ld` signal 9; ~6 GiB cell,
host under memory pressure) and a runtime restart killed
a retry mid-link. The gate binary was linked with
`-C link-arg=-Wl,--strip-debug` via `cargo rustc -p frost
--bins` (517 MB, same code, no debuginfo). No profile
change shipped; the zip's source builds the normal dev
binary.
