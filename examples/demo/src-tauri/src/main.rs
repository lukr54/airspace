#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod paint;

use tauri::{Emitter, WebviewUrl};
use tauri_plugin_airspace::{AirspaceExt, OverlayBuilder};

/// Create the opaque native host over the main window and start painting into it.
#[tauri::command]
fn start_native(app: tauri::AppHandle) -> Result<i64, String> {
    let hwnd = app
        .airspace()
        .create_host("main")
        .map_err(|e| e.to_string())?;
    paint::spawn(hwnd);
    Ok(hwnd as i64)
}

#[tauri::command]
fn stop_native(app: tauri::AppHandle) -> Result<(), String> {
    paint::stop();
    app.airspace()
        .destroy_host("main")
        .map_err(|e| e.to_string())
}

/// Show or hide the click-through overlay's content.
///
/// It doesn't hide the window: a transparent WebView2 window that gets hidden
/// and shown again comes back blank. The window stays up from startup and only
/// the content toggles, which is invisible anyway while it's empty.
#[tauri::command]
fn toggle_overlay(app: tauri::AppHandle, on: bool) -> Result<(), String> {
    // This only reaches the overlay because "hud" is listed in the
    // capability's `windows`. Leave it out and `listen()` in the second window
    // is denied while `invoke()` of your own commands still works, which reads
    // like "Tauri events don't reach runtime-created windows". They do. The
    // window just wasn't granted core:event.
    app.emit_to("hud", "hud-toggle", on)
        .map_err(|e| e.to_string())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_airspace::init())
        .invoke_handler(tauri::generate_handler![
            start_native,
            stop_native,
            toggle_overlay
        ])
        .setup(|app| {
            // Built in setup(), not from a command: building a webview window
            // inside a sync command can hang forever (tauri-apps/tauri#12521).
            OverlayBuilder::new("hud", WebviewUrl::App("overlay.html".into()))
                .size(360.0, 132.0)
                .position(80.0, 96.0)
                .build(app.handle())?;

            // AIRSPACE_DEMO_AUTOSTART=1 brings the host up without any
            // clicking, so the window region can be checked from a script. Also
            // exercises create_host from setup(), which is where people will
            // reach for it.
            if std::env::var("AIRSPACE_DEMO_AUTOSTART").as_deref() == Ok("1") {
                let delay: u64 = std::env::var("AIRSPACE_DEMO_DELAY_MS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(delay));
                    match handle.airspace().create_host("main") {
                        Ok(hwnd) => {
                            paint::spawn(hwnd);
                            println!("[autostart] host hwnd = {hwnd:#x} (after {delay}ms)");
                            // Hand over to the page so the holes come from
                            // the JS path: measure the overlays, scale by
                            // devicePixelRatio, invoke the plugin command.
                            let _ = handle.emit("native-started", ());
                        }
                        Err(e) => println!("[autostart] failed: {e}"),
                    }
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
