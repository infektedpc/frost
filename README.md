# Frost

A browser built in Rust on the Servo engine, with a libcosmic UI. Small
parts, every step committed, no giant rewrites: the extension host and the
HTML inspector are scaffolded from day one so they never need bolting on
later.

**Status: Round 8 — Firefox-style hamburger dropdown.** Joel's
direction (2026-11-15, with a Firefox application-menu screenshot):
the hamburger menu should drop down like Firefox's — a compact
panel anchored top-right under the ☰ button, not the full-width bar
of Rounds 2–7. Same five items, same native windows, same behavior;
the panel is 300 px wide with separators, F12 next to Inspector,
hover highlight, and closes on pick / Escape / click-outside / ☰
again. No fork patch; the fork is untouched at `f9462dbb7`. See
[ROADMAP.md](ROADMAP.md) for the plan.

## Layout

```
frost/
├── Cargo.toml            # cargo workspace
├── rust-toolchain.toml   # Rust 1.99.1 (matches the Servo fork's pin)
├── FORK.md               # the Servo fork: point, patches, planned patches
├── clone-fork.sh         # recreate servo/ from scratch (shallow v0.8.0 + patches)
├── patches/              # the eight proven 0.8.0 engine patches (vendored)
├── servo/                # the Servo fork (NESTED repo, gitignored, not in the zip)
├── crates/
│   ├── frost/            # libcosmic app: toolbar, address bar, hamburger menu
│   ├── frost-engine/     # Servo embedding wrapper + headless smoke test
│   ├── frost-extensions/ # extension host scaffolding (manifests, content scripts, frost.* API)
│   └── frost-inspector/  # HTML inspector scaffolding (DOM snapshots, element picker)
├── README.md / ARCHITECTURE.md / ROADMAP.md
└── smoke.png             # smoke-test screenshot (generated, gitignored)
```

## The fork

Frost builds against its own Servo fork at `servo/`, not upstream. The
fork point is **v0.8.0** (`4e136d8e…`) plus eight engine patches as fork
commits (keypress charCode, damage rects, dmabuf export, F-001 stop,
F-005 refresh-viewport, F-006 reload-notify, F-007 full-present,
F-008 stop-aborts-pending). Full detail in [FORK.md](FORK.md). To recreate it on another machine:

```bash
./clone-fork.sh
```

The fork is a nested repo with its own `.git`; the frost repo never tracks
it. It is also excluded from the delivery zip — it is reproducible from
`clone-fork.sh` and far too big to ship.

## Building

```bash
# 1. Recreate the fork (first time only)
./clone-fork.sh

# 2. Check everything (engine + scaffolding + app stub)
cargo check --workspace --all-targets

# 3. Headless proof: load example.org and screenshot it
cargo run -p frost-engine --example smoke
# writes smoke.png (1280x800) — must be a real page, not a solid color

# 4. The window (needs libcosmic's system libraries; see below).
#    `ui-libcosmic` is the default feature since Round 2.
cargo run -p frost
```

Without the `ui-libcosmic` feature (`cargo run -p frost --no-default-features`)
the `frost` binary prints a status note and exits — the workspace always
compiles even where libcosmic cannot.

### Toolchain

Rust **1.99.1**, pinned in `rust-toolchain.toml` to match the fork
(`servo/rust-toolchain.toml`). Rustup downloads it on first use if it is
not installed yet.

### libcosmic

libcosmic is **not on crates.io** — it comes from git
(`pop-os/libcosmic`). The v0.12 tag could NOT resolve (its wgpu git pin
needs a `web-sys` feature, `GpuImageCopyBuffer`, that no published
web-sys has), so the workspace pins libcosmic to HEAD rev `ecc5fe4`
(see `crates/frost/Cargo.toml`). It is an **optional** dependency
behind the `ui-libcosmic` feature (the default since Round 2), so the
engine proof never depends on it. Building the window needs the system
libraries libcosmic/iced link against (Wayland client, libxkbcommon,
fontconfig, and friends). If the build fails on a missing system library,
that is a Round 1 known limit: the engine is the gate, the window follows
once the libraries are there.

libcosmic defaults to transparent windows, which makes X11 hand the app
a depth-32 TrueColor visual — and iced's tiny-skia/softbuffer renderer
only accepts depth-24 visuals, so the window panics at startup
(`Visual 0xab does not use softbuffer's pixel format`). The fix is the
`libcosmic/wgpu` feature (added to `ui-libcosmic` in Round 2): iced
falls back to its wgpu renderer first, which accepts the visual. Proven
on Joel's box 2026-11-12 (the Round 1 window launched with the patch)
and in the sandbox under Xvfb.

## Packaging

### .deb (Debian 13, amd64)

`packaging/build-deb-0.4.0.sh` builds `frost-0.4.0_amd64.deb` (binary
`/usr/bin/frost`, desktop file, snowflake icon). It uses `dpkg-deb
--build` only and never installs anything itself; on the target machine:

```bash
sudo dpkg -i frost-0.4.0_amd64.deb
sudo apt-get install -f   # only if a dependency is missing
```

**Plain truth about 0.4.0:** it ships the Round 4 tree as a **stripped
debug build** (~300 MB), not a release build — a full release build of
the Servo fork + workspace was not feasible in the sandbox. It is fully
functional, just larger and slower than a release build would be. The
note is in the package Description too; remove it only when a real
release build replaces the binary. The packaged binary was gated from
a clean extraction: window launches, example.org paints fully, the
hamburger menu shows all four entries, `frost://settings` loads.

### .deb 0.7.0 (Round 7, Debian 13, amd64)

