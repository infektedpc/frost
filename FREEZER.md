# Freezer — Frost's engine

Frost does not build against upstream Servo. It builds against Freezer,
its own engine codebase, which lives at `freezer/` inside this repo (a
**nested git repo** — it has its own `.git`, is gitignored by the frost
repo, and is never committed into it). Freezer started life as a Servo
fork so Frost could move on its own schedule; in Round 23 (2026-11-17)
it was cut loose from Servo's git entirely — the upstream remote is
gone, the history was squashed into a single commit, and Freezer is
developed on its own from here. Engine fixes land as Freezer commits.

> **Cargo gotcha (2026-11-12):** `freezer/` is also a nested *workspace*.
> Cargo resolves a path dependency nested inside a workspace directory
> against the OUTER workspace root unless the inner one is excluded — so
> without `exclude = ["freezer"]` in the frost root `Cargo.toml`,
> `components/servo` inherits `rust-version`/`edition`/`license` from the
> frost root (wrong: edition 2021 instead of 2024, no `rust-version`) and
> manifest loading fails with "`workspace.package.rust-version` was not
> defined". The `exclude` line is load-bearing; do not remove it.

## Getting Freezer

Freezer travels with the frost repo on the build machine — it is NOT in
the delivery zip and NOT in the GitHub snapshot (both exclude the nested
repo). A machine that has only the zip or a snapshot checkout cannot
build until a Freezer tree is placed at `./freezer/`. There is no
recreation script any more: `clone-fork.sh` (shallow-clone upstream
Servo v0.8.0 + apply the patches) was retired in Round 23 when the
fork was cut loose. The 15 changes themselves are still recorded in
`patches/` (see below) — re-applying them to a fresh Servo 0.8.0
checkout by hand would be the recovery path if the tree were ever lost.

## Fork point

| | |
|---|---|
| Upstream | `servo/servo`, tag **v0.8.0** |
| Commit | `4e136d8ef31b7c4748fbfac606ca463f3efd7537` ("backport: release: Disable publishing of servo-svg (#49628)") |
| Clone | `git clone --depth 1 --branch v0.8.0 https://github.com/servo/servo servo` |
| Toolchain pinned by Freezer | Rust **1.99.1** (`freezer/rust-toolchain.toml`) |
| Cut loose | Round 23 (2026-11-17): upstream remote removed, 15 patch commits squashed into Freezer commit `597be6e`. No upstream any more. |

The old fork was a shallow clone. That history is gone with the
squash — the single Freezer commit is the whole record now.

## Patches applied as fork commits

