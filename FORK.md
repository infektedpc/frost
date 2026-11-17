# Frost's Servo fork

Frost does not build against upstream Servo. It builds against a fork that
lives at `servo/` inside this repo (a **nested git repo** — it has its own
`.git`, is gitignored by the frost repo, and is never committed into it).
The fork exists so Frost can move on its own schedule: engine fixes land as
fork commits instead of a permanent pile of `git apply` diffs.

> **Cargo gotcha (2026-11-12):** `servo/` is also a nested *workspace*.
> Cargo resolves a path dependency nested inside a workspace directory
> against the OUTER workspace root unless the inner one is excluded — so
> without `exclude = ["servo"]` in the frost root `Cargo.toml`,
> `components/servo` inherits `rust-version`/`edition`/`license` from the
> frost root (wrong: edition 2021 instead of 2024, no `rust-version`) and
> manifest loading fails with "`workspace.package.rust-version` was not
> defined". The `exclude` line is load-bearing; do not remove it.

## Fork point

| | |
|---|---|
| Upstream | `servo/servo`, tag **v0.8.0** |
| Commit | `4e136d8ef31b7c4748fbfac606ca463f3efd7537` ("backport: release: Disable publishing of servo-svg (#49628)") |
| Clone | `git clone --depth 1 --branch v0.8.0 https://github.com/servo/servo servo` |
| Toolchain pinned by the fork | Rust **1.99.1** (`servo/rust-toolchain.toml`) |

The clone is shallow. Full history later with:

```bash
git -C servo fetch --unshallow
```

## Patches applied as fork commits

The first three were proven earlier in the servo-de embedder against
Servo 0.8.0; the fourth is Frost's own F-001 (Round 2), the fifth is
F-005 (Round 3), the next three are Round 4 (F-006, F-007, F-008),
and the ninth is F-009 (Round 16).
Each was verified with `git apply --check` against this
fork point before applying, one commit per patch. The patch files are
vendored in `patches/` so the fork is reproducible from this repo alone
(`./clone-fork.sh`).