`packaging/build-deb-0.7.0.sh` builds `frost-0.7.0_amd64.deb` (SHA-256
`acaf4e01f5f036b46349bfd1eb52ec5d0e39282743b76048408236903f3fa118`,
64,676,464 B; stripped binary 318,681,992 B) from the Round 7 tree
(HEAD `bad2374`, same proven build11c binary — no rebuild). Same
contents: `/usr/bin/frost`, desktop file, snowflake icon. Same honest
stripped-DEBUG note in the Description. Depends were re-derived from
`ldd` + `/proc` maps of the running Round 7 binary; two changes vs
0.4.0: `libvulkan1` added (the Round 7 binary loads system
`libvulkan.so.1` and the lavapipe ICD in the gated run — wgpu takes
the Vulkan path here, where 0.4.0's run took GL) and
`libwayland-server0` added (`libwayland-server.so.0` is mapped by the
running process). The packaged binary was gated from a clean
extraction: launch + example.org full paint + banner, all five menu
entries (Settings and Inspector opened as native windows, Inspector
capturing a real 35-node snapshot), `frost://settings` failing plainly
with the honest status message, and no loopback listener. Install the
same way: `sudo dpkg -i frost-0.7.0_amd64.deb` (+ `sudo apt-get
install -f` only if a dep is missing). Never installed with `dpkg -i`
in the sandbox — the real install is Joel's-box verdict.

Harness note (2026-11-15): under bare Xvfb, OS-closing the Settings
popup with `xdotool windowclose` killed the whole app — winit panicked
in `GetGeometry` on the destroyed window id. That is a bare-Xvfb
harness artifact (no window manager); the designed close path is the
WM close button on a real desktop, and the in-app "Back to the page"
button closes Inspector cleanly. Do not treat the Xvfb panic as a
packaging failure.

### Flatpak (manifest only — UNBUILT, UNVERIFIED)

`packaging/org.frost.Frost.yml` + `packaging/build-flatpak.sh` build a
Flatpak **on Joel's machine** (the sandbox has no flatpak tooling at
all, so the manifest was never built or tested there). It clones the
GitHub repo (`infektedpc/frost`, branch `main`) and recreates the Servo
fork with `./clone-fork.sh`, then does a **release** build. It needs
network during the build (`--share=network`) and hours, not minutes.

## Pushing to GitHub

No remote exists yet — the sandbox has no logged-in `gh`. When you are
ready, from the frost repo root:

```bash
# Create the repo on GitHub first (web UI, or `gh repo create frost --private`)
git remote add origin git@github.com:<your-user>/frost.git
git push -u origin main
```

The `servo/` fork is a separate nested repo; push it separately if you
want it on GitHub too:

```bash
git -C servo remote add origin git@github.com:<your-user>/servo.git
git -C servo push -u origin HEAD:main
```

## Round 1 — what is proven, what is not

Proven (sandbox, 2026-11-12, Rust 1.99.1):

- The fork clones shallow at v0.8.0 (`4e136d8e…`) and all three patches
  apply clean (one commit each).
- `cargo check --workspace --all-targets` green; `cargo check -p frost
  --features ui-libcosmic` green (libcosmic HEAD `ecc5fe4` — the v0.12
  tag could NOT resolve: its wgpu git pin needs a `web-sys` feature,
  `GpuImageCopyBuffer`, that no published web-sys has).
- Unit tests: 13/13 pass (`cargo test -p frost-engine -p
  frost-extensions -p frost-inspector --lib`).
- The smoke test loads live `https://example.org` and writes
  `smoke.png` — 1280×800 RGBA PNG, 1248 distinct colors, verified
  visually as a real rendered page (multilingual text, link, icon).
  SHA-256: `d382c2ce9896375b7ad24c040d369448785b463a6a9a2d2916bf8137cd5d49ad`.
- The engine drives the paint itself (`make_current` + `webview.paint()`
  + `read_to_image` + `present()`); without `paint()` every frame came
  back a single solid color — caught by the smoke gate, fixed, re-proven.

Not proven:

- Anything on Joel's box (Debian 13, Mesa GL) beyond the Round 1 window
  launch — the Round 2 gates ran under Xvfb in the sandbox; his run of
  the Round 2 zip is the verdict on real hardware, real mouse/keyboard,
  and the wgpu adapter his Haswell picks.
- Real pages beyond example.org / example.com / iana.org; media/WebGL
  are off in this feature set (same cuts as the proven servo-de
  embedder: no webgl/webxr/ media-gstreamer in the sandbox build).
- `uv` is not installed in the sandbox; Servo's script-bindings build
  script warns about it and falls back to system python. The build
  works, but installing `uv` would silence the warning.

## Round 2 — what is proven, what is not

Proven (sandbox, 2026-11-13, under Xvfb + software GL):

- The window launches (`cargo run -p frost`) and shows Frost's own
  titlebar ("Frost — Example Domain", real minimize / maximize /
  close buttons, text glyphs), the toolbar (Back / Forward / Reload /
  Stop / address bar / hamburger), and a fully rendered example.org.
- Address-bar navigation: typed `example.com`, Enter — the page
  changed to `https://example.com/` (address bar + network log).
- Back/forward: example.org → example.com → Back → example.org →
  Forward → example.com, proven by URL events. (History traversals
  restore from cache and never fire `LoadFinished`; the 15 s timeout
  safety net now also clears the loading state and status line.)
- Reload: a frame event follows reload and the repaint is full.
- Menu: the hamburger opens a dropdown; Settings / Print load their
  placeholder pages; Open is a real file browser (home listing,
  folder navigation, file opens in the browser); Exit quits.
- Resize: 1280×800 → 1000×600 re-renders at 1000×484; back to
  1280×800 re-renders at 1280×684, full both ways.
- Input: clicking example.org's "Learn more" link navigates to
  iana.org; keyboard scrolling (End) works; typing in the address bar
  works (needs X input focus under bare Xvfb — the harness uses
  `xfocus` for that).
- Stop is live: fork patch F-001 adds a real `WebView::stop()`; the
  Stop button is enabled while loading.
- The partial first paint is fixed by the two-phase resize jiggle
  (see ARCHITECTURE.md): the first launch paints the full 1280×684
  frame, and a reload repaints fully too.
- `cargo check --workspace --all-targets` green; unit tests green
  (see `cargo test`).

Not proven:

- Mouse-wheel scrolling under Xvfb: synthetic wheel events (XSendEvent
  button 4/5) do not scroll the page in the sandbox. Keyboard scrolling
  works, so the engine side is proven; whether the wheel path is a
  harness artifact or a real gap needs a real mouse — Joel's box is the
  verdict.
- Stop on a navigation stuck at the constellation: F-001 aborts the
  document via `window.stop()` but does not send an `AbortLoadUrl` —
  that is the noted TODO in the fork.
- A proper full-repaint trigger in the fork: the jiggle is a workaround
  for a damage/initialization gap in the engine (see ARCHITECTURE.md).

