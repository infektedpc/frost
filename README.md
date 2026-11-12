# Frost

A browser built in Rust on the Servo engine, with a libcosmic UI. Small
parts, every step committed, no giant rewrites: the extension host and the
HTML inspector are scaffolded from day one so they never need bolting on
later.

**Status: Round 1 — prove the loop.** The engine wrapper compiles against
a Servo fork, loads a page headlessly, and screenshots it. The libcosmic
window is a skeleton. See [ROADMAP.md](ROADMAP.md) for the plan.

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

# 4. The window (needs libcosmic's system libraries; see below)
cargo run -p frost --features ui-libcosmic
```

Without `--features ui-libcosmic` the `frost` binary prints a status note
and exits — the workspace always compiles even where libcosmic cannot.

### Toolchain

Rust **1.99.1**, pinned in `rust-toolchain.toml` to match the fork
(`servo/rust-toolchain.toml`). Rustup downloads it on first use if it is
not installed yet.

### libcosmic

libcosmic is **not on crates.io** — it comes from git
(`pop-os/libcosmic`, tag `v0.12`). It is an **optional** dependency
behind the `ui-libcosmic` feature, so the engine proof never depends on
it. Building the window needs the system libraries libcosmic/iced link
against (Wayland client, libxkbcommon, fontconfig, and friends). If the
build fails on a missing system library, that is a Round 1 known limit:
the engine is the gate, the window follows once the libraries are there.

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

- Anything on Joel's box (Debian 13, Mesa GL) — the smoke test and the
  window both need his run. The window was never *launched* in the
  sandbox (no display); it only *compiles*.
- `stop()`: Servo 0.8.0 has no `WebView::stop`; `FrostWebView::stop()`
  returns `EngineError::Unsupported` until fork patch F-001 lands.
  (0.8.0 did add `LoadStatus::Stopped`, surfaced as
  `EngineEvent::LoadStopped`.)
- Real pages beyond example.org; media/WebGL are off in this feature set
  (same cuts as the proven servo-de embedder: no webgl/webxr/
  media-gstreamer in the sandbox build).
- `uv` is not installed in the sandbox; Servo's script-bindings build
  script warns about it and falls back to system python. The build
  works, but installing `uv` would silence the warning.
