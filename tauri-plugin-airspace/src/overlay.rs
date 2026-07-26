//! The cross-window case: native content that isn't yours at all, like a
//! fullscreen game or another process's window, with your HTML over it.
//!
//! Hole-punching doesn't apply, since you can't clip a window you don't own.
//! Instead use a separate top-level window that's transparent, always-on-top
//! and click-through.

use crate::error::{Error, Result};
use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// Builds a transparent, always-on-top, click-through overlay window with the
/// flag combination that works on Windows.
///
/// The defaults matter, see [`OverlayBuilder::build`] for why.
pub struct OverlayBuilder {
    label: String,
    url: WebviewUrl,
    size: (f64, f64),
    position: Option<(f64, f64)>,
    click_through: bool,
}

impl OverlayBuilder {
    pub fn new(label: impl Into<String>, url: WebviewUrl) -> Self {
        Self {
            label: label.into(),
            url,
            size: (420.0, 160.0),
            position: None,
            click_through: true,
        }
    }

    pub fn size(mut self, width: f64, height: f64) -> Self {
        self.size = (width, height);
        self
    }

    pub fn position(mut self, x: f64, y: f64) -> Self {
        self.position = Some((x, y));
        self
    }

    /// Whether the overlay passes every mouse event through to whatever is
    /// underneath. Default `true`. Turn it off temporarily if you want the user
    /// to be able to drag or click the overlay itself.
    pub fn click_through(mut self, yes: bool) -> Self {
        self.click_through = yes;
        self
    }

    /// Build the overlay.
    ///
    /// # Call this from `setup()`, not from a command
    ///
    /// `WebviewWindowBuilder::build()` dispatches to the event loop and waits.
    /// A synchronous command holds a lock the event loop needs, so building a
    /// webview window from inside one can hang forever. The window gets created
    /// but never navigates and you're left with a white, frozen, uncloseable
    /// client area. See <https://github.com/tauri-apps/tauri/issues/12521>.
    ///
    /// Build overlays once at startup and `show()`/`hide()` them after that, or
    /// use [`OverlayBuilder::spawn`], which doesn't block the caller.
    ///
    /// # About the flags
    ///
    /// - `transparent(true)` + `decorations(false)`: on Windows these go
    ///   together. `transparent(true)` with `decorations(true)` renders an empty
    ///   see-through box, because WebView2 won't composite a transparent surface
    ///   behind a native title bar. It's also intermittent, so it paints
    ///   correctly on some launches.
    /// - `visible(true)`: a transparent WebView2 window created hidden and shown
    ///   later stays blank, never composited, and nudging doesn't help. Build it
    ///   visible and keep the content `display:none` until you have something to
    ///   draw. It's transparent, so that's invisible anyway.
    /// - `resizable(false)`: `decorations(false)` with `resizable(true)` leaves
    ///   invisible resize borders that users grab when they meant to drag the
    ///   overlay. Move it with `start_dragging()` instead.
    /// - `skip_taskbar(true)`: not a window anyone alt-tabs to.
    ///
    /// # Known limitation
    ///
    /// Always-on-top loses to exclusive-fullscreen native content, so overlays
    /// are only visible over borderless-windowed apps. Nothing to be done about
    /// it besides telling your users.
    pub fn build<R: Runtime>(self, app: &AppHandle<R>) -> Result<WebviewWindow<R>> {
        if let Some(existing) = app.get_webview_window(&self.label) {
            return Ok(existing);
        }

        let mut builder = WebviewWindowBuilder::new(app, &self.label, self.url)
            .inner_size(self.size.0, self.size.1)
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .shadow(false)
            .visible(true);

        // Tauri gates `transparent` behind macos-private-api on macOS, so the
        // method doesn't exist there unless that feature is on. Enable this
        // crate's `macos-private-api` feature to pass it through. The plugin is
        // a no-op off Windows regardless, so this only affects whether it
        // compiles.
        #[cfg(any(not(target_os = "macos"), feature = "macos-private-api"))]
        {
            builder = builder.transparent(true);
        }

        if let Some((x, y)) = self.position {
            builder = builder.position(x, y);
        }

        let window = builder.build()?;
        if self.click_through {
            let _ = window.set_ignore_cursor_events(true);
        }
        Ok(window)
    }

    /// Build the overlay on the main thread without blocking the caller.
    ///
    /// Safe to call from a command: it queues the work and returns immediately.
    /// You don't get a handle back, so look the window up by label once it exists.
    pub fn spawn<R: Runtime>(self, app: &AppHandle<R>) {
        let app2 = app.clone();
        let _ = app.run_on_main_thread(move || {
            let _ = self.build(&app2);
        });
    }
}

/// Toggle click-through on an existing overlay.
///
/// `true` means every mouse event goes to whatever is underneath, so the
/// overlay is display-only. Set it to `false` when you want the user to be able
/// to grab it, then back to `true` afterwards.
pub fn set_click_through<R: Runtime>(window: &WebviewWindow<R>, yes: bool) -> Result<()> {
    window.set_ignore_cursor_events(yes)?;
    Ok(())
}

/// Force a window that was just shown to actually present a frame.
///
/// WebView2 sometimes leaves a shown-from-hidden window blank until something
/// disturbs it. Resizing by a pixel and back is the cheapest way. Runs on a
/// background thread and retries a few times, since how long the webview takes
/// varies with how heavy the page is.
pub fn nudge_present<R: Runtime>(app: &AppHandle<R>, label: impl Into<String>) {
    let label = label.into();
    let app = app.clone();
    std::thread::spawn(move || {
        for delay in [120u64, 350, 650, 1000] {
            std::thread::sleep(std::time::Duration::from_millis(delay));
            let a = app.clone();
            let label = label.clone();
            let _ = app.run_on_main_thread(move || {
                if let Some(w) = a.get_webview_window(&label) {
                    if let Ok(size) = w.inner_size() {
                        let _ = w.set_size(tauri::PhysicalSize::new(size.width + 1, size.height));
                        let _ = w.set_size(tauri::PhysicalSize::new(size.width, size.height));
                    }
                }
            });
        }
    });
}

pub(crate) fn window_or_err<R: Runtime>(
    app: &AppHandle<R>,
    label: &str,
) -> Result<WebviewWindow<R>> {
    app.get_webview_window(label)
        .ok_or_else(|| Error::NoSuchWindow(label.to_string()))
}