## Round 3 — what is proven, what is not

Proven (sandbox, 2026-11-13, under Xvfb + software GL; finish pass
re-ran the gates live against the shipped tree, HEAD `a7cc23d`):

- `cargo check --workspace --all-targets` green; unit tests **36/36**
  pass (`cargo test --workspace`: frost bin 20, frost-engine 3,
  frost-extensions 8, frost-inspector 5).
- The loopback server serves `frost://settings`, `frost://print`,
  `frost://exit`, `frost://open`, `frost://extensions`, and
  `frost://md` — the address bar shows the `frost://` URL, never a
  `data:` URL (the Round 2 wart is gone).
- Settings are wired for real: theme (Dark/Light links apply
  immediately and save), homepage, default search engine, "ask before
  quitting". Saved to `$XDG_CONFIG_HOME/frost/settings.json`; a
  relaunch opens the saved homepage in the saved theme (proven live:
  homepage `https://example.com` + Light theme survived a restart).
  An invalid homepage is rejected with "That homepage is not a valid
  http(s) URL — nothing was changed."
- Search: typing a query in the address bar routes to the configured
  engine — "frost browser" went to DuckDuckGo and the results page
  rendered in full.
- Extension host v1: the sample banner extension loads from disk and
  its blue "❄ Frost sample extension is live" bar shows on
  example.org; a deliberately broken manifest is skipped with a log
  line and no crash; the banner survives a reload (the `Reload`
  message clears the content-script guard — Servo 0.8.0
  `WebView::reload()` never notifies the delegate, so without the
  fix scripts never re-ran after a reload). `frost://extensions`
  lists loaded and failed extensions.
- Menu: all four entries fully visible, no clipping (the Round 2
  ~4 px clip of Exit is fixed).
- Markdown: `.md` files render as formatted HTML (heading, bold,
  list, link) via `frost://md`, with "Back to folder" and "Raw file"
  links — no more Servo "Unknown content type" page.
- File browser end to end: Open → home listing → folder clicks
  re-list (`user/`, `workspace/`, `harness-tmp/`, `frost/`,
  `r3-files/`) → file clicks open (`note.md` renders, `keytest.html`
  loads as `file://`).
- Print and Exit are real pages: `frost://print` names the page and
  says plainly that printing is not wired up yet (F-003, planned);
  `frost://exit` asks "Quit Frost?" and the Quit link exits the app
  (process gone, `/exit/confirm` 200).
- Typing into page inputs works: the key event's produced text is
  preferred (HEAD `a7cc23d`), proven by typing "abc" into the
  keytest page's input and by editing the settings fields.
- F-005 (`WebView::refresh_viewport()`) LANDED in the fork (commit
  `7357f4ebc`) but was PROVED INSUFFICIENT as the full-repaint repair
  (2 of 3 first loads painted partial ~1024×512 and stayed partial);
  the Round 2 resize jiggle is restored as the reliable repair and
  first paints are full again. F-005 stays in the fork as a harmless
  viewport re-send; the real fix is a future fork patch (F-007: force
  a reflow at an unchanged viewport).

Not proven / known limits:

- **Tiny instant-loading local HTML pages may never paint their body.**
  The keytest page (`file://…/keytest.html`, 529 B, script focuses an
  input on load) loads fine — title known, input focusable, typed text
  lands in it — but its body never paints on load or on reload; only
  the tiles touched by interaction paint (the typed "abc" appeared
  alone on a blank page). It painted fully exactly once, transiently,
  after a click. This held across all three finish attempts and both
  load and reload. Real pages (example.org/com, DuckDuckGo, all
  `frost://` pages, file listings, Markdown) paint fully. Same damage
  family as the F-005 story; F-007 is the real fix.
- **Ctrl+A (select-all) does not work in page text inputs** under the
  harness: the chord produced no selection, so typed text appended to
  the existing value instead of replacing it. Plain typing and
  Backspace work. Whether this is the produced-text key path dropping
  modifier chords or a harness artifact is unproven — a real keyboard
  on Joel's box is the verdict.
- Transient blank/partial frames can appear mid-typing in a page (the
  settings page showed blank frames twice while typing, then recovered
  fully on its own). Same damage-paint family as above.
- Mouse-wheel scrolling still unproven under Xvfb (carried from
  Round 2). wgpu adapter on Haswell, real mouse/keyboard, and drag
  latency are Joel's-box verdicts.

## Round 5 — Inspector v1 (what is proven, what is not)

Proven (sandbox, 2026-11-14, under Xvfb + software GL; gates run live
against the shipped tree; `cargo test --workspace` **49/49 green** —
frost bin 29, frost-engine 4, frost-extensions 8, frost-inspector 8):

- Inspector opens from the menu and with F12 on example.org; the
  header reads "Snapshot of https://example.org/, captured <time>, 35
  nodes — this is a snapshot, not a live view", with "Back to the page"
  and "Re-capture" links (r5-03-inspector.jpg, r5-17-f12-focused.jpg,
  r5-18-recapture.jpg). The tree is the real current DOM (no `<h1>` on
  the live page anymore); clicking a node fills the details pane (tag,
  attributes, computed style, box — r5-04-node-details.jpg), and the
  head "−" toggle collapses its children (r5-05-collapsed.jpg).
- "Back to the page" returns to example.org with the banner live
  (r5-06-back-to-page.jpg). Opening `frost://inspector` with no
  snapshot shows the honest "No page inspected yet" page, not a fake
  tree (r5-40-no-snapshot.jpg).
- File-browser Back/Forward move across folder clicks
  (r5-24-back.jpg / r5-25-back-wait.jpg, r5-30-forward2.jpg).
- Print Save works with `~/Downloads` deleted first: the dir is
  recreated and `frost-print-1794672243.png` (141 KB) written
  (r5-34-after-save.jpg). No "Not found" flash — and the header keeps
  naming the printed page after Save (the frozen-identity fix above;
  re-gated on the fixed binary, r5-45-after-save2.jpg).
- Regression: settings persist across relaunch (homepage example.com
  loads on launch, r5-39-homepage-persisted.jpg), banner on
  example.org, back/forward on normal pages (r5-41/42/43), all five
  menu entries (r5-02-menu.jpg, r5-19-menu2.jpg).

