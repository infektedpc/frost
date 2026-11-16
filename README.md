# Frost

A browser built in Rust on the Servo engine, with a libcosmic UI. Small
parts, every step committed, no giant rewrites: the extension host and the
HTML inspector are scaffolded from day one so they never need bolting on
later.

**Status: Round 12 — toolbar glyphs fixed.** The toolbar icons
are proper, bigger, centered glyphs now: 20 px text in a
centered container (libcosmic's TextButton left-aligns its
label at the 14 px default), and back/forward use proper ❮/❯
chevrons with a 2 px upward nudge for their low ink. Tests
60/60 (baseline 58/58 + 2 new Round 12 tests). No fork patch;
the fork is untouched at `f9462dbb7`. See
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

### .deb 0.9.0 (Round 9, Debian 13, amd64)

`packaging/build-deb-0.9.0.sh` builds `frost-0.9.0_amd64.deb` (SHA-256
`fe62fde622b18d2962a0258ac880caa9dfa632e0a2200abe92b6e6408bc251ae`,
64,789,764 B; stripped binary 318,769,544 B) from the Round 9 tree
(HEAD `1af7e10`, same proven gated binary — no rebuild). Same contents:
`/usr/bin/frost`, desktop file, snowflake icon. Same honest
stripped-DEBUG note in the Description. Depends were re-derived from
`ldd` + `/proc` maps of the running Round 9 binary and are IDENTICAL
to the 0.7.0 list — `libvulkan1` and `libwayland-server0` both confirmed
present again (wgpu takes the Vulkan path; libwayland-server is mapped
alongside libwayland-client). The packaged binary's BuildID
(`340ee2c9…f8aa7`) is identical to the tree binary's — the .deb really
contains the gated Round 9 build. The packaged binary was gated from a
clean extraction: launch + example.org full paint + banner, all six
menu entries (Settings, Print page…, Open file…, Inspector, Command
palette… with Ctrl+K, Exit), Settings as a native window, Inspector
capturing a real 35-node snapshot, Ctrl+K opening the command palette,
`frost://settings` failing plainly with the honest status message, and
no loopback listener. Install the same way: `sudo dpkg -i
frost-0.9.0_amd64.deb` (+ `sudo apt-get install -f` only if a dep is
missing). Never installed with `dpkg -i` in the sandbox — the real
install is Joel's-box verdict.

Harness notes (2026-11-15/16): closing the Settings popup with the
`xclose` helper (bare `WM_DELETE_WINDOW`) left the app alive this time
— unlike the 0.7.0 gate, where `xdotool windowclose` on Settings killed
it with a winit `GetGeometry` panic. The Inspector "Back to the page"
hit-area wart (Round 9) showed up again in this gate: a click on the
button's top edge did not close it; `xclose` closed it cleanly with
Frost alive. Both are bare-Xvfb harness observations, not packaging
failures.

### .deb 0.11.0 (Round 11, Debian 13, amd64)

`packaging/build-deb-0.11.0.sh` builds `frost-0.11.0_amd64.deb` (SHA-256
`2fea2805b9fb8066af47c2d95b4c335006894ad4cfddd3d61f131458d0384d5e`,
64,809,912 B; stripped binary 318,772,456 B) from the Round 11 tree
(HEAD `443ebed`, same proven gated binary — no rebuild). Same contents:
`/usr/bin/frost`, desktop file, snowflake icon. Same honest
stripped-DEBUG note in the Description. Depends were re-derived from
`ldd` + `/proc` maps of the running Round 11 packaged binary and are
IDENTICAL to the 0.9.0 list — `libvulkan1` and `libwayland-server0`
both confirmed present again (wgpu takes the Vulkan path;
libwayland-server is mapped alongside libwayland-client). The packaged
binary's BuildID (`1a4d5d32…4078db`) is identical to the tree binary's
— the .deb really contains the gated Round 11 build. The packaged
binary was gated from a clean extraction: launch + example.org full
paint + banner, round toolbar icons (true circles in a toolbar zoom)
with one reload/stop slot, the flip gated against the hang server
(load → ✕ → click ✕ → load stops → ↻, page settles back to
example.org), all six menu entries (Settings, Print page…, Open
file…, Inspector, Command palette… with Ctrl+K, Exit), Settings as a
native window, Inspector capturing a real 35-node snapshot, Ctrl+K
opening the command palette, `frost://settings` failing plainly with
the honest status message, and no loopback listener. Install the same
way: `sudo dpkg -i frost-0.11.0_amd64.deb` (+ `sudo apt-get install -f`
only if a dep is missing). Never installed with `dpkg -i` in the
sandbox — the real install is Joel's-box verdict.

