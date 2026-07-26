use crate::{error::Result, AirspaceExt, Hole};
use tauri::{AppHandle, Runtime, WebviewWindow};

// Sync on purpose: Tauri runs sync commands off the main thread, so the
// main-thread hop inside these is a hand-off rather than a re-entrant call.

#[tauri::command]
pub(crate) fn create_host<R: Runtime>(app: AppHandle<R>, window: WebviewWindow<R>) -> Result<i64> {
    app.airspace().create_host(window.label()).map(|h| h as i64)
}

#[tauri::command]
pub(crate) fn destroy_host<R: Runtime>(app: AppHandle<R>, window: WebviewWindow<R>) -> Result<()> {
    app.airspace().destroy_host(window.label())
}

#[tauri::command]
pub(crate) fn set_holes<R: Runtime>(
    app: AppHandle<R>,
    window: WebviewWindow<R>,
    holes: Vec<Hole>,
) -> Result<()> {
    app.airspace().set_holes(window.label(), holes)
}

#[tauri::command]
pub(crate) fn clear_holes<R: Runtime>(app: AppHandle<R>, window: WebviewWindow<R>) -> Result<()> {
    app.airspace().clear_holes(window.label())
}

#[tauri::command]
pub(crate) fn host_handle<R: Runtime>(app: AppHandle<R>, window: WebviewWindow<R>) -> Option<i64> {
    app.airspace().host_handle(window.label()).map(|h| h as i64)
}

/// Toggle click-through on an overlay window by label. Useful for letting the
/// user drag the HUD: drop click-through, let them move it, put it back.
#[tauri::command]
pub(crate) fn set_click_through<R: Runtime>(
    app: AppHandle<R>,
    label: String,
    enabled: bool,
) -> Result<()> {
    let window = crate::overlay::window_or_err(&app, &label)?;
    crate::overlay::set_click_through(&window, enabled)
}