- **Approach: script injection, NOT a fork patch.** The investigation
  found the fork already exposes everything the inspector needs:
  `WebView::evaluate_javascript(script, callback)` returns
  `Result<JSValue, JavaScriptEvaluationError>` (the callback fires on
  the app thread during `Engine::spin`), and `JSValue::String` carries
  the result back. So the inspector is a DOM walker script
  (`crates/frost-inspector/src/walk.js`, injected via the engine's new
  `evaluate_javascript_string`, same mpsc-channel pattern as the
  print-preview screenshot) that serializes the live DOM to JSON
  (tag, all attributes, direct text, computed style, viewport box,
  capped at 8000 nodes / 64 deep, whitespace-only text nodes skipped).
  The planned fork patch F-002 (a constellation DOM snapshot hook) is
  NOT needed — it would touch constellation/script-thread internals
  for the same data at much higher risk. A devtools server
  (`components/devtools` exists in the fork) is a multi-round job, not
  v1. The `frost-inspector` crate owns the walker script and the JSON
  parsing (`snapshot_from_json`); the engine only ferries the string.
- **Snapshot model (the single-webview answer).** Frost has one
  webview: navigating to `frost://inspector` would replace the page
  being inspected. So the menu entry / F12 captures the DOM FIRST
  (walker runs in the current page), stores the snapshot in the
  loopback server's shared state, and only then opens
  `frost://inspector`, which renders the snapshot — labeled "Snapshot
  of <url>, captured <time>, N nodes — not a live view", with "Back to
  the page" and "Re-capture" (re-capture sends the webview back to the
  inspected page, then captures again once it loads). Opening
  `frost://inspector` directly with no snapshot shows an honest
  "No page inspected yet" page instead of a fake tree. A failed
  capture leaves the current page up and says so in the status line.