Harness notes (2026-11-16): the Inspector "Back to the page" hit-area
wart was not re-clicked in this gate — Inspector was closed with the
`xclose` helper (bare `WM_DELETE_WINDOW`), which closed it cleanly
with Frost alive, as in the 0.9.0 gate. The stale "Capturing page
structure…" status was visible at the bottom of the screen after the
Inspector capture — the known Round 7 wart, carried over unchanged.

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

## Round 9 — Command palette (what is proven, what is not)

A VS Code-style command palette, opened with Ctrl+K (verified
unused in Frost — only F12 and Escape were browser shortcuts) or
from a "Command palette…" entry in the ☰ menu (Ctrl+K shown
right-aligned, so it is discoverable). Palette only — no new
features, no fork patch (the fork is untouched at `f9462dbb7`), no
server, no .deb.

What the palette is (`crates/frost/src/app.rs`, reusing the Round 8
overlay pattern — `Stack` + dimmed full-window click-catcher +
panel; `crates/frost/src/settings.rs` needed no new colors, the
menu's panel/hover styles fit):

- Centered panel, 560 px wide, 100 px from the top
  (`palette_panel_geometry` from the live window width). A window
  narrower than the panel shrinks it instead of running off-screen
  (x never negative — unit-tested down to 20 px wide). The page
  behind is dimmed (black, alpha 0.4) and blocked: clicks outside
  close the palette without clicking through, and the wheel does
  not scroll the page behind it.
- Text input on top ("Type a command", focused on open), filtered
  command list below. Fuzzy matching is a subsequence match,
  case-insensitive (`fuzzy_match`): "set" leaves Open Settings and
  Open Inspector; "k" matches nothing and the panel shows "No
  matching commands". Arrow Up/Down moves the highlight (wraps at
  both ends — `move_selection`), Enter runs the highlighted
  command, Escape closes, Ctrl+K toggles, clicking a row runs it.
- Commands are ONLY real, working actions wired to existing
  messages — nothing is shown that does nothing. The list is
  `palette_commands_for(can_back, can_forward, loading)`, with
  unavailable actions hidden rather than shown dead: Go back /
  Go forward only with history, Stop loading only while loading.
  Always present: Reload, Open Settings, Print page…, Open file…,
  Open Inspector, Focus address bar, Quit Frost. (Go back / Go
  forward were not gated live this round — the gate instance had
  no history to go back to; the hiding logic is unit-tested.)
- Follows the Dark/Light theme like the Round 8 menu (same panel
  and hover styles; the gate ran in Dark — Light follows the same
  code path and was not re-screenshotted this round).
- The palette must not break the menu: opening the palette closes
  the menu and vice versa; Escape closes whichever is open,
  palette first if both somehow open. The ☰ menu now has six
  items (Command palette… sits after Inspector, before Exit, with
  separators after Settings and after Command palette…).

**The gate found a real bug, and it is fixed in this round.**
Ctrl+K with the palette input focused did NOT toggle the palette
closed — three presses (fast chord x2, slow chord x1) all left it
open, while Escape and click-outside closed it fine. Root cause,
proven from the vendored libcosmic source: libcosmic's text_input
captures EVERY KeyPressed while it is focused (its handler ends in
an unconditional `shell.capture_event()`, except for Escape and
Up/Down, which return early without capturing) — so a Ctrl+K from
the palette input never reaches the Ignored-path toggle. (The
pre-fix comments blamed iced's text_input for swallowing Escape;
the app actually uses libcosmic's text_input, which lets Escape
through uncaptured — the comments are corrected in this round.)
Fix: `handle_iced_event_captured` — which the subscription feeds
with Captured keyboard events — now toggles the palette on a
Captured Ctrl+K exactly as the KeyPressed handler does for an
Ignored one. An event is Captured or Ignored, never both, so it
fires once. The same branch makes Ctrl+K work from the address
bar, which captures it the same way.

Proven (sandbox, 2026-11-15, under Xvfb + software GL; gates ran
live against the shipped tree, screenshots read firsthand):

- `cargo build -p frost --features ui-libcosmic` green (serialized,
  `CARGO_BUILD_JOBS=1`, 57m 49s for the fixed build — the first
  build was 26m 05s; the link alone runs 40+ min on this box);
  `cargo test --workspace --lib --bins` green — **53 tests**
  (baseline 49/49 from Round 8 + 4 new palette tests: fuzzy_match,
  palette_commands_hide_unavailable_actions,
  move_selection_wraps_both_ends,
  palette_panel_is_centered_and_never_overflows).
