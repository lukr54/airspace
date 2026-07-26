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

/// Clamp a hole to a client rect of `right` x `bottom`, as left/top/right/bottom.
///
/// `None` means there's nothing left to cut: the hole is empty, inverted, or
/// entirely outside the client area. Callers skip those rather than handing
/// Windows a degenerate rectangle.
pub(crate) fn clamp(h: &Hole, right: i32, bottom: i32) -> Option<(i32, i32, i32, i32)> {
    let l = h.x.max(0);
    let t = h.y.max(0);
    let r = h.x.saturating_add(h.w).min(right);
    let b = h.y.saturating_add(h.h).min(bottom);
    if r > l && b > t {
        Some((l, t, r, b))
    } else {
        None
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
                if let Some((l, t, r, b)) = super::clamp(h, right, bottom) {
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

// `create_child` is unreachable off Windows, since `create_host` returns
// UnsupportedPlatform before it would be called. Keep the stub anyway so the
// module has the same shape on every target.
#[cfg(not(windows))]
#[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use super::{clamp, Hole};

    const W: i32 = 1100;
    const H: i32 = 720;

    #[test]
    fn hole_fully_inside_is_unchanged() {
        assert_eq!(
            clamp(&Hole::new(200, 120, 400, 160), W, H),
            Some((200, 120, 600, 280))
        );
    }

    #[test]
    fn negative_origin_is_clamped_to_the_client() {
        assert_eq!(
            clamp(&Hole::new(-50, -20, 100, 60), W, H),
            Some((0, 0, 50, 40))
        );
    }

    #[test]
    fn overflow_is_clamped_to_the_client() {
        assert_eq!(
            clamp(&Hole::new(1000, 700, 400, 400), W, H),
            Some((1000, 700, W, H))
        );
    }

    #[test]
    fn hole_covering_everything_is_the_whole_client() {
        assert_eq!(
            clamp(&Hole::new(0, 0, 9999, 9999), W, H),
            Some((0, 0, W, H))
        );
    }

    #[test]
    fn holes_with_nothing_left_to_cut_are_skipped() {
        // entirely off each edge
        assert_eq!(clamp(&Hole::new(W, 10, 50, 50), W, H), None);
        assert_eq!(clamp(&Hole::new(10, H, 50, 50), W, H), None);
        assert_eq!(clamp(&Hole::new(-100, 10, 50, 50), W, H), None);
        assert_eq!(clamp(&Hole::new(10, -100, 50, 50), W, H), None);
        // degenerate and inverted
        assert_eq!(clamp(&Hole::new(10, 10, 0, 50), W, H), None);
        assert_eq!(clamp(&Hole::new(10, 10, 50, 0), W, H), None);
        assert_eq!(clamp(&Hole::new(10, 10, -50, -50), W, H), None);
    }

    #[test]
    fn absurd_sizes_do_not_overflow() {
        assert_eq!(
            clamp(&Hole::new(10, 10, i32::MAX, i32::MAX), W, H),
            Some((10, 10, W, H))
        );
        assert_eq!(clamp(&Hole::new(i32::MAX, i32::MAX, 10, 10), W, H), None);
    }

    // The worked example from the README, checked against real Windows rather
    // than asserted in prose: a 1100x720 client with one 400x160 hole at
    // (200,120) must clip to exactly four rectangles.
    #[cfg(windows)]
    #[test]
    fn region_is_client_rect_minus_hole() {
        use super::imp;
        use std::mem::{size_of, zeroed};
        use windows_sys::Win32::Foundation::RECT;
        use windows_sys::Win32::Graphics::Gdi::{
            CreateRectRgn, DeleteObject, GetRegionData, GetWindowRgn, RGNDATA,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, GetClientRect, WS_POPUP,
        };

        // WS_POPUP with the predefined STATIC class: no frame, so the client
        // rect is exactly the size we ask for.
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let parent = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                core::ptr::null(),
                WS_POPUP,
                0,
                0,
                W,
                H,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                core::ptr::null(),
            )
        };
        assert!(!parent.is_null(), "could not create the test parent window");

        let mut rc: RECT = unsafe { zeroed() };
        unsafe { GetClientRect(parent, &mut rc) };
        assert_eq!((rc.right, rc.bottom), (W, H), "unexpected client size");

        let child = imp::create_child(parent as isize).expect("create_child");
        imp::apply(child, parent as isize, &[Hole::new(200, 120, 400, 160)]);

        // Read the region back the way tools/verify-region.ps1 does.
        let rgn = unsafe { CreateRectRgn(0, 0, 1, 1) };
        assert_ne!(
            unsafe { GetWindowRgn(child as *mut core::ffi::c_void, rgn) },
            0,
            "no window region was set"
        );
        let size = unsafe { GetRegionData(rgn, 0, core::ptr::null_mut()) } as usize;
        let mut buf = vec![0u8; size];
        unsafe { GetRegionData(rgn, size as u32, buf.as_mut_ptr() as *mut RGNDATA) };

        let count = u32::from_le_bytes(buf[8..12].try_into().unwrap()) as usize;
        let header = size_of::<windows_sys::Win32::Graphics::Gdi::RGNDATAHEADER>();
        let mut got: Vec<(i32, i32, i32, i32)> = (0..count)
            .map(|i| {
                let o = header + i * 16;
                let g = |k: usize| i32::from_le_bytes(buf[o + k..o + k + 4].try_into().unwrap());
                (g(0), g(4), g(8), g(12))
            })
            .collect();
        got.sort();

        let mut want = vec![
            (0, 0, W, 120),
            (0, 120, 200, 280),
            (600, 120, W, 280),
            (0, 280, W, H),
        ];
        want.sort();

        unsafe {
            DeleteObject(rgn as _);
            imp::destroy(child);
            DestroyWindow(parent);
        }

        assert_eq!(
            got, want,
            "region rectangles did not match client minus hole"
        );
    }
}