| Commit | Patch | What it does |
|---|---|---|
| `deaae1393` | `patches/servo-keypress-charcode-0.8.patch` | Derive legacy `charCode` from `KeyboardEvent`, so pages that rely on keypress charCode (terminal-style input) work. |
| `feca6e10b` | `patches/servo-damage-rects-0.8.patch` | Expose `WebView::last_damage_rects()` so the embedder can present only damaged regions instead of full frames. |
| `3200e8754` | `patches/servo-dmabuf-0.8.patch` | Export DMA-BUF handles from the rendering context so the embedder can present frames zero-copy. |
| `3d65856d5` | `patches/servo-stop-0.8.patch` | F-001: add `WebView::stop()` — `EmbedderToConstellationMessage::Stop` → constellation `handle_stop_msg` → `ScriptThreadMessage::Stop` → script-thread `handle_stop` calling `Window::Stop`. |
| `7357f4ebc` | `patches/servo-refresh-viewport-0.8.patch` | F-005: add `WebView::refresh_viewport()` — re-send the current viewport and rebuild the root display list without resizing. **Landed Round 3 but PROVED INSUFFICIENT as the full-repaint repair on its own** (finish-pass gate: 2 of 3 first loads still painted partial and stayed partial — an unchanged viewport does not force layout to reflow the pipeline's display list). Round 4 proved the partial paint was never a reflow problem (it was partial present — see F-007), so the jiggle it was propping up retires too; F-005 stays in the fork as a harmless viewport re-send. |
| `fba674cc3` | `patches/servo-reload-notify-0.8.patch` | F-006 (Round 4): `WebView::reload()` now goes through `set_load_status(LoadStatus::Started)`, so the delegate is notified and the engine reports `LoadStarted` on reload. One line, no new message type. The app-side Reload workaround is retired. |
| `f4886bf57` | `patches/servo-full-present-0.8.patch` | F-007 (Round 4): WebRender `max_partial_present_rects: 1 → 0` (full present, the upstream default) in `Painter::new`, with the mechanism documented in a comment — partial present requires the embedder to keep its own full-frame backing buffers, which Frost does not (it reads the GL back buffer wholesale and `Painter::render` clears it first). Also adds two permanent `debug!` diagnostics: display-list arrival (pipeline, epoch, first_reflow, viewport/device size, byte counts) and the root-wrapper rect. The Round 2 resize jiggle retires with this. |
| `f9462dbb7` | `patches/servo-stop-abort-pending-0.8.patch` | F-008 (Round 4): `handle_stop_msg` also drains the webview's pending session-history changes for the top-level browsing context and closes their pipelines, mirroring `handle_abort_load_url_msg` — so Stop aborts a navigation stuck at the constellation, which `Stop` alone could never reach (it only goes to the CURRENT pipeline, and the embedder cannot name the pending pipeline). |
| `1136ffe68` | `patches/servo-hardware-rendering-context-0.8.patch` | F-009 (Round 16): add `HardwareRenderingContext` — the same offscreen surfman shape as `SoftwareRenderingContext`, but `Connection::create_adapter()` (hardware) instead of `create_software_adapter()` (llvmpipe), so pages rasterize on the GPU where one exists. The embedder tries it first and falls back to software on failure. Which adapter was picked is visible in the log without extra code: WebRender logs its GL renderer at device creation (`webrender::device::gl` — `Renderer: ...`). |

Fork log:

```
f9462dbb7 F-008: Stop also aborts a pending navigation
f4886bf57 F-007: full present instead of partial present (fix blank tiny pages)
fba674cc3 F-006: notify the delegate on reload (set_load_status)
7357f4ebc fork: F-005 — add WebView::refresh_viewport() (Round 3)
3d65856d5 fork: F-001 — add WebView::stop() (Round 2)
3200e8754 fork: apply dmabuf patch (0.8)
feca6e10b fork: apply damage-rects patch (0.8)
deaae1393 fork: apply keypress charcode patch (0.8)
4e136d8ef backport: release: Disable publishing of servo-svg (#49628)
```

## Clone quirk (sandbox, 2026-11-12)

The first shallow clone reported `fatal: unable to checkout working tree`
and left an **empty index** (`git ls-files` returned 0, every file staged
as deleted). Recovered with:

```bash
git read-tree HEAD && git checkout -- .
```

`clone-fork.sh` detects this state and recovers automatically.

## Planned fork patches (Servo gaps Frost will close itself)

These are NOT in the fork yet — they are the plan, so the work is scoped
before it is needed. Each gets its own commit when built.

| ID | Gap | Plan |
|---|---|---|
| **F-001** | ~~No `WebView::stop` in Servo's public API~~ — **LANDED 2026-11-12 (Round 2).** Commit `3d65856d5`: new `EmbedderToConstellationMessage::Stop(WebViewId)` → constellation `handle_stop_msg` → new `ScriptThreadMessage::Stop(PipelineId)` → script-thread `handle_stop` calling the existing `Window::Stop` (`window.stop()` path: abort the document, report `LoadStatus::Stopped`) → new `WebView::stop()` in `components/servo`. `FrostWebView::stop()` now calls it for real; the Stop button is enabled while loading. Limitation (inherited from upstream, noted in `Window::stop_loading`): a navigation still stuck at the constellation is not aborted — the TODO there asks for an `AbortLoadUrl` message. | Add `WebView::stop()` that sends the stop message to the constellation (mirrors the script thread's `stop_loading`). Until then `FrostWebView::stop()` returns `EngineError::Unsupported` and the UI keeps Stop disabled. |
| **F-002** | ~~No DOM snapshot API — the inspector needs one and Servo ships no inspector~~ — **NOT NEEDED (Round 5, 2026-11-14).** The investigation found the fork already exposes `WebView::evaluate_javascript(script, callback)` returning `Result<JSValue, JavaScriptEvaluationError>` on the app thread during `spin_event_loop`, so a walker script serializes the live DOM to JSON and the app parses it — the same data a constellation snapshot hook would produce, at far lower risk. A devtools server (`components/devtools` exists in the fork) would be a multi-round job for no v1 benefit. No fork commit. | ~~Add a constellation request that serializes a DOM subtree (tag, attributes, computed style, box) for `frost-inspector`.~~ Closed: script injection (walk.js + `evaluate_javascript_string`). |
| **F-003** | Limited print support (no print-to-PDF path an embedder can drive). Round 4 investigation (2026-11-14): Servo 0.8.0 has NO print pipeline at all — no `WebView::print`, no PDF path, only debug `PrintTree` dumps of the layout/fragment trees; servoshell has no print either. So paginated printing and print-to-PDF are genuinely unbuildable without a layout-level print mode. **LANDED 2026-11-14 (Round 4, app-side, no fork patch needed): a real print preview from `WebView::take_screenshot` (whole-webview capture) — the engine's `request_screenshot_png` encodes it as PNG, the app captures the current page BEFORE opening `frost://print`, the server serves it as `/print/preview.png`, and "Save as PNG" writes it to ~/Downloads for printing from any viewer. The page says plainly that it is a viewport screenshot, not paginated print layout.** | Paginated print layout / print-to-PDF would need a layout-level print mode in the fork — not attempted; the screenshot preview is the honest scope. |
| **F-004** | No spellcheck API. | Later round; likely a fork-side hook into the text-input path, or a pure-Rust spellcheck in the UI layer. |
| **F-005** | ~~A load's first paint is partial and no same-size repaint repairs it~~ — **LANDED 2026-11-13 (Round 3) as `WebView::refresh_viewport()`, commit `7357f4ebc`, BUT PROVED INSUFFICIENT on its own**: re-sending an unchanged viewport rebuilds the root display list but does not force layout to reflow the pipeline's display list, so 2 of 3 first loads in the finish-pass gate stayed partial. The app keeps the Round 2 resize jiggle as the reliable repair. **Superseded by F-007 (Round 4): the partial paint was never a reflow problem — it was partial present (see F-007). F-005 stays as a harmless viewport re-send.** | F-005 as built (viewport re-send + root display-list rebuild). The real fix is F-007. |
| **F-006** | `WebView::reload()` never notifies the embedder: it sets `load_status` by direct field assignment instead of `set_load_status`, so no `LoadStarted` fires on reload (found 2026-11-13: content scripts never re-ran after a reload, and Stop stayed disabled while reloading). Worked around app-side in Round 3 (the `Reload` message clears the injection guard and marks loading). **LANDED 2026-11-14 (Round 4, commit `fba674cc3`): `reload()` now calls `self.clone().set_load_status(LoadStatus::Started)` before sending the reload message — one line, no new message type. The app-side workaround is RETIRED (Round 4 gate: the delegate sees the load start on reload and the sample banner re-runs).** | Make `reload()` go through `set_load_status(LoadStatus::Started)` so the delegate contract holds for every load. |
| **F-007** | No way to force a layout reflow at an unchanged viewport (the real full-repaint trigger; F-005 only re-sends the viewport). Standing repro (finish pass 2026-11-13): the keytest page (`file://…/keytest.html`) never paints its body on load or reload — see ARCHITECTURE.md. **ROOT CAUSE FOUND 2026-11-14 (Round 4): not a reflow problem at all.** Instrumented paint-thread logging proved the display lists arrive complete (2046 bytes, viewport 1280×684) and WebRender renders the full frame once — then only dirty regions. The damage-rects patch had set `max_partial_present_rects: 1` (partial present), whose contract requires the embedder to keep its own full-frame backing buffers; Frost reads the GL back buffer wholesale and `Painter::render` clears it first, so every frame after the first is white except the dirty regions. **LANDED 2026-11-14 (Round 4, commit `f4886bf57`): `max_partial_present_rects: 0` — full present, the upstream default. No new reflow hook was needed; the Round 2 resize jiggle is RETIRED (Round 4 gate: keytest paints fully on load AND reload with no jiggle).** | Add a fork hook that forces the constellation to reflow the pipeline at the current viewport, so the jiggle can finally retire. |
| **F-008** | A navigation stuck at the constellation cannot be aborted: F-001's `stop()` only reaches the CURRENT pipeline (`handle_stop_msg` → `ScriptThreadMessage::Stop`), and the embedder cannot name the pending pipeline to abort it (`handle_abort_load_url_msg` exists script-side only). **LANDED 2026-11-14 (Round 4, commit `f9462dbb7`): `handle_stop_msg` now also drains the webview's pending session-history changes for the top-level browsing context and closes their pipelines — Stop aborts both the current load AND any pending navigation. No new message type; the app calls the same `stop()`.** **App-side companion (Round 4, `crates/frost/src/main.rs`): the app now sets `loading = true` at the moment a navigation is requested (address-bar submit, Back, Forward, Reload, `load_loopback`) instead of waiting for `LoadStarted`. Servo only reports `LoadStatus::Started` when a new Document is initialized — which needs the first response bytes — so a server that never responds at all never fires `LoadStarted`, and without this the Stop button (live only while `loading`) was dead for exactly the hang Stop is meant to escape. Gated 2026-11-14 (see README).** | Fold the pending-change abort into `stop()` (done) rather than a new `EmbedderToConstellationMessage`. |

## Updating the fork

When a new upstream Servo release is worth tracking:

1. `git -C servo fetch --unshallow` (once) then `git -C servo fetch origin`.
2. Create a branch at the new tag, cherry-pick the fork commits on top.
3. Re-run `git apply --check` for each patch in `patches/`; fix conflicts
   in the fork, not in the patch files, and record what changed here.
4. Re-run the smoke test (`cargo run -p frost-engine --example smoke`).