- Palette open (Ctrl+K): centered panel with input + 7 commands
  (Reload highlighted first; no Go back / Go forward with no
  history, no Stop loading with nothing loading), page dimmed
  behind it. Typing "set" narrows it to Open Settings + Open
  Inspector; ArrowDown moves the highlight visibly to Open
  Inspector; Enter runs it — the Inspector window opens with a
  real 35-node example.org snapshot. Typing "k" shows "No
  matching commands".
- Escape closes the palette (the Captured-path Escape works with
  the input focused); click outside closes it (no click-through);
  Ctrl+K toggles it closed — verified on the fixed build after
  the bug above (open shot + closed shot).
- Menu entry "Command palette…" (Ctrl+K right-aligned) opens the
  palette and closes the menu; opening the palette with Ctrl+K
  while the menu is open closes the menu. Clicking the "Open
  Settings" row opens the native Settings window and closes the
  palette. Typing "addr" + Enter focuses the address bar with the
  URL selected; typing a URL + Enter navigates (example.org loads
  with the banner live).
- Regression: settings save writes the JSON and persists across
  relaunch (homepage example.com applied on relaunch, then
  restored to example.org/Dark), banner on example.org and
  correctly absent on example.com, Back/Forward (settled shots —
  the address bar updates ~10–15 s before the content repaints
  under SwiftShader, the known Round 6 observation), Print Save
  writes a PNG (`frost-print-1794783773.png`, 183,217 B), Inspector
  F12 captures a real 33-node example.com snapshot, strip-scroll
  fully painted (BAND 1–2, 4, 8 including a rapid Space burst —
  the Round 6 fix is untouched), `frost://settings` fails plainly
  with the honest status message, `ss` shows no Frost listener,
  the Round 8 menu still opens and closes on Escape.

Not proven / known limits:

- Mouse-wheel scrolling still unproven under Xvfb (carried from
  Round 2). Real mouse/keyboard, drag latency, and whether the
  Round 6 flashing fix holds on Joel's real box are his-box verdicts.
- PageDown (keycode 105) did not scroll the tall test page in this
  gate — Space scrolled it fine, so the page scrolls and paints;
  the PageDown non-response is a harness keycode/focus quirk, not
  a proven app bug, but it was not chased to a root cause.
- The Round 7 warts carry over unchanged: the status bar keeps
  "Capturing print preview…" / "Capturing page structure…" after
  the capture finishes (clears on the next navigation), the Settings
  window has no in-content Close button, and the theme applies on
  Save, not instantly per click.
- **Inspector "Back to the page" hit-test quirk (new wart, measured
  this round):** the button's hit area sits ~15 px above its visual
  rect — clicks at the button's visual center (140,165) did nothing
  four times, while a click at its top edge (140,150) closed the
  window. The button works; aim at its top edge under the harness.
  The OS close (WM_DELETE_WINDOW via `xclose`) closes it cleanly.
  This is a Round 7 window, untouched since — not a Round 9
  regression — but it was not measured before this round.
- The Inspector's "Re-capture" button was not re-clicked this
  round (it shares the button mechanism with Back, which was
  proven to work at its top edge).