- **The tree is real.** The inspector page fetches
  `/inspector/snapshot.json` and renders the snapshot as an
  expandable/collapsible tree (JS toggles, not `<details>` — the
  toggle is Frost's own code, not engine behavior); clicking a node
  fills the details pane (tag, full attributes, text, computed style,
  box with margin/padding/border). Gate page is example.org — the live
  page has no `<h1>` anymore (verified 2026-11-14): the inspector shows
  the real current DOM, `html > head (meta, link, meta, title, style)
  > body (div#frost-sample-banner [extension-injected], style, svg, p
  ×5)`, 35 nodes.
- **Wart fixes (Round 4 leftovers, in this round).**
  - File-browser history: only the `frost://open` form is intercepted
    now (`needs_open_intercept`); the loopback `/open` form loads fine
    on its own, and re-loading it had created a duplicate history
    entry — which is why Back across folder clicks stayed on the same
    folder. Back/Forward now move across folder navigation.
  - Print save: the save is a real server route (`/print/save`) that
    creates `~/Downloads` first (`create_dir_all`) and 303-redirects
    to `/print` — which also kills the "Page: Not found" flash (the
    old app-side save ran on the UrlChanged event while the webview
    itself loaded `/print/save` and got the server's 404 page). The
    printed page's title/URL are frozen at print-open time
    (`print_page_title`/`print_page_url`): after the Save redirect
    re-renders `/print`, the live current page is `frost://print`, and
    the header used to describe the print page itself ("Page: Print
    page") instead of the printed page — caught by the harness gate,
    fixed in-round, covered by a unit test.
- **What v1 does NOT do:** no live editing, no live updates (the
  snapshot is frozen at capture time), no click-to-inspect from the
  page (that needs a docked panel inside the live page — v2), no
  console, no network panel, no shadow DOM / iframe sub-documents /
  comments (the walker skips them and says so in its header comment).
  The tree is capped (8000 nodes / 64 deep) and says "truncated" when
  the cap bites.

Not proven / known limits:

- Mouse-wheel scrolling still unproven under Xvfb (carried from
  Round 2). wgpu adapter on Haswell, real mouse/keyboard, and drag
  latency are Joel's-box verdicts.
- One cosmetic warning in the build: three `InspectorSnapshot` fields
  (`captured_at_ms`, `node_count`, `truncated`) are never read by Rust
  code — the inspector page reads them from the served JSON instead.
  The fields stay as the snapshot record; the warning is noted, not
  hidden.

## Round 6 — flashing fix (what is proven, what is not)

Joel's verdict on the 0.4.0 .deb (2026-11-14 ~07:43): "the browser
works but its drawing isnt entirely smooth theres alot of flashing
going on." Asked (2026-11-14 ~18:41) whether it flashes all the time
or mainly on load/scroll, he answered: mainly on load and scroll,
not on a still page. That pointed the fix at the load/scroll paint
path.

**Root cause, proven by code reading + a headless probe.** Round 4's
F-007 (full present) was the prime suspect — it repaints the whole
scene every frame — but the engine was exonerated first: a headless
probe (`crates/frost-engine/examples/frameprobe.rs`) scrolled a
4000 px test page 12 steps down and back up and logged every render;
all 24 renders were full frames (223–923 distinct colors, center
pixel tracking the bands), never a solid or blank frame. `render()`
returns complete pixels during scroll. The fault is in the app's
upload path, in iced 0.14 (vendored in the libcosmic checkout Frost
builds against):

1. `Handle::from_rgba` mints a unique id per call, so the app's
   one-new-handle-per-frame meant a brand-new texture every frame.
2. iced's wgpu image cache uploads a raster image synchronously
   ONLY when it is under `MAX_SYNC_SIZE` = 2 MiB; anything larger
   goes to a worker thread and `upload_raster` returns `None`.
3. The image pipeline adds no instances when `upload_raster`
   returns `None` — the widget draws NOTHING for that frame, so the
   content area shows the chrome background.
4. The worker requests a redraw when the upload lands, so the frame
   appears one draw later — but by then the app has usually replaced
   it with a newer handle during load/scroll.
5. Frost's full frame is 1280x684x4 = 3,502,080 B > 2 MiB, so EVERY
   frame during load and scroll cost one blank draw. A still page
   reuses the same handle (already uploaded), so it never flashes —
   exactly Joel's symptom. The atlas-full theory was checked and
   ruled out (`Atlas::allocate` grows a layer instead of failing).

**Fix (app-side, no fork patch — no F-number): split the frame into
horizontal strips, each under iced's 2 MiB synchronous-upload line.**
`Frame::split_strips` (`frost-engine`, `MAX_SYNC_UPLOAD_BYTES` =
1.9 MB with margin) tiles the frame exactly — same width, heights
summing to the frame height, rows in order — and the app uploads one
`Handle` per strip, drawn as a column of images at 1:1. Every strip
takes the synchronous path and is drawn the same frame it arrives:
no worker hop, no one-frame delay, no blank draw. The strip count
adapts to the viewport (1280x684 → 2 strips; 1920x984 → 4). Unit
tests prove the tiling and the under-the-limit invariant for the
sizes Frost renders at. F-007 (full present) stays — reverting it
would bring back the partial-paint loss. `frameprobe.rs` remains in
the tree as a diagnostic example (same status as `smoke.rs`).

Proven (sandbox, 2026-11-15, under Xvfb :97 + software GL; gates
re-ran live against the shipped tree):

- **Build green** (`cargo build -p frost --features ui-libcosmic`,
  38m 33s; one warning — the known Round 5 `InspectorSnapshot`
  dead-code cosmetic).
- **Tests 52/52** (`cargo test --workspace --lib --bins`: frost bin
  29, engine 7, extensions 8, inspector 8 — the engine's 3 new
  `split_strips` tests all pass). Note: the first full-workspace
  run was OOM-killed at the frameprobe example link (three parallel
  2 GB debug links); the rerun with `CARGO_BUILD_JOBS=1` and
  `--lib --bins` (skips the example links) passed cleanly.
- **Load paints fully** (`r6-09-launch-r6.jpg`): example.org with
  the banner, no blank intermediate frame.
- **Scroll paints fully at every step** (`r6-10`–`r6-14`): tall
  10-band test page, PageDown ×2, Space, plus a rapid-fire burst
  during a scroll — every sampled frame is fully painted (BAND
  1/2 → 2/3 → 3/4 → 5/6 → burst 5/6 → 6/7), no blank or dark
  content area anywhere. The strip seam is invisible (strips tile
  exactly).
- **Reload repaints fully** (`r6-15-reload1/2-r6.jpg`): back at
  scroll-top, BAND 1/2 fully painted 2 s and 10 s after the click.
- **Regression — all pass:** settings save + persist across
  relaunch (homepage set to example.com in the UI, `settings.json`
  on disk, relaunch loads example.com — `r6-17/18/19`); banner on
  example.org (`r6-20/22`) and correctly absent on example.com
  (`r6-19/23`); Back/Forward on normal pages (`r6-21/22/23`) and
  across file-browser folders (`r6-28/29/30`); all five menu
  entries open (`r6-16` menu shot; Settings `r6-17`, Print `r6-36`,
  Inspector `r6-24`, Open `r6-26`, Exit confirmation `r6-39`,
  Quit exits cleanly); Inspector snapshot via menu (`r6-24`, 33
  nodes on example.com) and via F12 (`r6-25`, fresh capture
  4:51:59 AM); file-browser folder click into workspace (`r6-27`)
  and file click opening AGENTS.md as rendered Markdown
  (`r6-35`); print Save writes
  `~/Downloads/frost-print-1794718653.png` (141,710 B) and the
  page shows the saved path with the header still "Example Domain"
  — no "Not found" flash (`r6-37`).

Not proven / known limits:

- Mouse-wheel scrolling still unproven under Xvfb (carried from
  Round 2 — the harness's `xwheel` sends XSendEvent button 4/5,
  which moves nothing through the real app). Keyboard scroll
  (PageDown/Space) is the gated path. wgpu adapter on Haswell, real
  mouse/keyboard, and drag latency are Joel's-box verdicts.
- The sandbox cannot catch a one-frame flash directly: SwiftShader
  paints at ~0.5–1 s/frame, so the worker upload completes inside
  the same paint window and burst screenshots never saw a blank
  frame even on the Round 5 binary. The fix is proven by the
  mechanism (code-reading of iced's cache + the headless engine
  probe) and by the strip invariant in unit tests, plus the visual
  gates below showing full paints at every step. The real verdict is
  Joel's box.

## Round 4 — what is proven, what is not

Proven (sandbox, 2026-11-14, under Xvfb + software GL; gates re-ran
live against the shipped tree):

- `cargo check --workspace --all-targets` green; unit tests green
  (`cargo test --workspace` — count in the delivery note).
- **F-007: keytest paints fully on load AND reload, with no jiggle.**
  The 529 B keytest page — the standing repro since Round 3 — shows
  its heading, input, and "no keys yet" div on first load and after
  Reload, with no interaction beforehand. The reload was confirmed
  genuine (a new pipeline in the engine log). The Round 2 resize
  jiggle is RETIRED: `start_repaint_jiggle`, `advance_repaint_jiggle`,
  and the `RepaintPhase` machinery are deleted; the Tick renders on
  `frame_dirty && !awaiting_load`. Real pages still paint fully.
- **F-006: the delegate sees the load start on reload.** Reloading
  example.org re-runs the sample banner (the content-script guard
  clears via `LoadStarted`, not the app-side workaround — which is
  retired).
- **F-003: print preview is real.** Opening Print from a page
  captures the page's viewport as a PNG and opens `frost://print`
  showing it, with "Save as PNG" writing the file to ~/Downloads
  (path shown on the page). The page says plainly it is a viewport
  screenshot, not paginated print layout — Servo 0.8.0 has no print
  pipeline, so paginated printing and print-to-PDF are genuinely
  unbuildable without a layout-level print mode.
- **Ctrl+A select-all works in page text inputs.** The key path now
  ignores produced text for Control/Meta chords (iced/winit reports
  Ctrl+A's text as `"\x01"`, which Servo's shortcut matcher never
  matched). Gate: type "hello", Ctrl+A, type "X" → the input holds
  `"X"`, not `"helloX"`.
- **F-008: Stop aborts a hanging load.** A load against a server that
  never responds stops on Stop, and a subsequent navigation proceeds
  cleanly (the pending change is drained, not left queued). The fork
  change alone was not enough to reach: Servo only reports
  `LoadStatus::Started` when a new Document is initialized (first
  response bytes), so a never-responding server never fires it and
  the Stop button — live only while `loading` — stayed dead. The app
  now sets `loading = true` when a navigation is requested
  (`main.rs`: address-bar submit, Back, Forward, Reload,
  `load_loopback`), so Stop is live from the request, not the first
  response. Gate (2026-11-14, build 5): hang server on
  127.0.0.1:18099 — status went "Loading http://127.0.0.1:18099/ …"
  → "Stopping…" on Stop → back to "Example Domain" (the current
  page's title; a never-started load has no Document to report
  `Stopped`, so the status settles on the page it is still showing
  — "Stopped." is the status for stopping a load that had started).
  The abort itself is proven by the follow-up: example.org then
  loaded cleanly, banner re-ran, webview not wedged.
- Regression: settings load and save; the banner survives reload;
  file browser end to end; menu all four entries visible;
  back/forward/reload work.

Not proven / known limits:

- Mouse-wheel scrolling still unproven under Xvfb (carried from
  Round 2). wgpu adapter on Haswell, real mouse/keyboard, and drag
  latency are Joel's-box verdicts.
- Full present (F-007) repaints the whole scene every frame — more
  paint work per frame under SwiftShader/llvmpipe than partial
  present was, but partial present never saved Frost anything (the
  engine reads the full framebuffer regardless).
- **File-browser history quirk (pre-existing, Round 2/3 code, not a
  Round 4 regression):** Back across file-browser folder clicks
  stays on the same folder, and Forward after such a Back goes
  nowhere — the `frost://open` intercept re-loads the URL it just
  intercepted, which disturbs the engine's history (duplicate entry
  / forward stack cleared). Back/forward on normal pages work. Use
  the "↑ Up" link to go up a folder.
- **Print page, transient cosmetic wart:** right after Save, the
  "Page:" line can briefly read "Not found / frost://print/save"
  (the save URL is the current page for a moment); it corrects
  itself to "Print page / frost://print" once the print page
  reloads. The preview and the saved path are correct throughout.
- **Print save robustness gap (known, not fixed this round):** the
  `frost://print/save` intercept writes the PNG to `~/Downloads`
  but does not `create_dir_all` first — if `~/Downloads` does not
  exist the save fails (loudly: the page shows the error, it does
  not fail silently). On a normal desktop `~/Downloads` always
  exists, so this only bites a bare sandbox/home. Fix for a future
  round: `std::fs::create_dir_all` the downloads dir before the
  write in the print/save intercept (`server.rs`).
- **Harness lesson (Round 4):** link clicks that "do nothing" were
  mis-aimed synthetic clicks — the harness was reading Y positions
  ~20 px too high off downscaled screenshots. Measure the link's
  exact pixel rows (blue-pixel bands or an enlarged crop) before
  clicking; hit-testing itself is correct (`elementFromPoint`
  matches `getBoundingClientRect`).

## Round 7 — native windows, loopback server deleted (what is proven, what is not)

Joel (2026-11-14): the hamburger menu's pages should pop up in their
own windows, drawn with libcosmic, so Frost runs no internal web
server. The `frost://` loopback server (`crates/frost/src/server.rs`,
~700 lines) is deleted, and every menu page is rebuilt as a native
libcosmic window in `crates/frost/src/app/windows.rs`. No fork patch;
the fork is untouched at `f9462dbb7`.

What replaced each `frost://` page:

- `frost://settings` → **Settings window** (640×620): theme
  Dark/Light, homepage, default search engine (DuckDuckGo default),
  "Ask before quitting" — the same options, the same JSON settings
  file, honored on relaunch. Edits go to a draft; **Save** validates
  the homepage (`is_loadable_homepage`, moved from `server.rs`),
  persists, and applies the theme. Reset restores defaults and
  applies immediately. One interaction change: the theme now applies
  on Save, not instantly per click as the old page's theme links did.
- `frost://print` → **Print window** (960×720): the same WebView
  screenshot preview (`Handle::from_bytes`) + "Save as PNG" to
  `~/Downloads` (path shown), with the same honest note that Servo
  0.8.0 has no print pipeline (no print-to-PDF/printer output). A
  failed capture opens the window on the error instead of a blank
  page. The printed page's label/title/URL are frozen at print-open
  time, so the header never flickers after a Save (the Round 4/5
  wart is gone with the page it lived on).
- `frost://open` → **Open file window** (880×640): the real file
  browser (`crates/frost/src/file_browser.rs`, rewritten to a native
  API — `list_dir`, `DirEntry`, `human_size`, `read_markdown`,
  `markdown_blocks`), listing from `$HOME`, dirs first, sizes shown,
  folder clicks navigate, file clicks open via `file://` in the main
  window. Markdown renders **natively**: pulldown-cmark events become
  styled native widgets (headings, paragraphs, code, lists, quotes,
  rules; inline emphasis is flattened to plain text, math source is
  kept as text) — nothing was silently dropped.
- `frost://inspector` → **Inspector window** (1100×720): the Round 5
  snapshot model is unchanged — menu/F12 captures the DOM first, the
  window shows the tree labeled "Snapshot of <url>, captured <time>,
  N nodes — not a live view", node click fills the details pane
  (attributes, computed style, viewport box), collapse toggles work,
  "Back to the page" + "Re-capture" (re-capture replaces the snapshot
  in place; the live page never left the main window). Same cap (8000
  nodes / 64 deep). No live editing, no live updates, no
  click-to-inspect — same limits as v1.
- `frost://exit` → **Exit dialog** (420×200): "Quit Frost?" / Cancel,
  shown only when "Ask before quitting" is on; otherwise the menu's
  Exit quits directly.
- `frost://extensions` → **Extensions section inside the Settings
  window**: the loaded extensions (name, version, enabled) and the
  failed ones (directory + error), plus the load-path note. The
  extension host v1 + sample banner on example.org keep working
  exactly as before.

Typing a `frost://` URL in the address bar now fails plainly with an
honest status message ("Frost no longer has internal pages — Settings,
Print, Open file, and Inspector open in their own windows from the ☰
menu.") and leaves the current page alone — never a dead page, never
a hang. `resolve_address` no longer treats `frost` as a loadable
scheme. Search-engine routing for bare queries and `file://` opening
are unchanged.

How multi-window works (libcosmic 1.0.0 / iced 0.14, vendored
checkout `ecc5fe4`): the `multi-window` feature is now enabled on the
libcosmic dep (it is an empty flag that turns on the two-arg
`title(id)` / `set_window_title(title, id)` and `view_window(id)`).
Popups open via `window::open(Settings)` with
`exit_on_close_request: false` (closing a popup must never quit the
browser) and `set_window_title` batched with the open task; the app
subscribes with `event::listen_with` and routes Opened / Closed /
CloseRequested per window id. Windows are single-instance per kind —
opening one that is already open just focuses it (`window::gain_focus`).
`handle_iced_event` and `on_window_resize` are gated on the main window id, so popup
coordinates never reach the page. The main window's `view()` is
unchanged; `view_window(id)` dispatches to `view_popup(id)` for any
other window, and an unknown id gets an empty container, never a
panic. The `app` module moved from an inline block in `main.rs` to
`crates/frost/src/app.rs` so `windows.rs` can live under it as a
child module (`src/app/windows.rs`) and see `Frost`'s private fields.

**Popup transparency is load-bearing, not cosmetic.** iced's wgpu
compositor picks ONE alpha mode at startup from the main window's
surface (PreMultiplied — libcosmic creates the main window
transparent, so its surface is a 32-bit visual) and applies that
same mode to every window's surface. A popup opened with iced's
default `transparent: false` gets a 24-bit visual whose surface
supports only [Opaque, Inherit], and `surface.configure` with
PreMultiplied panics fatally — the first Settings-window open in
this round crashed the whole browser that way (wgpu validation
error in `Surface::configure`, `Requested alpha mode PreMultiplied
is not in the list of supported alpha modes: [Opaque, Inherit]`).
Every popup is therefore opened with `transparent: true`, giving it
the same 32-bit visual as the main window; the popup content paints
fully opaque, so the windows look identical. This is an iced/libcosmic
platform quirk, not a Frost bug — it will bite any second window on
any X11 setup where the main window is transparent.

**Settings hit-testing is correct; an earlier "−24 px offset" was a
harness misread, not a bug.** During gating, clicks aimed at the
theme-row buttons kept missing — but the buttons render at y≈184–215
(pixel-measured) and the clicks were landing at y≈225–227, below
them. Clicks at the buttons' true rendered positions fire first try
(theme, search, toggle, Save, field, extensions — all verified). A
`scrollable` wrapper was briefly removed from the Settings view on
that false lead and then restored; the Settings content scrolls if
the extensions list ever outgrows the window.

Proven (sandbox, 2026-11-15, under Xvfb + software GL; gates re-ran
live against the shipped tree):

- `cargo build -p frost --features ui-libcosmic` green (serialized,
  `CARGO_BUILD_JOBS=1`, ~40 min); `cargo test --workspace --lib
  --bins` green — 45 tests (baseline 52/52 after Round 6; new
  tests: file-browser listing sort/hidden + markdown blocks +
  markdown round-trip + human sizes, windows capture-time format +
  print-save write + node labels + visible-rows collapse).
- Visual gates, screenshots read firsthand (paths in the delivery
  note): Settings window with all four options; Print window preview
  + Save writes a PNG; Open window lists `$HOME`, folder + file
  clicks work; Inspector window shows the real example.org tree with
  node details; Exit dialog appears and respects Ask-before-quitting
  off. Loopback-gone gate: no server listening, `frost://` fails
  plainly.
- Regression: settings save + persist across relaunch (homepage
  example.com applied on relaunch), banner on example.org and
  correctly absent on example.com, back/forward on pages and across
  file-browser folders, F12 Inspector capture, print Save,
  strip-upload scrolling (tall page scrolls fully painted, no blank
  frames).

Not proven / known limits:

- Mouse-wheel scrolling still unproven under Xvfb (carried from
  Round 2). Real mouse/keyboard, drag latency, and whether the
  flashing is gone on real hardware are Joel's-box verdicts.
- Theme applies on Save, not instantly per click (see above) — a
  minor interaction change from the old settings page.
- Two cosmetic warts: the main window's status bar keeps
  "Capturing print preview…" / "Capturing page structure…" after
  the capture finishes (the status is never cleared once the popup
  opens; it clears on the next navigation), and the Settings window
  has no in-content Close button (it closes via the OS close button
  / CloseRequested only).
- The Round 4/5 warts that lived in the loopback pages (file-browser
  history quirk, print "Page:" flicker, print-save `create_dir_all`
  gap) are gone with the pages; the print save now recreates
  `~/Downloads` if missing, and the Open window's folder navigation
  is window state, not webview history, so Back/Forward on pages are
  never disturbed by it.
- The harness lesson from Round 6 still applies: serialize links
  (`CARGO_BUILD_JOBS=1`, `--lib --bins` for the full suite) — three
  parallel ~2 GB debug links OOM-kill the run.

## Round 8 — Firefox-style hamburger dropdown (what is proven, what is not)

Joel (2026-11-15, with a screenshot of Firefox's application menu):
Frost's menu was a full-width bar that dropped down across the entire
window under the toolbar — a plain list of teal links spanning the
whole width. He wants it like Firefox's: a compact panel anchored at
the top-right, directly under the ☰ button. Menu only — nothing the
menu items do changes, no new items, no fork patch (the fork is
untouched at `f9462dbb7`), no server, no .deb.

What the panel is (`crates/frost/src/app.rs`, `MENU_ITEMS` table so
the spec and the rendering cannot drift; `crates/frost/src/settings.rs`
gains `menu_hover` / `menu_border` on `ChromeColors`):

- 300 px wide, 8 px from the right edge, 4 px below the toolbar
  (`menu_panel_geometry` from the live window width). At 800 px wide
  it still anchors top-right (panel x 492); on a window narrower
  than the panel it shrinks instead of running off-screen (x never
  negative — unit-tested down to 20 px wide).
- Dark/light panel with a visible border, floating over the page —
  it reads as a panel, not a toolbar row. It follows the current
  Frost theme (Dark/Light) the same way the Settings window does
  (verified visually in both themes).
- Same five items, same behavior (each opens its existing native
  libcosmic window): Settings, Print page…, Open file…, Inspector,
  Exit. No new items — Frost has no tabs/bookmarks/history, and none
  were invented. Separators after Settings and after Inspector.
- The keyboard shortcut is shown right-aligned only next to
  Inspector (F12); the other items have no shortcut, and none was
  invented. Hover highlight on the item under the pointer.
- Closes when an item is picked, when Escape is pressed, when the
  user clicks outside the panel, and when ☰ is clicked again. The
  ☰ button shows a pressed/active state while the menu is open.
  While the menu is open the page takes no keys, and an outside
  click does not click through to the page or toolbar underneath
  (like Firefox). The content area no longer subtracts menu height —
  the panel floats over the content instead of pushing it down
  (`content_rect` in Round 8; Rounds 2–7 subtracted `MENU_HEIGHT`).

How it is built (no built-in dropdown exists to reach for): the
libcosmic book study (2026-11-15, full report in the delivery note's
harness files) found **no dropdown/popover/context-menu widget for
desktop apps** — the only anchored floating panel the book documents
is the applet popup, which is for panel applets, not app windows, and
`menu::bar` is a top menu bar, not a hamburger dropdown. So the panel
is a custom overlay drawn in `view()` with manual dismissal, which is
the expected approach: the main column (titlebar / toolbar / content /
status) is the base layer of an iced `Stack`; when the menu is open,
two layers float over it — a transparent full-window click-catcher
(any press outside the panel closes the menu) and then the dropdown
panel itself. The Stack dispatches events top-first, so the panel's
own entries still win where they overlap the catcher. Each entry is a
`mouse_area`, not a button — the raw iced button's custom `.style()`
closure cannot be used under cosmic's Theme (its button class does
not accept an arbitrary style fn), and cosmic's own buttons are
label-only, so a row with a right-aligned shortcut cannot live inside
one; the hover highlight comes from `menu_hovered` (`MenuHover`).

Proven (sandbox, 2026-11-15, under Xvfb + software GL; gates ran
live against the shipped tree, screenshots read firsthand):

- `cargo build -p frost --features ui-libcosmic` green (serialized,
  `CARGO_BUILD_JOBS=1`, 42m 36s); `cargo test --workspace --lib
  --bins` green — **49 tests** (baseline 45/45 from Round 7 + 4 new
  menu tests: panel anchors right at 1280 and 800, panel never
  overflows narrow windows, five items with F12 only on Inspector,
  panel style follows the theme).
- Menu open: compact panel top-right under ☰, all five items,
  separators, F12 next to Inspector, dark panel with border — not
  full-width. Clicking each of the five entries opens its native
  window (Settings, Print, Open file, Inspector, Exit dialog) and
  closes the menu. Escape closes it; click outside closes it (no
  click-through to the page); clicking ☰ again toggles it closed.
  At 800 px wide the panel still anchors top-right, no overflow.
  Light theme: the panel follows the theme (light panel, dark text).
- Regression: settings save + persist across relaunch (homepage
  example.com applied on relaunch, then restored to example.org),
  banner on example.org and correctly absent on example.com,
  Back/Forward, Print Save writes a PNG, Inspector F12 captures a
  real 35-node snapshot, strip-scroll fully painted (BAND 1–2, 4,
  9–10, including a rapid PageDown burst — the Round 6 fix is
  untouched), `frost://` fails plainly, no listener.

Not proven / known limits:

- Mouse-wheel scrolling still unproven under Xvfb (carried from
  Round 2). Real mouse/keyboard, drag latency, and whether the
  Round 6 flashing fix holds on Joel's real box are his-box verdicts.
- The Round 7 warts carry over unchanged: the status bar keeps
  "Capturing print preview…" / "Capturing page structure…" after the
  capture finishes (clears on the next navigation), the Settings
  window has no in-content Close button, and the theme applies on
  Save, not instantly per click.

Harness notes from this round (for the next round's builder):

- **The "Exit doesn't open" false alarm was a harness misread, not
  a bug** — the same class of error as Round 7's "Settings hit-test
  bug". Clicks aimed at Exit at y=311 kept closing the menu without
  opening the dialog — but the Exit row renders at y≈292
  (pixel-measured from the screenshot: the panel spans y≈94–312 and
  the Exit text is at y≈290); y=311 is the panel's bottom padding,
  which takes no clicks, so the press fell through to the
  click-catcher. Clicks at the row's true position fire first try.
  Measure the row from the screenshot; never trust the displayed
  image's apparent scale.
- **A hung search can wedge later navigations.** During gating, a
  DuckDuckGo search load hung 4.5 minutes, then failed with a hyper
  `ConnectionReset` (sandbox proxy); while it was hung, later
  navigations (file://, example.com) never started — no GET in the
  log at all. A fresh instance navigates fine. That is Servo
  serializing navigations behind an in-flight load plus a sandbox
  network stall, not a Frost bug — but if a navigation stalls in a
  gate, check the log for the in-flight load before assuming the app
  is broken, and prefer direct URLs over search routing in gates.
  (Also: the gate log is block-buffered when redirected to a file —
  lines can lag minutes behind real time and only flush on exit, so
  a quiet log does not mean a quiet engine.)
- **Closing popups under bare Xvfb:** `xclose` (new harness tool,
  `harness-tmp/frost/xclose.c`) sends a bare `WM_DELETE_WINDOW`
  client message — the same message a real WM's close button sends —
  and popups close cleanly with Frost alive. `xdotool windowclose`
  is not installed in this sandbox, and its close path killed the
  app in the Round 7 packaging gate (winit GetGeometry panic on the
  destroyed window id) — do not use a kill-based close.
- **Two first-draft harness bugs, fixed before gating:** the first
  `r8-launch.sh` referenced `~/workspace/harness-tmp/frost-libs`,
  which does not exist, and passed a `--sample-banner-only` flag
  that `main.rs` never parses (it takes no arguments at all). The
  launcher now mirrors the proven `r7-launch.sh` env
  (mesa-egl / vulkan-lvp / libs-shim) with `FROST_EXTENSIONS_DIR`
  carrying the sample banner. `r7-type.py` (not raw `xinject type`)
  is the way to type shifted punctuation such as `:` — raw type
  produced `;` instead, which turned `frost://settings` into a
  search for `frost;//settings`.