The first three were proven earlier in the servo-de embedder against
Servo 0.8.0; the fourth is Frost's own F-001 (Round 2), the fifth is
F-005 (Round 3), the next three are Round 4 (F-006, F-007, F-008),
and the ninth is F-009 (Round 16), the tenth is F-010 (Round 17),
and the eleventh is F-011 (Round 18), the twelfth is F-012 (Round 19),
and the thirteenth is F-013 (Round 20), and the fourteenth is F-014
(Round 21), and the fifteenth is F-015 (Round 22), and the sixteenth
is F-016 (Round 24) — the first change made as a direct Freezer
commit after the patch workflow was retired, so it has no patch file.
Each was verified with `git apply --check` against the fork point
before applying, one commit per patch in the old fork. In Round 23 the
fork was cut loose and those 15 commits were squashed into the single
Freezer commit `597be6e`; the table below is the record of what each
patch did. The patch files are vendored in `patches/` as the historical
record of each change.

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
| `c16cca72d` | `patches/servo-http2-header-list-size-0.8.patch` | F-010 (Round 17): raise the HTTP/2 client's max header list size from h2's 16 KiB default to 256 KiB (`.http2_max_header_list_size(256 * 1024)` in `create_http_client`, `components/net/connector.rs`). YouTube's homepage response headers decode to more than 16 KiB, so the h2 decoder flagged the block oversize and reset the stream with PROTOCOL_ERROR — youtube.com loaded (tiny 301 headers) but www.youtube.com failed with "client error (SendRequest)". Chrome allows ~256 KiB of response headers; this matches that. |
| `c4e87a381` | `patches/servo-mediasource-foundation-0.8.patch` | F-011 (Round 18): MSE foundation — new `MediaSource` / `SourceBuffer` / `SourceBufferList` WebIDL + DOM types (`components/script/dom/media/`), `URL.createObjectURL(mediaSource)` via a script-side per-global map on `GlobalScope`, attach to a `<video>` element (`closed` → `open` + `sourceopen`, detach in the load algorithm), `isTypeSupported` via `ServoMedia::can_play_type`, `appendBuffer` queues bytes with the `updatestart` → `update` → `updateend` sequence. `buffered` stays empty — nothing is decoded yet; the decode pipeline is Round 19. |
| `f638be464` | `patches/servo-mediasource-append-0.8.patch` | F-012 (Round 19): `appendBuffer` → media thread — new pure-fMP4 parser `components/script/dom/media/mp4.rs` (init segment: mvhd/tkhd/mdhd/hdlr; segments: tfhd/tfdt/trun; timestampOffset + appendWindow clipping; 7 unit tests, fork-only, not run by the frost workspace suite — the live probe gate is the guard); `SourceBuffer::drain_pending` pushes queued bytes to the player via `HTMLMediaElement::push_media_source_data` (same path as the network fetch) and computes `buffered` TimeRanges script-side from the parsed segments (the player's `buffered()` needs a known total input size, which MSE never has); `HTMLMediaElement` drives `readyState`/`duration`/`networkState` from the media thread (metadata → `HAVE_METADATA`, `loadedmetadata`, `networkState` Idle for MSE, duration propagated to the `MediaSource`); `SourceBuffer.remove()` subtracts from the bookkeeping ranges; MSE seeks are refused with a warning (no byte-range fetch exists). `add_to_active` creates the `activeSourceBuffers` list on demand — adding to a not-yet-read list previously dropped the buffer silently. Stretch result, gated live: the pushed segments decode and play through the existing ServoSrc/decodebin3 pipeline (`playing` fires, `currentTime` advances, frames render) — so Round 20 is no longer "build an appsrc pipeline" but "feed YouTube's segments through this one". |
| `2d0d4dd09` | `patches/servo-mediasource-remux-0.8.patch` | F-013 (Round 20): MSE remuxer — new `components/script/dom/media/mux.rs` (`MseMuxer`, 7 unit tests, fork-only, same status as F-012's parser tests). A real YouTube player appends one init segment per track (video `moov` track 1, audio `moov` also track 1), so the single `ServoSrc` byte stream had TWO `moov` boxes and two tracks numbered 1 — `qtdemux` could not negotiate it (`not-negotiated (-4)`, every later push `BufferPushFailed`). The muxer merges the buffers' streams before pushing: one buffer passes through byte-identical (Round 19 behavior); two or more buffers hold inits + media until every buffer has supplied its init, then emit ONE combined init (first track's `ftyp` + fresh `moov`: new `mvhd` timescale 1000, copied `trak`s with reassigned IDs, fresh `mvex` with one `trex` per track) followed by held media in presentation-time order, rewriting each `moof`'s `tfhd` track ID and renumbering `mfhd` sequence numbers. `MediaSource` owns the muxer (per-buffer creation-order keys from `AddSourceBuffer`; reset on detach); `SourceBuffer::drain_pending` feeds chunks through it and records `buffered` ranges from the ORIGINAL chunk when the muxer accepts it (held or emitted), so the merged stream's reassigned IDs never leak into accounting; refused emitted bytes wait in `push_pending` and go straight to the player on the next drain — re-feeding an original chunk would duplicate it. Limits: all buffers must append their init before any media flows; a buffer created after the combined init went out is too late (its init/media are dropped with a warning). Gated live: real YouTube DASH segments ("Me at the zoo", itag 134 + 140) play end to end through the remuxer. |
| `09b36f9a4` | `patches/servo-mediasource-logging-0.8.patch` | F-014 (Round 21): MSE append + player-event logging — permanent info-level diagnostics so a gate can prove from the log whether a page's player appends at all. `SourceBuffer::AppendBuffer` logs mime + byte count; `drain_pending` logs one summary line per drain that moved or dropped bytes (chunks/bytes pushed, chunks dropped, buffered ranges after — `accepted: false` means the remuxer dropped the chunk, never held it; held media returns `accepted: true`); `MediaSource::attach_to_media_element` and `addSourceBuffer` log attach + mime/key; `HTMLMediaElement`'s player-event handler logs `MetadataUpdated` (duration, video/audio track counts, size, format), `DurationChanged`, `NeedData`, `EnoughData`, `EndOfStream`, and `StateChanged` (transition only — the state value is not formatted), prefixed `MSE:` for elements with an attached MediaSource and `media:` for plain progressive elements. `PositionChanged` and `VideoFrameUpdated` stay silent (they fire every frame); player errors already log via `playback_error`. No behavior change — logging only. |
| `35749febb` | `patches/servo-mediasource-steer-fmp4-0.8.patch` | F-015 (Round 22): steer adaptive players to fMP4 — `MediaSource::is_type_supported` returns false for any type containing `webm` (`audio/webm`, `video/webm`) or `av01` (AV1 in any container), via a pure predicate `mse_steer_blocks` (case-insensitive); everything else still delegates to `ServoMedia::can_play_type()`. The MSE remuxer is fMP4-only, so a player that picks WebM/AV1 starves (Round 21: `pushed=0` in all drains); returning false here makes the player fall back to fMP4 DASH (itag 134 + 140, proven in Round 20). Scoped to MediaSource only — `HTMLMediaElement::canPlayType` still calls `can_play_type()` directly, so plain `<video src>` playback of WebM files is unaffected. `addSourceBuffer` inherits the steer (it calls `is_type_supported` and throws for refused types). 5 fork unit tests (predicate cases + short-circuit false cases; the passthrough true cases are proven live by the R18 probe gate, since `ServoMedia::get()` blocks until init in a unit-test process). |
| `b36a43729` | — (direct Freezer commit; patch workflow retired in Round 23, no patch file) | F-016 (Round 24): MSE event logging — diagnosis only, no behavior change. `StateChanged` now logs the `PlaybackState` value (it derives `Debug`; the R21 comment in `htmlmediaelement.rs` claiming otherwise was wrong and is corrected here), and `PositionChanged` (every 0.5 s, set by `config.set_position_update_interval(500)` in the GStreamer player) is now logged. This is what proved the post-steer stall is an event-delivery block, not a pipeline stall: the positions advance on the pipeline side but stop reaching the script thread ~1.5 s in and flush in a burst ~34 s later. |

Fork log:

```
2d0d4dd09 F-013: MSE slice 3 — remux per-track DASH streams into one player stream (Round 20)
f638be464 F-012: MSE slice 2 — appendBuffer → media thread (Round 19)
c4e87a381 F-011: MSE foundation — MediaSource/SourceBuffer/SourceBufferList (Round 18)
c16cca72d F-010: Raise HTTP/2 max header list size to 256 KiB (Round 17)
1136ffe68 F-009: Add HardwareRenderingContext (Round 16)
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

## Planned Freezer work (engine gaps Frost will close itself)

These are NOT in Freezer yet — they are the plan, so the work is scoped
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

## No upstream

Freezer has no upstream remote and no tracking branch. There is nothing
to fetch or rebase against — new engine work is committed straight into
`freezer/`. If a future Servo release is ever worth looking at, that is
a manual porting job, not a merge. Re-run the smoke test after engine
changes (`cargo run -p frozen --example smoke`).
