//! # tauri-plugin-airspace
//!
//! Render native content **under** or **alongside** HTML in a Tauri v2 window
//! on Windows.
//!
//! ## Background
//!
//! "Airspace" is the WPF/WinForms interop name for a plain Win32 constraint: a
//! windowed child HWND owns its rectangle. It isn't a layer in a composited
//! tree, it's a hole in the desktop that other code paints into. Two sibling
//! child HWNDs can't alpha-blend with each other, and z-order applies to whole
//! rectangles.
//!
//! Which is why the usual first idea, making the webview transparent and
//! floating it over the video, can't work. Tauri hosts WebView2 in *windowed*
//! mode, so transparent web pixels don't reveal the sibling window under them,
//! they reveal whatever is behind the top-level window (usually the desktop).
//! There's no flag for it.
//!
//! Two approaches work, depending on whether the native content is inside your
//! window or in someone else's.
//!
//! ## 1. Native content inside your window: hole-punching
//!
//! Put an opaque native child window over the whole client area, give its
//! `HWND` to whatever renders (mpv via `--wid`, your own D3D swapchain, a child
//! process), then cut the HTML controls out of that window's region with
//! `SetWindowRgn`. Where the region is clipped away the child doesn't exist as
//! far as the compositor cares, so the webview shows through, with your HTML
//! there on top of full-size native content.
//!
//! The alternative is to shrink the native child and leave room for a control
//! strip. That works, and it's what most apps do, but the video changes size
//! whenever a transient control appears.
//!
//! ```no_run
//! use tauri_plugin_airspace::{AirspaceExt, Hole};
//!
//! let builder = tauri::Builder::default()
//!     .plugin(tauri_plugin_airspace::init())
//!     .setup(|app| {
//!         let airspace = app.airspace();
//!         // Create the opaque host over the main window, then hand the HWND
//!         // to your renderer (e.g. `mpv --wid=<hwnd>`).
//!         let hwnd = airspace.create_host("main")?;
//!         // Cut a 900x76 control bar out of the bottom (physical pixels).
//!         airspace.set_holes("main", vec![Hole::new(20, 800, 900, 76)])?;
//!         let _ = hwnd;
//!         Ok(())
//!     });
//! // ...then `builder.run(tauri::generate_context!())` as usual.
//! ```
//!
//! From JS, after granting `airspace:default` in a capability:
//!
//! ```js
//! const { invoke } = window.__TAURI__.core;
//! const r = el.getBoundingClientRect();
//! const s = window.devicePixelRatio; // holes are PHYSICAL pixels
//! await invoke('plugin:airspace|set_holes', {
//!   holes: [{ x: r.x * s | 0, y: r.y * s | 0, w: r.width * s | 0, h: r.height * s | 0 }],
//! });
//! ```
//!
//! ### WS_CLIPSIBLINGS
//!
//! The host, and every sibling it overlaps (the webview in particular), need
//! `WS_CLIPSIBLINGS`. Overlapping siblings without it are undefined per the
//! Win32 rules, each one can paint over the others, and in practice WebView2
//! wins. The failure is confusing because the host is clearly there: topmost in
//! z-order, correct window region, wins every hit-test, and not one pixel on
//! screen. wry doesn't set the style on `WRY_WEBVIEW`, so this plugin adds it to
//! every sibling when it creates the host.
//!
//! If you already have a working implementation of this technique, check whether
//! it sets the style. Content that presents continuously (a video player at
//! 60fps) can win the repaint race often enough to look correct without it.
//!
//! ### What can render into the host
//!
//! mpv via `--wid`, your own D3D renderer, a child process. Plain GDI painting
//! works too once `WS_CLIPSIBLINGS` is set.
//!
//! ### No holes means no input
//!
//! With no holes the webview receives nothing, mouse or keyboard, because the
//! opaque host owns the whole client area. An auto-hiding control bar can't be
//! brought back by a mouse move, and in fullscreen that can leave the app with
//! no way out. Anything the user needs wants a non-web fallback (a background
//! thread polling `GetCursorPos` and key state, say), and that fallback's
//! lifetime should key on the host window existing rather than on whether your
//! player thinks it's playing.
//!
//! Two more:
//!
//! - If native content paints over your holes, its presentation path is skipping
//!   the window region. This is per-renderer rather than a rule about the flip
//!   model. mpv's gpu-next flip path does it until `--d3d11-flip=no`, but a
//!   `DXGI_SWAP_EFFECT_FLIP_DISCARD` swapchain created directly on the host
//!   respected the region when measured here. Test yours.
//! - Anything still visible in the DOM shows through the holes. "Hidden behind
//!   the video" isn't a thing when native content is on top.
//!
//! ## 2. Native content in another process: a click-through overlay window
//!
//! You can't clip a window you don't own, so the other approach is a separate
//! top-level window that's transparent, always-on-top and click-through.
//! [`OverlayBuilder`] sets the flag combination that works, and its docs say
//! why each one matters.
//!
//! ```no_run
//! use tauri_plugin_airspace::OverlayBuilder;
//! use tauri::WebviewUrl;
//!
//! let builder = tauri::Builder::default()
//!     .plugin(tauri_plugin_airspace::init())
//!     .setup(|app| {
//!         // Build overlays in setup(), never from a command. See OverlayBuilder::build.
//!         OverlayBuilder::new("hud", WebviewUrl::App("overlay.html".into()))
//!             .size(430.0, 152.0)
//!             .build(app.handle())?;
//!         Ok(())
//!     });
//! ```
//!
//! ## Platforms
//!
//! Windows only. On other platforms every call returns
//! [`Error::UnsupportedPlatform`] rather than failing to compile, so
//! cross-platform apps can keep one code path.

