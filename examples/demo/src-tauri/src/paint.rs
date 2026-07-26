//! Renders animated content into the native host with a D3D11 swapchain.
//!
//! # Why a swapchain and not GDI?
//!
//! Because it's what the real cases use (mpv, any D3D renderer), so the demo
//! looks like the thing it's demonstrating.
//!
//! Not because GDI is broken. The first version of this painted with `GetDC` +
//! `FillRect` and was invisible, which sent me hunting for a compositing rule
//! that doesn't exist. The actual cause was the missing `WS_CLIPSIBLINGS` on
//! the host and the webview. With that set, GDI painting shows up fine. You
//! don't need D3D to make hole-punching work.
//!
//! `AIRSPACE_DEMO_FLIP=1` builds the swapchain with `FLIP_DISCARD` instead of
//! `DISCARD`, so you can check on your own hardware whether the presentation
//! path still honours `SetWindowRgn`. It is here because mpv's gpu-next flip
//! path does *not* honour it (hence `--d3d11-flip=no`), and it seemed likely to
//! be a property of the flip model. Measured on this stack it isn't: both swap
//! effects respected the region, same ten region rectangles, all three overlays
//! visible. So treat "does my renderer honour the region?" as a per-renderer
//! question to test, not something to predict from the swap effect. The startup
//! line prints which effect is live so the answer is never ambiguous.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Bumped on every start/stop so a previous renderer exits.
pub static EPOCH: AtomicUsize = AtomicUsize::new(0);

pub fn stop() {
    EPOCH.fetch_add(1, Ordering::SeqCst);
}

#[cfg(not(windows))]
pub fn spawn(_hwnd: isize) {}

#[cfg(windows)]
pub fn spawn(hwnd: isize) {
    let epoch = EPOCH.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        if let Err(e) = render_loop(hwnd, epoch) {
            eprintln!("[paint] renderer stopped: {e}");
        }
    });
}

#[cfg(windows)]
fn render_loop(hwnd: isize, epoch: usize) -> windows::core::Result<()> {
    use windows::core::Interface;
    use windows::Win32::Foundation::{HMODULE, HWND, RECT};
    use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL};
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDeviceAndSwapChain, ID3D11Device, ID3D11DeviceContext, ID3D11DeviceContext1,
        ID3D11RenderTargetView, ID3D11Texture2D, D3D11_CREATE_DEVICE_FLAG, D3D11_SDK_VERSION,
    };
    use windows::Win32::Graphics::Dxgi::Common::{
        DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_DESC, DXGI_SAMPLE_DESC,
    };
    use windows::Win32::Graphics::Dxgi::{
        IDXGISwapChain, DXGI_SWAP_CHAIN_DESC, DXGI_SWAP_EFFECT_DISCARD,
        DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, IsWindow};

    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    let flip = std::env::var("AIRSPACE_DEMO_FLIP").as_deref() == Ok("1");

    let mut client = RECT::default();
    unsafe { GetClientRect(hwnd, &mut client)? };
    let (mut w, mut h) = (client.right.max(1) as u32, client.bottom.max(1) as u32);

    let desc = DXGI_SWAP_CHAIN_DESC {
        BufferDesc: DXGI_MODE_DESC {
            Width: w,
            Height: h,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            ..Default::default()
        },
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        // Blt model, one buffer. See AIRSPACE_DEMO_FLIP above.
        BufferCount: if flip { 2 } else { 1 },
        OutputWindow: hwnd,
        Windowed: true.into(),
        SwapEffect: if flip {
            DXGI_SWAP_EFFECT_FLIP_DISCARD
        } else {
            DXGI_SWAP_EFFECT_DISCARD
        },
        ..Default::default()
    };

    let mut swapchain: Option<IDXGISwapChain> = None;
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    unsafe {
        D3D11CreateDeviceAndSwapChain(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_FLAG(0),
            None,
            D3D11_SDK_VERSION,
            Some(&desc),
            Some(&mut swapchain),
            Some(&mut device),
            Some(&mut D3D_FEATURE_LEVEL::default()),
            Some(&mut context),
        )?
    };
    println!(
        "[paint] swapchain created: {} ({}x{}, {} buffer(s))",
        if flip {
            "DXGI_SWAP_EFFECT_FLIP_DISCARD"
        } else {
            "DXGI_SWAP_EFFECT_DISCARD"
        },
        w,
        h,
        desc.BufferCount
    );
    let swapchain = swapchain.expect("swapchain");
    let device = device.expect("device");
    let context: ID3D11DeviceContext1 = context.expect("context").cast()?;

    let make_rtv = |sc: &IDXGISwapChain| -> windows::core::Result<ID3D11RenderTargetView> {
        let back: ID3D11Texture2D = unsafe { sc.GetBuffer(0) }?;
        let mut rtv = None;
        unsafe { device.CreateRenderTargetView(&back, None, Some(&mut rtv))? };
        Ok(rtv.expect("rtv"))
    };
    let mut rtv = make_rtv(&swapchain)?;

    let mut frame: u32 = 0;
    loop {
        if EPOCH.load(Ordering::SeqCst) != epoch {
            break;
        }
        unsafe {
            if !IsWindow(Some(hwnd)).as_bool() {
                break;
            }
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let (nw, nh) = (rc.right.max(1) as u32, rc.bottom.max(1) as u32);
            if nw != w || nh != h {
                w = nw;
                h = nh;
                drop(std::mem::replace(&mut rtv, {
                    context.ClearState();
                    swapchain.ResizeBuffers(
                        0,
                        w,
                        h,
                        DXGI_FORMAT_B8G8R8A8_UNORM,
                        Default::default(),
                    )?;
                    make_rtv(&swapchain)?
                }));
            }

            // Background.
            context.ClearRenderTargetView(&rtv, &[0.047, 0.055, 0.078, 1.0]);

            // Moving bars via ClearView rects, so there are no shaders,
            // vertex buffers or assets involved.
            let bar_w = (w as i32 / 14).max(8);
            for i in 0..16 {
                let span = w as i32 + bar_w * 2;
                let x = ((frame as i32 * 4) + i * (bar_w + 26)) % span - bar_w;
                let rect = RECT {
                    left: x.max(0),
                    top: 0,
                    right: (x + bar_w).min(w as i32),
                    bottom: h as i32,
                };
                if rect.right <= rect.left {
                    continue;
                }
                let t = i as f32 / 16.0;
                context.ClearView(
                    &rtv,
                    &[1.0 * t, 0.35, 0.1 + 0.5 * (1.0 - t), 1.0],
                    Some(&[rect]),
                );
            }

            swapchain.Present(1, Default::default()).ok()?;
        }
        frame = frame.wrapping_add(1);
    }
    Ok(())
}