Harness notes from this round (for the next round's builder):

- **Runtime restarts kill the build and Xvfb.** The first full
  build was killed mid-link by a runtime restart (~7 min in) and
  Xvfb :97 died with it — the rebuild (57m 49s) and an Xvfb
  restart (`Xvfb :97 -screen 0 1280x800x24`) were needed before
  gating. If a build session reports no confirmed result, check
  the binary timestamp and whether cargo is still alive before
  re-running — and check Xvfb before launching.
- **F12 is keycode 96, not 123.** Sending 123 opened nothing; 96
  opens the Inspector. (F1–F12 are 67–78, then F11=95, F12=96 on
  evdev.)
- **Back/Forward settle slowly under SwiftShader.** The address bar
  updates immediately but the content repaints ~15–35 s later; a
  shot taken at 8 s shows the new URL with the old page (and the
  banner still visible). Wait 30+ s for the settled shot before
  concluding the navigation is wrong; the gate log's GET lines
  confirm the fetch happened.
- **Ctrl+K chords need direct xinject keydown/key/keyup**
  (`r9-chord.sh`: Control=37, k=45) — `r7-key.sh` does single keys
  only. The toggle bug above was real, not a chord artifact: it
  reproduced with fast chords and with a slow separated chord.

## Round 10 — WM titlebar with GNOME fallback (what is proven, what is not)

The window manager draws the titlebar by default now. Frost's
own 40 px titlebar row — title text plus the minimize /
maximize / close buttons, the drag-to-move press handling, the
double-press to maximize — is gone on KDE and every other desktop
whose window manager draws decorations, and the toolbar is the
top row of the window. On GNOME (Mutter refuses server-side
decorations, so the WM bar there would mean no titlebar at all)
Frost keeps drawing its own titlebar, exactly as in Rounds 1–9.
The choice is made once at startup from the desktop environment,
before `cosmic::app::run` — no setting, no toggle, no runtime
switch. No fork patch; the fork is untouched at `f9462dbb7`.

What changed (`crates/frost/src/app.rs`):

- `use_client_decorations(desktop: &str) -> bool` — the rule,
  pure and unit-tested: `true` (Frost draws its own bar) only
  when the desktop string contains "gnome" (case-insensitive,
  so colon-separated values like "ubuntu:GNOME" match too);
  `false` (the WM draws it) for KDE, Plasma, XFCE, COSMIC,
  Hyprland, Sway, and unset/unknown. `detected_desktop()`
  resolves the string: `XDG_CURRENT_DESKTOP` first,
  `XDG_SESSION_DESKTOP` as the fallback, empty when neither is
  set (which means the WM bar).
- `run()` passes the flag both ways at once: libcosmic's
  `Settings.client_decorations(...)` (the vendored setting —
  `src/app/settings.rs`, `derive_setters::Setters` generates the
  setter; the non-Wayland path in `src/app/mod.rs` turns it
  into `iced.window.decorations = !client_decorations`, the
  Wayland path into `SctkWindowSettings.client_decorations`) and
  the app's `Flags`, which changed from `()` to `bool` — so the
  view agrees with the window by construction, never by a second
  env read.
- The `Frost` struct carries `client_decorations: bool`, and the
  geometry reads it through `titlebar_height()` (40 px in the
  fallback, 0 in WM mode): `content_rect()`, the initial
  webview height in `init()`, and the menu panel's top padding
  (`titlebar_height() + TOOLBAR_HEIGHT + MENU_PANEL_GAP` — the
  panel still sits 4 px under the toolbar, which is now the top
  row). The view builds the titlebar row and its buttons as
  before, but only pushes the row into the main column in
  fallback mode; the TitlebarMinimize / TitlebarMaximize /
  TitlebarClose / TitlebarDrag / TitlebarDoubleClick handlers
  stay live behind the flag. `titlebar_text()` is unchanged and
  still feeds `set_window_title` on every Tick, so the OS
  window title stays correct in both modes — the WM shows it in
  WM mode, Frost's own bar in fallback mode.
- The built-in libcosmic headerbar stays hidden in both modes
  (`core.window.show_headerbar = false`, as before): it renders
  the title but no window-control buttons on this setup, so it
  would only be an empty strip.

Popups (Round 7 windows — Settings, Print, Open file, Inspector,
Exit) already asked the window manager for titlebars: their
`window::open` calls use `window::Settings` with
`..Default::default()`, and iced's default `decorations` is
`true`. No popup code changed. `transparent: true` stays, and it
does not conflict with decorations: transparency is about the
surface's alpha mode (the Round 7 finding — iced picks one alpha
mode from the main window's surface, and an opaque popup's 24-bit
visual panics on configure), while decorations are about the WM
frame around that surface; the two are orthogonal. The WM close
button on the popups now also fixes the Round 7 wart that the
Settings window had no in-content Close button.

Not proven / known limits:

- Bare Xvfb has NO window manager, so the WM-drawn titlebar
  itself — its look, and real min/max/close buttons — CANNOT be
  verified in the sandbox: the window simply shows no titlebar
  here, in either mode. What the sandbox proves is the app side
  (the client row is gone/present as the mode says, the toolbar
  is the top row in WM mode, the OS window title is set, the
  geometry is right). Joel's KDE box is the real verdict for the
  actual titlebar.
