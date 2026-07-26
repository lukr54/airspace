//! Talking to mpv over its JSON IPC pipe.
//!
//! Two things here are not obvious and both cost real time to find.
//!
//! First, use two separate connections to the pipe: one that only reads events,
//! one that only writes commands. Sharing a single handle between a blocking
//! reader and a writer crashes on the first command.
//!
//! Second, drain the writer. mpv sends command replies and async events to every
//! client that is connected. Nothing ever reads the write connection, so its
//! pipe buffer fills, mpv blocks trying to write to it, and then it stops reading
//! our commands. Every control dies while position updates keep arriving, so it
//! looks like a UI bug rather than a pipe problem.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::windows::io::AsRawHandle;
use std::process::{Child, Command};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

pub struct Mpv {
    writer: Mutex<Option<File>>,
    child: Mutex<Option<Child>>,
}

impl Mpv {
    pub fn new() -> Self {
        Self {
            writer: Mutex::new(None),
            child: Mutex::new(None),
        }
    }

    /// Launch mpv rendering into `hwnd` and connect to its IPC pipe.
    pub fn play(&self, app: &AppHandle, hwnd: isize, path: &str) -> Result<(), String> {
        self.stop();

        let pipe = format!(r"\\.\pipe\airspace-mpv-{}", std::process::id());
        let child = Command::new(mpv_binary())
            .arg(format!("--wid={hwnd}"))
            .arg(format!("--input-ipc-server={pipe}"))
            // mpv's own OSD and key handling both fight the page for input.
            .arg("--no-osc")
            .arg("--no-osd-bar")
            .arg("--input-default-bindings=no")
            .arg("--input-vo-keyboard=no")
            // gpu-next's flip presentation ignores the window region, so the
            // holes stop working and video paints over the controls.
            .arg("--d3d11-flip=no")
            .arg("--keep-open=yes")
            .arg(path)
            .spawn()
            .map_err(|e| format!("could not start mpv ({e}). Is it on PATH?"))?;

        let reader = connect(&pipe).map_err(|e| format!("no IPC pipe from mpv: {e}"))?;
        let writer = connect(&pipe).map_err(|e| format!("second IPC connection failed: {e}"))?;

        // Stop mpv pushing events down the write connection. It still sends
        // command replies, so `send` drains anyway.
        let mut w = writer;
        let _ = writeln!(w, r#"{{"command":["disable_event","all"]}}"#);
        let _ = w.flush();

        *self.child.lock().unwrap() = Some(child);
        *self.writer.lock().unwrap() = Some(w);

        spawn_reader(app.clone(), reader);
        Ok(())
    }

    pub fn send(&self, args: Vec<Value>) {
        let mut guard = self.writer.lock().unwrap();
        let Some(file) = guard.as_mut() else { return };
        drain(file);
        let line = json!({ "command": args }).to_string();
        if writeln!(file, "{line}").is_err() {
            return;
        }
        let _ = file.flush();
        drain(file);
    }

    pub fn stop(&self) {
        if self.writer.lock().unwrap().is_some() {
            self.send(vec![json!("quit")]);
        }
        *self.writer.lock().unwrap() = None;
        if let Some(mut child) = self.child.lock().unwrap().take() {
            // quit is usually enough, but don't leave an orphan if it wasn't.
            std::thread::sleep(Duration::from_millis(120));
            match child.try_wait() {
                Ok(Some(_)) => {}
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
    }
}

fn mpv_binary() -> String {
    std::env::var("MPV").unwrap_or_else(|_| "mpv".into())
}

/// mpv creates the pipe a moment after launch, so retry briefly.
fn connect(pipe: &str) -> std::io::Result<File> {
    let mut last = None;
    for _ in 0..120 {
        match OpenOptions::new().read(true).write(true).open(pipe) {
            Ok(f) => return Ok(f),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    Err(last.unwrap())
}

/// Read and discard whatever mpv has queued on this connection.
fn drain(file: &File) {
    use windows_sys::Win32::Storage::FileSystem::ReadFile;
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;

    let handle = file.as_raw_handle();
    let mut buf = [0u8; 4096];
    loop {
        let mut available = 0u32;
        let ok = unsafe {
            PeekNamedPipe(
                handle,
                core::ptr::null_mut(),
                0,
                core::ptr::null_mut(),
                &mut available,
                core::ptr::null_mut(),
            )
        };
        if ok == 0 || available == 0 {
            return;
        }
        let want = available.min(buf.len() as u32);
        let mut read = 0u32;
        let ok = unsafe {
            ReadFile(
                handle,
                buf.as_mut_ptr(),
                want,
                &mut read,
                core::ptr::null_mut(),
            )
        };
        if ok == 0 || read == 0 {
            return;
        }
    }
}

/// Ask for the properties we care about, then forward them to the page.
fn spawn_reader(app: AppHandle, reader: File) {
    std::thread::spawn(move || {
        let mut w = match reader.try_clone() {
            Ok(f) => f,
            Err(_) => return,
        };
        for (id, prop) in [
            (1, "time-pos"),
            (2, "duration"),
            (3, "pause"),
            (4, "eof-reached"),
        ] {
            let _ = writeln!(w, r#"{{"command":["observe_property",{id},"{prop}"]}}"#);
        }
        let _ = w.flush();

        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if msg["event"] != "property-change" {
                continue;
            }
            let value = &msg["data"];
            match msg["name"].as_str() {
                Some("time-pos") => emit(&app, "mpv-pos", value),
                Some("duration") => emit(&app, "mpv-dur", value),
                Some("pause") => emit(&app, "mpv-paused", value),
                Some("eof-reached") if value == &Value::Bool(true) => {
                    let _ = app.emit("mpv-ended", ());
                }
                _ => {}
            }
        }
    });
}

fn emit(app: &AppHandle, event: &str, value: &Value) {
    if !value.is_null() {
        let _ = app.emit(event, value.clone());
    }
}
