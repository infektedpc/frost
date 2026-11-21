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
//!   [`frozen::FrostInputEvent`]s. Keyboard goes to the page only
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
//! * Round 16: the UI compositor defaults to the GL backend
//!   (`WGPU_BACKEND=gl` set in `main` only if unset AND only if a
//!   wgpu probe finds a GL adapter — see `ensure_gl_backend_default`
//!   and `gl_adapter_available`), and the engine renders pages
//!   through a hardware surfman adapter (fork patch F-009) with a
//!   software fallback. If no GL adapter exists, the default is
//!   simply not set and the app takes iced's normal Vulkan-first
//!   path, so a machine without GL still launches. A user-set
//!   `WGPU_BACKEND` is never overridden. (A retry was considered
//!   and rejected: on libcosmic's multi-window path the app's init
//!   runs before the compositor, so a second `app::run()` would
//!   panic in the engine's one-shot `setup_logging()`.)
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
mod archive;

#[cfg(feature = "ui-libcosmic")]
mod cache;

#[cfg(feature = "ui-libcosmic")]
mod file_browser;

#[cfg(feature = "ui-libcosmic")]
mod keymap;

#[cfg(feature = "ui-libcosmic")]
mod settings;

#[cfg(feature = "ui-libcosmic")]
#[cfg(feature = "ui-libcosmic")]
mod app;

/// Test-only: serialize tests that mutate process env vars (the
/// XDG-path tests in `settings` and `archive`). Env vars are
/// process-global and the test runner runs tests on multiple threads
/// in one process, so two env-mutating tests must never overlap.
/// The lock is poison-tolerant: a test that panics mid-mutation must
/// not wedge the rest of the suite.
#[cfg(all(test, feature = "ui-libcosmic"))]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Test-only: run `f` with the given env vars set (or removed, for
/// `None`), restoring the previous values afterwards — even if `f`
/// panics. Holds `ENV_LOCK` for the whole call, including the
/// restore.
#[cfg(all(test, feature = "ui-libcosmic"))]
pub(crate) fn with_env(vars: &[(&str, Option<&str>)], f: impl FnOnce()) {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    struct Restore<'a> {
        saved: Vec<(&'a str, Option<std::ffi::OsString>)>,
    }
    impl Drop for Restore<'_> {
        fn drop(&mut self) {
            for (k, v) in self.saved.drain(..) {
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
    }
    let saved: Vec<(&str, Option<std::ffi::OsString>)> = vars
        .iter()
        .map(|(k, _)| (*k, std::env::var_os(k)))
        .collect();
    let restore = Restore { saved };
    for (k, v) in vars {
        match v {
            Some(v) => std::env::set_var(k, v),
            None => std::env::remove_var(k),
        }
    }
    f();
    drop(restore);
}

#[cfg(feature = "ui-libcosmic")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    ensure_gl_backend_default();
    app::run()
}

/// Round 16: is there a GL adapter on this machine? (Probe for the
/// `WGPU_BACKEND=gl` default — see `ensure_gl_backend_default`.)
///
/// A bare wgpu instance with the GL backend only, no window: if it
/// enumerates at least one adapter, the compositor's later GL request
/// will find one too. wgpu-hal reports a broken EGL as "no adapters",
/// never a panic, so a machine with no working EGL just answers false
/// here. (That does not save a machine with no EGL at all: the engine
/// still needs EGL to render pages, and there surfman's
/// `Connection::new` asserts on `eglInitialize` — see ARCHITECTURE.md.
/// The probe covers "no GL adapter", not "no EGL".)
#[cfg(feature = "ui-libcosmic")]
fn gl_adapter_available() -> bool {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::GL,
        ..Default::default()
    });
    !futures_executor::block_on(instance.enumerate_adapters(wgpu::Backends::GL)).is_empty()
}

/// Round 16: default the wgpu backend to GL (hardware OpenGL via Mesa)
/// — but only when a GL adapter actually exists.
///
/// Why the env var, not a code setting: libcosmic's iced wgpu
/// compositor reads `WGPU_BACKEND` itself (`Backends::from_env`) when
/// it creates the adapter, and there is no public settings knob for
/// it — the env var is the supported mechanism. Setting it in `main`
/// (before any iced code runs) covers every launch path, including a
/// plain `frost` from a terminal; a `.desktop` `Exec=` line would only
/// cover desktop launches, so it is the weaker choice and is not used.
///
/// Why a probe instead of "set it and retry on failure": a retry
/// cannot work. Frost runs on libcosmic's multi-window (daemon) path,
/// where the app's `init` — and the engine's one-shot
/// `setup_logging()` — runs BEFORE the compositor is created, so a
/// second `app::run()` after a graphics failure would panic in
/// `setup_logging()` (`log::set_boxed_logger` can only run once per
/// process). The probe decides before anything starts: GL adapter
/// present → set the default; absent → leave the backend unset and
/// Frost takes iced's normal Vulkan-first path (lavapipe on a machine
/// with no hardware GL — slow, but it launches). A user-set value is
/// never touched: `WGPU_BACKEND=vulkan` (or anything else) keeps
/// working exactly as before.
///
/// Returns true when Frost set the GL default.
#[cfg(feature = "ui-libcosmic")]
fn ensure_gl_backend_default() -> bool {
    if std::env::var_os("WGPU_BACKEND").is_some() {
        return false;
    }
    if gl_adapter_available() {
        std::env::set_var("WGPU_BACKEND", "gl");
        return true;
    }
    eprintln!("frost: no GL adapter found; using the default Vulkan path");
    false
}

#[cfg(not(feature = "ui-libcosmic"))]
fn main() {
    eprintln!("frost: libcosmic UI is not enabled in this build.");
    eprintln!("frost: run with `cargo run -p frost --features ui-libcosmic` for the window.");
    eprintln!("frost: the engine is headless — prove it with:");
    eprintln!("frost:   cargo run -p frozen --example smoke");
}

#[cfg(all(test, feature = "ui-libcosmic"))]
mod tests {
    use super::*;

    #[test]
    fn gl_adapter_probe_finds_the_sandbox_gl_adapter() {
        // The sandbox's vendored Mesa EGL always offers a GL adapter
        // (llvmpipe), so the probe answers true here. On a machine
        // with no GL adapter it answers false — that branch is covered
        // by the gate (launch with GL broken), not by this test.
        assert!(gl_adapter_available());
    }

    #[test]
    fn gl_backend_default_respects_user_value() {
        // Save and restore: no other test touches WGPU_BACKEND.
        let saved = std::env::var_os("WGPU_BACKEND");
        std::env::remove_var("WGPU_BACKEND");
        assert!(ensure_gl_backend_default());
        assert_eq!(std::env::var("WGPU_BACKEND").as_deref(), Ok("gl"));
        std::env::set_var("WGPU_BACKEND", "vulkan");
        assert!(!ensure_gl_backend_default());
        assert_eq!(std::env::var("WGPU_BACKEND").as_deref(), Ok("vulkan"));
        match saved {
            Some(v) => std::env::set_var("WGPU_BACKEND", v),
            None => std::env::remove_var("WGPU_BACKEND"),
        }
    }
}
