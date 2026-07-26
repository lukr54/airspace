//! The in-window case: an opaque native child window covering the webview, with
//! holes cut out of it so HTML shows through onto full-size native content.
//!
//! The crate docs explain why the obvious alternative (a transparent webview
//! floating over the native surface) can't work.

use serde::{Deserialize, Serialize};

/// A rectangle to cut out of the native host, in **physical pixels**, relative
/// to the top-left of the webview window's *client* area.
///
/// The distinction matters: CSS gives you logical pixels. Multiply by
/// `window.devicePixelRatio` before sending these, or your holes will be in the
/// wrong place on any display that isn't at 100% scaling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hole {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Hole {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
}

/// A live native host: the child window handle plus the holes currently cut in it.
#[derive(Debug, Clone)]
pub(crate) struct Host {
    /// HWND of the webview window (the parent).
    pub parent: isize,
    /// HWND of the child we created.
    pub child: isize,
    pub holes: Vec<Hole>,
}

#[cfg(windows)]
pub(crate) mod imp {
    use super::Hole;
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        CombineRgn, CreateRectRgn, DeleteObject, SetWindowRgn, RGN_DIFF,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindow,
        GetWindowLongPtrW, IsWindow, RegisterClassExW, SetWindowLongPtrW, SetWindowPos, CS_HREDRAW,
        CS_VREDRAW, GWL_STYLE, GW_CHILD, GW_HWNDNEXT, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, WNDCLASSEXW, WS_CHILD, WS_CLIPSIBLINGS,
        WS_VISIBLE,
    };

    fn class_name() -> *const u16 {
        static NAME: OnceLock<Vec<u16>> = OnceLock::new();
        NAME.get_or_init(|| "tauri_airspace_host\0".encode_utf16().collect())
            .as_ptr()
    }

    fn register_class() {
        static ONCE: OnceLock<()> = OnceLock::new();
        ONCE.get_or_init(|| unsafe {
            let mut wc: WNDCLASSEXW = core::mem::zeroed();
            wc.cbSize = core::mem::size_of::<WNDCLASSEXW>() as u32;
            // CS_HREDRAW | CS_VREDRAW: repaint the whole client area on resize,
            // so a host that *is* being painted into doesn't smear.
            wc.style = CS_HREDRAW | CS_VREDRAW;
            // DefWindowProcW on purpose: this window is just a surface for
            // something else (mpv via --wid, your own renderer, a child
            // process). Don't use a STATIC control instead, STATIC returns
            // HTTRANSPARENT from hit testing so it never receives input.
            wc.lpfnWndProc = Some(DefWindowProcW);
            wc.hInstance = GetModuleHandleW(core::ptr::null());
            wc.lpszClassName = class_name();
            RegisterClassExW(&wc);
        });
    }

    /// Give a window `WS_CLIPSIBLINGS`.
    ///
    /// Overlapping sibling windows without this style are undefined per the
    /// Win32 rules, each one can paint over the others, and in practice
    /// WebView2 wins. The host then ends up invisible while still having the
    /// correct z-order and window region and winning every hit-test.
    ///
    /// wry creates `WRY_WEBVIEW` without the style, so we add it to every
    /// sibling when creating the host.
    unsafe fn add_clip_siblings(hwnd: *mut core::ffi::c_void) {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        if style == 0 || (style as u32 & WS_CLIPSIBLINGS) != 0 {
            return;
        }
        SetWindowLongPtrW(hwnd, GWL_STYLE, style | WS_CLIPSIBLINGS as isize);
        // The style cache is only refreshed on a frame change.
        SetWindowPos(
            hwnd,
            core::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }

    /// Create the opaque child window over `parent`'s client area.
    /// MUST be called on the thread that owns `parent` (the main/UI thread).
    pub fn create_child(parent: isize) -> Result<isize, u32> {
        register_class();
        unsafe {
            // Every existing child of the parent, the webview included, has to
            // agree to clip against its siblings or it'll paint over the host
            // we're about to create.
            let mut sibling = GetWindow(parent as *mut core::ffi::c_void, GW_CHILD);
            while !sibling.is_null() {
                add_clip_siblings(sibling);
                sibling = GetWindow(sibling, GW_HWNDNEXT);
            }

            let hwnd = CreateWindowExW(
                0,
                class_name(),
                core::ptr::null(),
                WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
                0,
                0,
                0,
                0,
                parent as *mut core::ffi::c_void,
                core::ptr::null_mut(),
                GetModuleHandleW(core::ptr::null()),
                core::ptr::null(),
            );
            if hwnd.is_null() {
                return Err(windows_sys::Win32::Foundation::GetLastError());
            }
            Ok(hwnd as isize)
        }
    }

    /// Resize the host to the parent's full client area, then set its window
    /// region to `client rect - holes`.
    ///
    /// Where the region is clipped away the child window doesn't exist as far as
    /// the compositor cares, so the webview behind it shows through along with
    /// whatever HTML you positioned there, on top of full-size native content.
    pub fn apply(child: isize, parent: isize, holes: &[Hole]) {
        unsafe {
            let mut rc: RECT = core::mem::zeroed();
            GetClientRect(parent as *mut core::ffi::c_void, &mut rc);
            let right = rc.right.max(0);
            let bottom = rc.bottom.max(0);

            // Passing a null hwndInsertAfter without SWP_NOZORDER re-raises the
            // host to the top of the sibling z-order, so the WebView2 chrome
            // window can never end up above it.
            SetWindowPos(
                child as *mut core::ffi::c_void,
                core::ptr::null_mut(),
                0,
                0,
                right,
                bottom,
                SWP_SHOWWINDOW | SWP_NOACTIVATE,
            );

            let region = CreateRectRgn(0, 0, right, bottom);
            for h in holes {
                let (l, t) = (h.x.max(0), h.y.max(0));
                let (r, b) = ((h.x + h.w).min(right), (h.y + h.h).min(bottom));
                if r > l && b > t {
                    let hole = CreateRectRgn(l, t, r, b);
                    CombineRgn(region, region, hole, RGN_DIFF);
                    DeleteObject(hole as _);
                }
            }
            // SetWindowRgn takes ownership of `region` on success, so don't
            // delete it here. Doing so is a use-after-free the next time
            // Windows validates the window. It looks like a leak; it isn't.
            SetWindowRgn(child as *mut core::ffi::c_void, region, 1);
        }
    }

    pub fn destroy(child: isize) {
        unsafe {
            DestroyWindow(child as *mut core::ffi::c_void);
        }
    }

    pub fn alive(hwnd: isize) -> bool {
        unsafe { IsWindow(hwnd as *mut core::ffi::c_void) != 0 }
    }
}

#[cfg(not(windows))]
pub(crate) mod imp {
    use super::Hole;

    pub fn create_child(_parent: isize) -> Result<isize, u32> {
        Err(0)
    }
    pub fn apply(_child: isize, _parent: isize, _holes: &[Hole]) {}
    pub fn destroy(_child: isize) {}
    pub fn alive(_hwnd: isize) -> bool {
        false
    }
}
