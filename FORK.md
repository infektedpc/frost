# Frost's Servo fork

Frost does not build against upstream Servo. It builds against a fork that
lives at `servo/` inside this repo (a **nested git repo** — it has its own
`.git`, is gitignored by the frost repo, and is never committed into it).
The fork exists so Frost can move on its own schedule: engine fixes land as
fork commits instead of a permanent pile of `git apply` diffs.

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

All three were proven earlier in the servo-de embedder against Servo 0.8.0.
Each was verified with `git apply --check` against this fork point before
applying, one commit per patch. The patch files are vendored in `patches/`
so the fork is reproducible from this repo alone (`./clone-fork.sh`).

| Commit | Patch | What it does |
|---|---|---|
| `deaae1393` | `patches/servo-keypress-charcode-0.8.patch` | Derive legacy `charCode` from `KeyboardEvent`, so pages that rely on keypress charCode (terminal-style input) work. |
| `feca6e10b` | `patches/servo-damage-rects-0.8.patch` | Expose `WebView::last_damage_rects()` so the embedder can present only damaged regions instead of full frames. |
| `3200e8754` | `patches/servo-dmabuf-0.8.patch` | Export DMA-BUF handles from the rendering context so the embedder can present frames zero-copy. |

Fork log:

```
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
| **F-001** | No `WebView::stop` in Servo's public API (verified: `components/servo/webview.rs` has no `stop`). The engine has `window.stop()` internally but no embedder-facing trigger. | Add `WebView::stop()` that sends the stop message to the constellation (mirrors the script thread's `stop_loading`). Until then `FrostWebView::stop()` returns `EngineError::Unsupported` and the UI keeps Stop disabled. |
| **F-002** | No DOM snapshot API — the inspector needs one and Servo ships no inspector. | Add a constellation request that serializes a DOM subtree (tag, attributes, computed style, box) for `frost-inspector`. |
| **F-003** | Limited print support (no print-to-PDF path an embedder can drive). | Investigate Servo's print pipeline; if it cannot be driven headlessly, add a fork hook that renders a page to paginated images for `frost://print`. |
| **F-004** | No spellcheck API. | Later round; likely a fork-side hook into the text-input path, or a pure-Rust spellcheck in the UI layer. |

## Updating the fork

When a new upstream Servo release is worth tracking:

1. `git -C servo fetch --unshallow` (once) then `git -C servo fetch origin`.
2. Create a branch at the new tag, cherry-pick the fork commits on top.
3. Re-run `git apply --check` for each patch in `patches/`; fix conflicts
   in the fork, not in the patch files, and record what changed here.
4. Re-run the smoke test (`cargo run -p frost-engine --example smoke`).
