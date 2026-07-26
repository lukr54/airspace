#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod input;
mod mpv;

use serde_json::json;
use tauri::Manager;
use tauri_plugin_airspace::AirspaceExt;

use mpv::Mpv;

#[tauri::command]
fn open(app: tauri::AppHandle, window: tauri::WebviewWindow, path: String) -> Result<(), String> {
    if !std::path::Path::new(&path).is_file() {
        return Err(format!("not a file: {path}"));
    }

    let airspace = app.airspace();
    let hwnd = match airspace.host_handle("main") {
        Some(h) => h,
        None => airspace.create_host("main").map_err(|e| e.to_string())?,
    };

    app.state::<Mpv>().play(&app, hwnd, &path)?;

    let parent = window
        .hwnd()
        .map(|h| h.0 as isize)
        .map_err(|e| e.to_string())?;
    input::watch(app.clone(), parent, hwnd);
    Ok(())
}

#[tauri::command]
fn close(app: tauri::AppHandle) -> Result<(), String> {
    input::stop();
    app.state::<Mpv>().stop();
    app.airspace()
        .destroy_host("main")
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn toggle_pause(app: tauri::AppHandle) {
    app.state::<Mpv>()
        .send(vec![json!("cycle"), json!("pause")]);
}

#[tauri::command]
fn seek(app: tauri::AppHandle, secs: f64) {
    app.state::<Mpv>()
        .send(vec![json!("seek"), json!(secs), json!("absolute")]);
}

#[tauri::command]
fn seek_relative(app: tauri::AppHandle, secs: f64) {
    app.state::<Mpv>()
        .send(vec![json!("seek"), json!(secs), json!("relative")]);
}

#[tauri::command]
fn set_volume(app: tauri::AppHandle, level: f64) {
    app.state::<Mpv>()
        .send(vec![json!("set_property"), json!("volume"), json!(level)]);
}

/// A path given on the command line, so `cargo run -p airspace-mpv -- video.mkv`
/// starts playing immediately.
#[tauri::command]
fn initial_path() -> Option<String> {
    std::env::args().nth(1).filter(|a| !a.starts_with('-'))
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_airspace::init())
        .manage(Mpv::new())
        .invoke_handler(tauri::generate_handler![
            open,
            close,
            toggle_pause,
            seek,
            seek_relative,
            set_volume,
            initial_path
        ])
        .on_window_event(|window, event| {
            // mpv is a separate process. Without this, closing the window leaves
            // it running with a parent that no longer exists.
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                input::stop();
                window.app_handle().state::<Mpv>().stop();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
