# Frost

A browser built in Rust on the Servo engine, with a libcosmic UI. Small
parts, every step committed, no giant rewrites: the extension host and the
HTML inspector are scaffolded from day one so they never need bolting on
later.

**Status: Round 2 — the window is live.** The libcosmic window launches
under Xvfb, shows the toolbar + address bar + a rendered page, navigates,
goes back/forward, reloads, stops (F-001), resizes, and opens a real file
browser from the menu. See [ROADMAP.md](ROADMAP.md) for the plan.

## Layout

```
frost/
├── Cargo.toml            # cargo workspace
├── rust-toolchain.toml   # Rust 1.99.1 (matches the Servo fork's pin)
├── FORK.md               # the Servo fork: point, patches, planned patches
├── clone-fork.sh         # recreate servo/ from scratch (shallow v0.8.0 + patches)
├── patches/              # the three proven 0.8.0 engine patches (vendored)
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
fork point is **v0.8.0** (`4e136d8e…`) plus three engine patches as fork
commits (keypress charCode, damage rects, dmabuf export). Full detail in
[FORK.md](FORK.md). To recreate it on another machine:

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
