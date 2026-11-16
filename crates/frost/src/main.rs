//! Frost browser — libcosmic application shell.
//!
//! Round 2 wires the engine in for real:
//!
//! * The content area shows the live Servo frame, uploaded into iced
//!   textures whenever the engine reports `FrameReady` (or while loading).
//!   Round 6: the frame goes up as horizontal strips, each under iced's
//!   2 MiB synchronous-upload line — one whole-frame image per render
//!   flashed dark for a frame on every load/scroll repaint (see
//!   `Frame::split_strips`).
//! * The toolbar navigates for real: back / forward (disabled when the
//!   engine says there is nothing to go to), reload, stop (live since fork
//!   patch F-001), address bar (Enter navigates; bare hostnames get
//!   `https://`, anything else is a search with the default engine).
//! * Input forwards into the page: the app listens to all iced events and
//!   translates the ones landing in the content area into
//!   [`frost_engine::FrostInputEvent`]s. Keyboard goes to the page only
//!   while the content area has focus (a click in the content, or Enter in
//!   the address bar); typing in the address bar stays in the bar.
//! * The hamburger menu opens as a dropdown; Settings / Print /
//!   Inspector / Open / Exit each pop up their own native libcosmic
//!   window (Round 7, `windows.rs`) — no internal web server runs
//!   any more (the Round 3–6 loopback server is deleted; a `frost://`
//!   URL typed in the address bar fails plainly with an honest
//!   message). Open is a real file browser — the listing is generated
//!   app-side, folder clicks re-list, file clicks open the file in the
//!   browser, `.md` files render as styled native text; Exit quits
//!   the process (via the confirmation dialog when enabled).
//!   Inspector (Round 5) captures the current page's DOM before
//!   opening its window — the snapshot is taken BEFORE, and the
//!   inspector window renders that snapshot (labeled with its URL and
//!   capture time). The main window keeps showing the inspected page,
//!   so Re-capture just re-runs the walker on the live page.
//! * Settings (`settings.rs`) are edited in the native Settings
//!   window: theme (light/dark, applied live to Frost's own chrome
//!   and to the pages on Save), homepage (next launch), default
//!   search engine, exit confirmation. Settings persist as JSON
//!   under the config dir. The Extensions list lives in the Settings
//!   window too (Round 7).
//! * Extension host v1 (`frost-extensions`): extensions load from
//!   directories (manifest + content script), the host matches content
//!   scripts against page URLs, and the engine evaluates them in the
//!   page. A bad manifest is logged and skipped, never a crash.
//! * Round 10: the window manager draws the titlebar by default
//!   (`client_decorations: false`); Frost's own titlebar is now
//!   only the GNOME fallback (Mutter refuses server-side
//!   decorations, so there the WM bar would mean no titlebar at
//!   all). The built-in libcosmic headerbar stays hidden in both
//!   modes — it renders the title but no window-control buttons
//!   on this setup. The fallback bar reads "Frost — <page title>"
//!   with real minimize, maximize/restore, and close buttons
//!   (text glyphs, real iced window commands). Either way the OS
//!   window title is kept in step, so the WM shows the right
//!   title in WM mode. The mode is picked once at startup from
//!   `XDG_CURRENT_DESKTOP` (`XDG_SESSION_DESKTOP` as fallback):
//!   GNOME (or any value containing it) gets Frost's own bar,
//!   everything else — including unset — gets the WM bar.
//! * Round 4, F-007: the Round 2 resize "jiggle" is RETIRED. Its root
//!   cause was the fork's damage-rects patch setting WebRender
//!   `max_partial_present_rects: 1` (partial present) while Frost's
//!   engine reads the GL back buffer wholesale — the full scene
//!   rendered once and was lost, and only dirty regions repainted.
//!   The fork now uses full present (`max_partial_present_rects: 0`,
//!   the upstream default), so the first paint is complete and no
//!   post-load jiggle is needed. F-005's `refresh_viewport` stays
//!   (harmless viewport re-send).
//!
//! Chrome geometry is fixed by construction (toolbar 48, status 28,
//! plus Frost's own titlebar 40 only in the GNOME fallback — 0 in
//! WM mode), so the webview size and the click mapping are exact:
//! the content area is the window minus the chrome, and mouse
//! coordinates translate 1:1 into page coordinates. The hamburger menu
//! (Round 8) is a floating panel over the content — it consumes no
//! layout space, so it does not figure in the chrome heights above
//! (Rounds 2–7 subtracted a 148 px menu row here).
//!
//! Launch with:
//!   cargo run -p frost
//! Without the `ui-libcosmic` feature this binary prints a short status
//! note and exits 0, so the workspace always compiles even where
//! libcosmic cannot build.

#[cfg(feature = "ui-libcosmic")]
mod file_browser;

#[cfg(feature = "ui-libcosmic")]
mod keymap;

#[cfg(feature = "ui-libcosmic")]
mod settings;

#[cfg(feature = "ui-libcosmic")]
#[cfg(feature = "ui-libcosmic")]
mod app;

#[cfg(feature = "ui-libcosmic")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    app::run()
}

#[cfg(not(feature = "ui-libcosmic"))]
fn main() {
    eprintln!("frost: libcosmic UI is not enabled in this build.");
    eprintln!("frost: run with `cargo run -p frost --features ui-libcosmic` for the window.");
    eprintln!("frost: the engine is headless — prove it with:");
    eprintln!("frost:   cargo run -p frost-engine --example smoke");
}
