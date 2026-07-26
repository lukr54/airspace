//! The fallback that keeps the app usable while video covers it.
//!
//! Once the host is up and the control bar has auto-hidden there are no holes
//! left, so the webview receives no mouse and no keyboard. Nothing in the page
//! can bring the bar back. This polls the OS instead and emits events the page
//! can act on.
//!
//! Its lifetime is keyed on the host window still existing rather than on
//! whether we think playback is running. Key it on the latter and it can die
//! while the host is still up, which is exactly when it's needed.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tauri::{AppHandle, Emitter};

/// Bumped on each new playback so an older poller stops.
pub static EPOCH: AtomicUsize = AtomicUsize::new(0);

pub fn stop() {
    EPOCH.fetch_add(1, Ordering::SeqCst);
}

pub fn watch(app: AppHandle, parent: isize, host: isize) {
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, GetForegroundWindow, GetWindowRect, IsWindow,
    };

    const VK_ESCAPE: i32 = 0x1B;
    const VK_SPACE: i32 = 0x20;

    let epoch = EPOCH.fetch_add(1, Ordering::SeqCst) + 1;

    std::thread::spawn(move || {
        let parent = parent as *mut core::ffi::c_void;
        let host = host as *mut core::ffi::c_void;
        let mut last = (i32::MIN, i32::MIN);
        // Edge triggered, or a held key fires on every tick.
        let mut esc_down = false;
        let mut space_down = false;

        loop {
            if EPOCH.load(Ordering::SeqCst) != epoch {
                return;
            }
            unsafe {
                if IsWindow(host) == 0 {
                    return;
                }

                let mut pos: POINT = core::mem::zeroed();
                let mut rect: RECT = core::mem::zeroed();
                if GetCursorPos(&mut pos) != 0 && GetWindowRect(parent, &mut rect) != 0 {
                    let inside = pos.x >= rect.left
                        && pos.x < rect.right
                        && pos.y >= rect.top
                        && pos.y < rect.bottom;
                    if inside && (pos.x, pos.y) != last {
                        last = (pos.x, pos.y);
                        let _ = app.emit("activity", ());
                    }
                }

                if GetForegroundWindow() == parent {
                    let esc = GetAsyncKeyState(VK_ESCAPE) as u16 & 0x8000 != 0;
                    if esc && !esc_down {
                        let _ = app.emit("key", "Escape");
                    }
                    esc_down = esc;

                    let space = GetAsyncKeyState(VK_SPACE) as u16 & 0x8000 != 0;
                    if space && !space_down {
                        let _ = app.emit("key", "Space");
                    }
                    space_down = space;
                }
            }
            std::thread::sleep(Duration::from_millis(120));
        }
    });
}