use std::collections::HashMap;
use std::sync::Mutex;

use tauri::{
    plugin::{Builder, TauriPlugin},
    AppHandle, Manager, Runtime, WindowEvent,
};

mod commands;
mod error;
mod host;
mod overlay;

pub use error::{Error, Result};
pub use host::Hole;
pub use overlay::{nudge_present, set_click_through, OverlayBuilder};

use host::{imp, Host};

/// Plugin state. Reach it with [`AirspaceExt::airspace`].
pub struct Airspace<R: Runtime> {
    app: AppHandle<R>,
    hosts: Mutex<HashMap<String, Host>>,
    /// Captured during plugin setup, which runs on the main thread. Lets
    /// [`Airspace::on_main`] tell "dispatch and wait" apart from "already here,
    /// just run it". Without it, calling `create_host` from `setup()` queues
    /// work behind the thread that's waiting for it.
    main_thread: std::thread::ThreadId,
}

impl<R: Runtime> Airspace<R> {
    /// Create the opaque native host child window over the window `label`, and
    /// return its `HWND` as an `isize`. Give that to whatever will render into
    /// it (`mpv --wid=<hwnd>`, your own swapchain, a child process).
    ///
    /// The host starts with no holes, i.e. covering the entire client area.
    ///
    /// Creation is marshalled to the main thread, because a child window's
    /// messages are dispatched to the thread that created it and only the main
    /// thread pumps them. Calling this *from* the main thread will therefore
    /// time out rather than deadlock.
    pub fn create_host(&self, label: &str) -> Result<isize> {
        #[cfg(not(windows))]
        {
            let _ = label;
            Err(Error::UnsupportedPlatform)
        }
        #[cfg(windows)]
        {
            let window = overlay::window_or_err(&self.app, label)?;
            let parent = window
                .hwnd()
                .map(|h| h.0 as isize)
                .map_err(|_| Error::NoHandle(label.to_string()))?;

            {
                let hosts = self.hosts.lock().unwrap();
                if let Some(existing) = hosts.get(label) {
                    if imp::alive(existing.child) {
                        return Err(Error::HostExists(label.to_string()));
                    }
                }
            }

            // Create and fit in one hop to the main thread.
            let child = self.on_main(move || {
                let child = imp::create_child(parent)?;
                imp::apply(child, parent, &[]);
                Ok::<isize, u32>(child)
            })??;

            self.hosts.lock().unwrap().insert(
                label.to_string(),
                Host {
                    parent,
                    child,
                    holes: Vec::new(),
                },
            );
            Ok(child)
        }
    }

    /// Destroy the native host for `label`, if any. Idempotent.
    pub fn destroy_host(&self, label: &str) -> Result<()> {
        let host = self.hosts.lock().unwrap().remove(label);
        if let Some(host) = host {
            let child = host.child;
            let _ = self.app.run_on_main_thread(move || imp::destroy(child));
        }
        Ok(())
    }