- GNOME is a known limitation by design, not a bug to chase in
  this round: the fallback exists because Mutter refuses
  server-side decorations. The env check matters most on X11
  (Frost's current path); on Wayland, sctk also negotiates
  xdg-decoration with the compositor.
- The Round 7 warts carry over except one: the Settings window
  now has a WM close button, so its missing in-content Close
  button no longer strands it. The stale "Capturing…" status and
  theme-applies-on-Save are unchanged, and the Inspector "Back
  to the page" hit area still sits ~15 px above its visual rect
  (Round 9 wart, unchanged).

Proven in the gate (2026-11-16, Xvfb :97, the binary built Nov 16
02:21 UTC):

- Build green in 42m 53s serialized; tests 56/56 (frost bin 33,
  engine 7, extensions 8, inspector 8).
- WM mode (`XDG_CURRENT_DESKTOP=KDE`): `r10-01-launch-wm.jpg` —
  the toolbar is the top row, no client titlebar row, example.org
  fully painted with the banner live; the OS window title is
  "Frost — Example Domain" (proven by the harness focus tool
  reading the window). `r10-02-menu-wm.jpg` — the six-item menu
  panel anchors 4 px under the toolbar (top y≈52). Settings
  opens (`r10-03`), saves the homepage to example.com with
  "Settings saved." and matching JSON (`r10-04/05`), and closes
  cleanly via the OS close (`r10-06`, Frost alive). Ctrl+K
  palette (`r10-07`), F12 Inspector with a real 35-node snapshot
  (`r10-08`), Print with a real preview and Save as PNG writing
  `frost-print-1794796026.png` (212,613 B) (`r10-09`), Open file
  with the real $HOME listing (`r10-10`), Quit Frost? dialog
  with Cancel keeping Frost alive (`r10-11`). A 10-band tall page
  paints fully at the top (`r10-12`) and at BAND 4 after Space
  scrolling (`r10-13`); Back/Forward settle on the right pages
  (`r10-14/15`); `frost://settings` fails plainly and leaves the
  page alone (`r10-16`); `ss` shows no Frost listener (only
  127.0.0.1:9999, the ps_server watchdog). Settings persist across
  a relaunch — the new launch opens example.com (`r10-17`).
- GNOME fallback (`XDG_CURRENT_DESKTOP=GNOME`):
  `r10-18-launch-gnome.jpg` — Frost's own 40 px titlebar row is
  back ("Frost — Example Domain" plus min/max/close), toolbar
  below it; `r10-20-menu-gnome.jpg` — the menu panel anchors
  under the toolbar in fallback mode too (top y≈92).
- Session-desktop fallback (`XDG_CURRENT_DESKTOP` unset,
  `XDG_SESSION_DESKTOP=GNOME`): `r10-19-launch-session-desktop.jpg`
  — the own titlebar row is back, so the secondary check works.
- Popup transparency: `transparent: true` + server-side
  decorations do NOT conflict — all five popups opened, rendered,
  and closed (Settings, Inspector, and Open file via the OS close;
  Print via its in-content Close), with no configure panic.
- A VM/runtime restart at ~02:23 UTC killed Xvfb :97 and the
  running Frost mid-gate (after the first three WM screenshots);
  the gates were re-run from a fresh launch on the same binary —
  no code change, no rebuild.

## Round 11 — merged reload/stop button + round toolbar icons (what is proven, what is not)

Two toolbar changes Joel asked for, done together because they
touch the same buttons. No fork patch; the fork is untouched at
`f9462dbb7`. One file changed, `crates/frost/src/app.rs` (+78/−18).

What changed:

- Merged reload/stop: the toolbar had separate reload (⟳) and stop
  (✕) buttons. There is one button now. It shows ⟳ while idle and
  ✕ while a page is loading; clicking it reloads when idle and
  stops the load when loading. It flips back to ⟳ in both end
  states — load finishes normally, and load stopped by the user.
  The wiring reuses the `loading` flag Frost already tracks
  (set true on navigation request, false on LoadFinished /
  LoadStopped, Round 4 / F-008). `reload_stop_button(loading)`
  is a pure mapping (idle → ("⟳", Reload), loading → ("✕",
  Stop)); the command palette's Reload / Stop loading entries
  already hide/show by the same state and are untouched, and
  keyboard reload is untouched (F5 is forwarded to the page).
- Round icons: the toolbar buttons were pill-shaped (wider than
  tall). They are circles now — square 32×32 bounds
  (`TOOLBAR_ICON_SIZE: f32 = 32.0`, the toolbar inner height)
  with zero padding, via a `round_icon_button()` helper used by
  back, forward, the merged reload/stop button, and the ☰ menu
  button. The address bar stays as-is (text field); the
  GNOME-fallback titlebar's min/max/close buttons were already
  circular and are untouched. The pill shape came only from
  wider-than-tall bounds (glyph + 2×space_s=16 horizontal
  padding on a 32 px tall button): the theme's button corner
  radius `radius_xl = [160.0; 4]` is already far past half the
  side, so square bounds render as a circle. No custom style
  was needed (`Button::Custom` exists but was not used). Hit
  areas are the same visual size as before, just square.

Tests 58/58 (baseline 56/56 + 2 new: the reload/stop mapping in
both states, and the icon size filling the toolbar inner
height). Build green in 43m 22s serialized.

Gates, all on the final binary (Xvfb :97, 1280×800):

- Launch (`r11-01-launch.jpg`): example.org fully painted,
  banner live, toolbar is the top row (WM mode). Roundness
  measured from the shot pixels, not eyeballed: back x 9..40,
  forward 49..80, reload/stop 89..120, menu 1232..1263 — all
  exactly 32 px wide, 32 px tall (y 9..40); all four buttons'
  centers/tops/lefts are button background and all four corners
  are toolbar background — true circles. (One naive scan was
  contaminated by a 1 px top window-border line and by sampling
  the menu button as the toolbar background; corrected by
  sampling the background from the row's 8 px left padding.)
- Flip, hanging load (`r11-02/03/03b`): navigate to the
  never-responding server on 127.0.0.1:18099 → button shows ✕
  → click ✕ → load stops → button flips back to ⟳ and the page
  settles (slow settle under SwiftShader, ~13 s, same as the
  Round 4 F-008 pattern).
- Flip, normal load (`r11-04/05`): example.org → ✕ while
  loading → ⟳ + banner + "Example Domain" on finish.
- Back/Forward: clean sequence on a fresh instance —
  example.org → bands file page → palette shows "Go back"
  (`r11-15`) → palette "Go back" runs, address bar back on
  example.org (`r11-16`) → toolbar Forward click, address bar
  back on the bands page (`r11-17`). Post-stop sequence on the
  same instance — hang → stop → example.com → palette shows
  "Go back" (`r11-20`) → Back click traverses to the bands
  page (`r11-21`). The toolbar buttons work; the palette
  agrees with them.
- Menu (`r11-22`): all six entries (Settings, Print page…,
  Open file…, Inspector F12, Command palette… Ctrl+K, Exit),
  separators, ☰ pressed state; Escape closes it (`r11-23`).
- Ctrl+K palette opens and runs a command: "Go back" via Enter
  (`r11-15/16`).
- Settings save + persist (`r11-28/29/30`): homepage set to
  https://example.com, "Settings saved.", matching JSON on
  disk; relaunch opens example.com with the banner correctly
  absent on .com.
- Banner on .org / absent on .com (launch vs relaunch shots).
- Inspector F12 (`r11-25`): real 26-node snapshot of the bands
  page, label/tree/Back/Re-capture.
- Strip-scroll (`r11-24`): Space scrolls the 10-band page fully
  painted (BAND 2/3/4, no blank frame).
- `frost://settings` fails plainly (`r11-27`): the honest
  status message, page left alone. `ss` shows no Frost
  listener (only 127.0.0.1:18099, the hang server, and
  127.0.0.1:9999, the ps_server watchdog).
- GNOME fallback (`r11-31`): `XDG_CURRENT_DESKTOP=GNOME` —
  Frost's own 40 px titlebar ("Frost — Example Domain" plus
  min/max/close) is back, with the new round-icon toolbar
  under it.

The Back anomaly, investigated, not a Round 11 regression: the
first gate instance showed Back doing nothing after the
sequence example.org → hang → stop → example.org →
example.com → example.org → bands page — the palette showed
no "Go back" (can_back false) for every navigation after the
stop, and two Back clicks did nothing. The Round 11 diff
touches nothing in the history/navigation path (one file,
toolbar view + helpers + tests), and the same binary passes
Back/Forward on the clean sequence and on the hang → stop →
navigate sequence (above), so the round did not break
history. The first instance's sequence additionally included
a same-URL re-navigation (example.org right after the stop,
when it was already current) and a Back click fired DURING
the example.com load — one of those interleavings, or their
combination, left that session's history state stuck; the
minimal hang → stop → navigate sequence does NOT reproduce
it. It is a pre-existing engine-side quirk (the stop path is
Round 4's F-008 fork code plus Servo 0.8's constellation
session history, untouched since Round 4), recorded here,
not fixed in this round. Two misreads owned along the way:
the "dim" back button in the downscaled screenshots was not
dim at all — pixel measurement shows the button renders
identically in every shot, so the palette (not the button
shade) is the reliable can_back read; and the gate log being
quiet proves nothing, because it is block-buffered when
redirected.

Carried over, unchanged: the stale "Capturing…" status after
a capture (Round 7); the Inspector "Back to the page" hit
area sitting ~15 px above its visual rect — the OS close
works (Round 9); Settings has no in-content Close button
(OS close works, and in WM mode the popup has a real WM
close button); theme applies on Save, not per click. Not
verified in the sandbox, as before: real mouse/keyboard,
mouse-wheel scroll over the page, drag latency, the Round 6
flashing fix on real hardware, and the WM-drawn titlebar
itself (look, real min/max/close) — bare Xvfb has no window
manager, so Joel's KDE box is the verdict for those.

## Round 12 — toolbar glyphs bigger + centered (what is proven, what is not)

Joel installed the 0.11.0 .deb on his KDE box: everything
looked good except the icons. The toolbar buttons were
round (Round 11 geometry worked), but the glyphs were small
(~10 px) and sat left-of-center in the 32×32 circles, with
empty dark space to the right of each glyph. The defect
reproduced in the sandbox (`r11-01-toolbar-zoom.jpg` shows
the same small left-shifted glyphs), so the fix was iterated
and verified here. No fork patch; the fork is untouched at
`f9462dbb7`. One file changed, `crates/frost/src/app.rs`.

Root cause, proven from the vendored libcosmic source
(pinned rev `ecc5fe4` — the `9c62f19` checkout in the same
directory is stale and must not be read): libcosmic's
`button::standard` / `button::suggested` build a `TextButton`
whose label lives in an inner row that is only
`.align_y(Alignment::Center)` — horizontal layout is
left-aligned by default — at the default 14 px font / 20 px
line height, with no horizontal-alignment control exposed
on the Builder. Round 11 set 32×32 bounds + zero padding,
so the glyph sat at the button's left edge no matter the
bounds. The fix does not fight `TextButton`: `round_icon_button()`
now builds on libcosmic's `button::custom` — the same
`Button` widget and the same theme classes as
`button::standard`, so hover/pressed/disabled look identical
— with a 32×32 `container` holding `text(label)` at an
explicit `TOOLBAR_GLYPH_SIZE: f32 = 20.0` and
`.align_x(Alignment::Center)` + `.align_y(Alignment::Center)`,
outer button 32×32 + zero padding + `.class(class)`.

What changed:

- Glyph size + centering, all four toolbar buttons: 20 px
  text, explicitly centered both ways. Measured from pixels
  (see gates): every glyph's bounding-box center is within
  0.5 px of its button's center.
- Back/forward glyph swap: the single guillemets ‹/› are
  quotation marks, not chevrons — their ink is only ~6×9 px
  at 20 px and sits ~3 px low in the em (measured: 4×8 px
  ink, dy +3.0 on the Round 11 build). Back/forward now use
  the proper heavy chevrons ❮/❯ (U+276E/U+276F), ~10×20 px
  ink at the same 20 px size, which read as real toolbar
  icons next to ↻/☰/✕. Same glyph family as before
  (chevrons), just the typographically correct ones.
- Chevron nudge: ❮/❯ ink still sits ~2 px low next to
  ↻/☰/✕ at the same size (font metrics: their ink center
  is ~2 px below the reload/menu glyphs'), so the chevron
  buttons nudge their glyphs up by
  `TOOLBAR_CHEVRON_NUDGE_UP: f32 = 2.0`, applied as bottom
  padding on the glyph container (padding shrinks the
  centered area from the bottom, so the glyph sits higher by
  half the padding). Reload/stop and ☰ use a 0.0 nudge.
- Same 32×32 round geometry and the same hit areas; the ☰
  menu button gets the fix automatically (it shares
  `round_icon_button`). The GNOME-fallback titlebar's
  min/max/close buttons were measured in the GNOME gate and
  are fine — their glyphs render centered — so they are
  untouched.

Tests 60/60 (baseline 58/58 + 2 new: the glyph size is
bigger than the old 14 px TextButton default and fits the
32 px circle, and the chevron nudge is small and positive).
Build green in 36m 15s serialized. (The first build attempt
was SIGTERM'd mid-link by a runtime restart after ~34 min;
cargo resumed from cached artifacts. The restart also
killed Xvfb :97 and the hang server; both were restarted
before gating.)

Gates, all on the final binary (Xvfb :97, 1280×800), with a
new pixel measurement script (`measure12.py` in the harness
dir — it finds the four 32×32 button circles from pixels
instead of assuming positions, then measures each glyph's
bright-pixel bbox against its button's true center):

- Launch (`r12-01-launch.jpg`, `r12-01-toolbar-zoom.png`):
  example.org fully painted, banner live. Measured offsets
  (button → glyph): back 10×20 px, dx +0.0, dy +0.0;
  forward 10×20 px, dx +0.0, dy +0.0; reload/stop 15×12 px,
  dx −0.5, dy +0.0; menu 14×15 px, dx +0.0, dy −0.5. The
  chevrons went from 4×8 px at dy +3.0 (Round 11 build,
  measured with the same script) to 10×20 px dead-center.
  The zoom shows big bold chevrons, centered, matching ↻
  in visual weight.
- Flip, hanging load (`r12-02-loading.jpg`,
  `r12-03/03b/03c-stopped*.jpg`): navigate to the
  never-responding server on 127.0.0.1:18099 → button shows
  ✕ (11×11 px, dx −0.5, dy −0.5, centered) → click ✕ → load
  stops → button flips back to ⟳ (15×12 px, dx −0.5,
  dy +0.0), status bar back to "Example Domain". The flip
  took three clicks to observe, not one: the stop's UI
  update lagged ~15 s+ under SwiftShader, so the second
  click landed on the (already flipped, not yet repainted)
  ↻ button and reloaded the hang URL; the third click
  stopped that reload. The flip itself works; the lesson is
  to wait 30 s after a stop click before screenshotting.
- Back/Forward, clean sequence on a fresh instance
  (`r12-11/12/13-clean-*.jpg`): example.org → example.com
  (banner correctly absent on .com) → Back click →
  example.org with the banner back → Forward click →
  example.com, no banner. The palette agrees: it lists
  "Go back" on the clean instance (`r12-15-palette.jpg`).
- Menu (`r12-14-menu.jpg`): all six entries (Settings,
  Print page…, Open file…, Inspector F12, Command palette…
  Ctrl+K, Exit), separators, ☰ pressed state; Escape closes.
- Ctrl+K palette (`r12-15-palette.jpg`, and
  `r12-07-palette-poisoned.jpg` on the first instance):
  opens, lists the available commands, Enter runs; on the
  poisoned instance it correctly hid "Go back" / "Go
  forward" (see the anomaly note below).
- Settings save + persist (`r12-16-settings.jpg`,
  `r12-17-relaunch-persist.jpg`): homepage set to
  https://example.com, matching JSON on disk; relaunch opens
  example.com with the banner correctly absent.
- Banner on .org / absent on .com (launch vs .com shots).
- Inspector F12 (`r12-18-inspector.jpg`): real 33-node
  snapshot of example.com, label/tree/Back/Re-capture;
  closed cleanly via the OS close button, Frost alive.
- Strip-scroll (`r12-21/23-bands-*.jpg`): the 10-band page
  fully painted at the top (BAND 1/2), Space scrolls to
  BAND 2/3/4 fully painted, no blank frame. (The first
  Space press did nothing because keyboard focus was still
  on the address bar after the navigation — clicking the
  page first moves focus into the webview. Harness quirk,
  same class as Round 9's PageDown note.)
- `frost://settings` fails plainly (`r12-24-frosturl.jpg`):
  the honest status message, page left alone (bands page,
  fully painted). `ss` shows no Frost listener (only
  127.0.0.1:18099, the hang server, and 127.0.0.1:9999,
  the ps_server watchdog).
- GNOME fallback (`r12-25-gnome.jpg`,
  `r12-25-titlebar-zoom.png`): `XDG_CURRENT_DESKTOP=GNOME`
  — Frost's own 40 px titlebar ("Frost — Example Domain"
  plus min/max/close) is back, with the new toolbar under
  it; measured with `--titlebar-offset 40`: back 10×20 px
  dx +0.0 dy +0.0, forward 10×20 px dx +0.0 dy +0.0,
  reload/stop 15×12 px dx −0.5 dy +0.0, menu 14×15 px
  dx +0.0 dy −0.5. The titlebar's own −/▢/✕ glyphs render
  centered in their circles, so they needed no fix.

The Back anomaly, reproduced, still not a Round 12
regression: the first gate instance showed the same stuck
history as Round 11's first instance — after
example.org → hang → stop → reload (same URL) → stop →
example.com, the palette showed no "Go back" / "Go
forward" (can_back/can_forward false) for every navigation
after the stop, the .com navigation never committed (the
banner stayed visible on .com), and Back clicks did
nothing. The Round 12 diff touches nothing in the
history/navigation path (one file, toolbar view + helpers
+ tests), and the same binary passes Back/Forward on the
clean sequence (above), so the round did not break history.
It is the same pre-existing engine-side quirk (Round 4
F-008 abort + Servo 0.8 constellation session history),
recorded here, not fixed in this round. Two corrections
from this round's measurements: (1) the enabled/disabled
shade DOES differ measurably — disabled buttons render
(58,57,65), enabled (72,71,78) — so the button shade is a
usable can_back read after all (Round 11's "does not
visibly differ" was wrong at pixel level); (2) the gate
log being quiet proves nothing (block-buffered when
redirected). Harness notes: `xclose` needs `DISPLAY=:97`
in its environment (the first two calls without it failed
with "cannot open display"); `r12-launch.sh` does not set
`XDG_CURRENT_DESKTOP`, so the first GNOME attempt ran in
WM mode — the GNOME script adds the export.

Carried over, unchanged: the stale "Capturing…" status after
a capture (Round 7); the Inspector "Back to the page" hit
area sitting ~15 px above its visual rect — the OS close
works (Round 9); Settings has no in-content Close button
(OS close works, and in WM mode the popup has a real WM
close button); theme applies on Save, not per click. Not
verified in the sandbox, as before: real mouse/keyboard,
mouse-wheel scroll over the page, drag latency, the Round 6
flashing fix on real hardware, and the WM-drawn titlebar
itself (look, real min/max/close) — bare Xvfb has no window
manager, so Joel's KDE box is the verdict for those.