    /// Replace the set of holes cut in the host for `label`.
    ///
    /// Rects are physical pixels relative to the window's client area. Cheap
    /// enough to call on every animation frame; it does not block the caller.
    pub fn set_holes(&self, label: &str, holes: Vec<Hole>) -> Result<()> {
        let (child, parent, holes) = {
            let mut hosts = self.hosts.lock().unwrap();
            let host = hosts
                .get_mut(label)
                .ok_or_else(|| Error::NoHost(label.to_string()))?;
            host.holes = holes;
            (host.child, host.parent, host.holes.clone())
        };
        if std::thread::current().id() == self.main_thread {
            imp::apply(child, parent, &holes);
        } else {
            // Fire and forget: this is called on every frame of an animating
            // overlay, and the caller has nothing to learn from waiting.
            let _ = self
                .app
                .run_on_main_thread(move || imp::apply(child, parent, &holes));
        }
        Ok(())
    }

    /// Cut no holes: the host covers the whole client area again.
    ///
    /// Remember that this also means the webview stops receiving input.
    pub fn clear_holes(&self, label: &str) -> Result<()> {
        self.set_holes(label, Vec::new())
    }

    /// The host's `HWND`, if one is live.
    pub fn host_handle(&self, label: &str) -> Option<isize> {
        self.hosts
            .lock()
            .unwrap()
            .get(label)
            .map(|h| h.child)
            .filter(|h| imp::alive(*h))
    }

    /// Re-fit the host to the window's current client size, keeping the current
    /// holes. Wired to `Resized` automatically; exposed for odd cases.
    pub fn refit(&self, label: &str) {
        let hosts = self.hosts.lock().unwrap();
        if let Some(host) = hosts.get(label) {
            imp::apply(host.child, host.parent, &host.holes);
        }
    }
}

impl<R: Runtime> Airspace<R> {
    /// Run `f` on the main thread and wait for its result.
    ///
    /// If we're already on the main thread (`setup()`, a window event handler)
    /// it runs inline. Dispatching in that case would queue the work behind the
    /// thread waiting for it, which is a deadlock that shows up as a five second
    /// pause.
    ///
    /// Otherwise it dispatches and waits with a timeout, so a caller that has
    /// blocked the event loop some other way gets an error instead of a hang.
    ///
    /// Only reachable on Windows: everywhere else `create_host` returns
    /// `UnsupportedPlatform` before it gets here.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn on_main<T: Send + 'static>(&self, f: impl FnOnce() -> T + Send + 'static) -> Result<T> {
        if std::thread::current().id() == self.main_thread {
            return Ok(f());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.app.run_on_main_thread(move || {
            let _ = tx.send(f());
        })?;
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| Error::MainThreadTimeout)
    }
}

impl From<u32> for Error {
    fn from(code: u32) -> Self {
        Error::CreateFailed(code)
    }
}

/// Access to the plugin state from any [`Manager`].
pub trait AirspaceExt<R: Runtime> {
    fn airspace(&self) -> tauri::State<'_, Airspace<R>>;
}

impl<R: Runtime, T: Manager<R>> AirspaceExt<R> for T {
    fn airspace(&self) -> tauri::State<'_, Airspace<R>> {
        self.state::<Airspace<R>>()
    }
}

/// Initialise the plugin.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("airspace")
        .invoke_handler(tauri::generate_handler![
            commands::create_host,
            commands::destroy_host,
            commands::set_holes,
            commands::clear_holes,
            commands::host_handle,
            commands::set_click_through,
        ])
        .setup(|app, _api| {
            app.manage(Airspace {
                app: app.clone(),
                hosts: Mutex::new(HashMap::new()),
                // Plugin setup runs on the main thread.
                main_thread: std::thread::current().id(),
            });
            Ok(())
        })
        .on_window_ready(|window| {
            let label = window.label().to_string();
            let app = window.app_handle().clone();
            window.on_window_event(move |event| match event {
                // The host is sized to the parent's client rect, so it has to be
                // re-fitted on every resize. Holes anchored to a window edge are
                // the frontend's problem: re-push them after a resize.
                // Dragging the window to a monitor with a different DPI changes
                // the client size in physical pixels, so the host has to be
                // re-fitted. Your holes are in physical pixels too, so the
                // frontend needs to re-measure and re-push them: listen for
                // Tauri's scale-change event, or watch devicePixelRatio.
                WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                    if let Some(state) = app.try_state::<Airspace<R>>() {
                        state.refit(&label);
                    }
                }
                WindowEvent::Destroyed => {
                    if let Some(state) = app.try_state::<Airspace<R>>() {
                        let _ = state.destroy_host(&label);
                    }
                }
                _ => {}
            });
        })
        .build()
}
