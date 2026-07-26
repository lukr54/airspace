# tauri-plugin-airspace

Render native content under or alongside HTML in a Tauri v2 window on Windows.

Video under your HTML controls at full size, or a click-through HUD over a
fullscreen game you don't own.

```toml
[dependencies]
tauri-plugin-airspace = "0.1"
```

Windows only. Elsewhere every call returns `Error::UnsupportedPlatform` rather
than failing to compile, so cross-platform apps can keep one code path.

Everything below was measured on Windows 11 (build 26200), Tauri 2.11.5,
wry 0.55.1, WebView2 runtime 150.0.4078.99, Rust 1.97.1.

---

## The constraint

"Airspace" is the WPF/WinForms name for a plain Win32 rule:

> A windowed child HWND owns its rectangle.

It isn't a layer in a composited tree. It's a hole in the desktop that other code
paints into. Two sibling child HWNDs can't alpha-blend with each other, and
z-order applies to whole rectangles.

Everything else follows from that.

## Why transparency doesn't work

The obvious plan is to put the video in a child window, put a transparent
WebView2 on top, and let the HTML float over it. Most forum threads suggest it.

It can't work. Tauri (and Electron, and anything using windowed WebView2
hosting) hosts the webview as a windowed child HWND. Transparent web pixels don't
reveal the sibling window underneath, they reveal whatever is behind the whole
top-level window, which is usually the desktop. You get a hole straight through
your app.

There's no flag for it. It isn't a Tauri limitation or a WebView2 bug awaiting a
fix. It's what "windowed child HWND" means.

Two approaches do work, depending on whether the native content is inside your
window or in someone else's.

## 1. Native content inside your window

### Reserved insets, the version most apps ship

Size the native child to client area minus a strip, and put your controls in the
strip.

This works, it's simple, and it costs you one thing: the native content shrinks
to make room for the UI. With a static control bar you may not care. As soon as
you add transient overlays (a skip-intro button, an up-next card, a title that
fades in) the video visibly changes aspect every time one appears. For a media
player that's disqualifying.

### Hole-punching

Cover the whole client area with the opaque native child, give its `HWND` to
whatever renders, then cut the controls out of that window's region:

```rust
let region = CreateRectRgn(0, 0, right, bottom);
for h in holes {
    let hole = CreateRectRgn(h.left, h.top, h.right, h.bottom);
    CombineRgn(region, region, hole, RGN_DIFF);
    DeleteObject(hole as _);
}
// SetWindowRgn takes ownership on success. Freeing `region` here is a
// use-after-free the next time Windows validates the window.
SetWindowRgn(child, region, 1);
```

Where the region is clipped away the child window doesn't exist as far as the
compositor cares, so the webview shows through with your HTML on top of full-size
native content.

With the plugin:

```rust
use tauri_plugin_airspace::{AirspaceExt, Hole};

tauri::Builder::default()
    .plugin(tauri_plugin_airspace::init())
    .setup(|app| {
        let hwnd = app.airspace().create_host("main")?;
        // give `hwnd` to your renderer: mpv --wid=<hwnd>, your D3D swapchain, ...
        app.airspace().set_holes("main", vec![Hole::new(20, 800, 900, 76)])?;
        Ok(())
    })
```

From JS, after granting `airspace:default` in a capability:

```js
const { invoke } = window.__TAURI__.core;
const r = el.getBoundingClientRect();
const s = window.devicePixelRatio;          // holes are physical pixels
await invoke('plugin:airspace|set_holes', {
  holes: [{ x: r.x*s|0, y: r.y*s|0, w: r.width*s|0, h: r.height*s|0 }],
});
```

Forget `devicePixelRatio` and your holes land in the wrong place on any display
that isn't at 100% scaling. CSS gives you logical pixels, `SetWindowRgn` wants
physical ones.

The host is re-fitted on window resize automatically. Holes anchored to a window
edge move, so re-push them after a resize.

## 2. Native content in another process

For a fullscreen game or another process's window, hole-punching doesn't apply,
since you can't clip a window you don't own. Use a separate top-level window
instead: transparent, always-on-top, click-through.

```rust
use tauri_plugin_airspace::OverlayBuilder;
use tauri::WebviewUrl;

OverlayBuilder::new("hud", WebviewUrl::App("overlay.html".into()))
    .size(430.0, 152.0)
    .build(app.handle())?;
```

`OverlayBuilder` sets the flag combination that works. Four things about that
window on Windows:

- **`transparent: true` with `decorations: true` is broken.** You get an empty
  see-through box, because WebView2 won't composite a transparent surface behind
  a native title bar. It's also intermittent, which is worse: it paints correctly
  on some launches. The valid combinations are `transparent` with
  `decorations:false`, or `transparent:false` with `decorations:true`.
- **Create it visible, not hidden then shown.** A transparent WebView2 window
  built hidden and shown later stays blank, never composited, and nudging doesn't
  help. Build it visible and keep the content hidden until you have something to
  draw. It's transparent, so that's invisible anyway.
- **`decorations:false` with `resizable:true` leaves invisible resize borders**
  that users grab when they meant to drag. Make it non-resizable and move it with
  `start_dragging()`.
- **Always-on-top loses to exclusive fullscreen.** Nothing to be done, document
  it and tell users to run borderless-windowed.

---

## WS_CLIPSIBLINGS

This is the part that isn't in any of the existing answers, and the reason the
crate exists.

I built the host as described. Then:

- `GetRegionData` on the host returned exactly `client rect - hole`. For a
  1100x720 client with one 400x160 hole at (200,120): four rectangles, the band
  above, left of, right of and below the hole. Correct.
- The z-order walk (`GW_CHILD` then `GW_HWNDNEXT`) put `tauri_airspace_host`
  above `WRY_WEBVIEW`.
- `WindowFromPoint` outside the hole returned `tauri_airspace_host`. Inside the
  hole it returned `Chrome_RenderWidgetHostHWND`, so the hole really was exposing
  the webview to input.

And not one pixel of the host was on screen.

I went through the obvious suspects. GDI painting: invisible. A D3D11 blt-model
swapchain: invisible. Created during `setup()` versus three seconds after the app
was up: invisible either way. mpv, cross-process, embedded with `--wid` exactly
as a real player does it: invisible. Hiding `WRY_WEBVIEW` showed the host had
been rendering perfectly the whole time, animated content and all, with the hole
as a clean cut-out.

So the host was rendering, on top, winning hit-tests, and losing every pixel to
the webview.

The cause is a Win32 rule older than all of this:

> Overlapping sibling windows without `WS_CLIPSIBLINGS` are undefined. Each one
> can paint over the others.

wry creates `WRY_WEBVIEW` without `WS_CLIPSIBLINGS`. If you create your host
without it too, which is the obvious thing to do since none of the examples
mention it, you have two overlapping siblings that never agreed to clip against
each other, and WebView2 wins.

Set the style on the host and on every sibling it overlaps:

```rust
let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
SetWindowLongPtrW(hwnd, GWL_STYLE, style | WS_CLIPSIBLINGS as isize);
SetWindowPos(hwnd, null_mut(), 0,0,0,0,
    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
```

`SWP_FRAMECHANGED` matters, the style cache only refreshes on a frame change.
With the style on both windows it worked on the first frame. The plugin does this
for you when it creates the host.

### If yours works without this

Check whether it really does. Content that presents continuously, like a video
player at 60fps, repaints often enough to win the race most of the time, so an
implementation missing the style can look correct for months. Whether it works
depends on your frame rate, the user's refresh rate and how often the page
repaints. That's how my own media player shipped for months.

---

## Other things that each cost a day

**No holes means no input**, mouse or keyboard. The opaque host owns the entire
client area, so an auto-hiding control bar can't be brought back by a mouse move
(the webview never sees it), clicking the picture gives keyboard focus to the
host so the page stops seeing keys, and in fullscreen with no titlebar there's no
way out at all. Anything the user genuinely needs wants a native fallback: a
background thread polling `GetCursorPos` and key state, emitting events the page
can act on. That thread's lifetime has to key on the host window existing
(`IsWindow`), not on whether your player thinks it's playing. Tie it to the wrong
thing and it dies early, and the app is a locked box.

**Don't gate transient overlays on the main bar's visibility.** If the bar hides
after 3s and skip-intro becomes eligible at 5s, gating one on the other means it
never appears. Give each transient control its own hole.

**Anything still visible in the DOM shows through the holes.** "Hidden behind the
video" isn't a thing when native content is on top. A detail page you forgot to
hide will peek around the video.

**If native content paints over your holes**, its presentation path is skipping
the window region. That's a property of the renderer rather than of the flip model
in general. mpv's gpu-next flip path does it until `--d3d11-flip=no`, while a
`DXGI_SWAP_EFFECT_FLIP_DISCARD` swapchain created directly on the host respected
the region when measured here. `AIRSPACE_DEMO_FLIP=1` in the demo switches the
swap effect so you can check your own hardware.

**Don't build webview windows from a command.**
`WebviewWindowBuilder::build()` dispatches to the event loop and waits, and a
sync command holds a lock the event loop needs, so it can hang forever: window
created, never navigates, white and uncloseable. See
[tauri#12521](https://github.com/tauri-apps/tauri/issues/12521), still open, where
a maintainer's diagnosis is that plugins are locked for command invocation while
the event loop needs the same lock. Build overlays in `setup()`, or use
`OverlayBuilder::spawn`. The same applies to the host window, which has to be
created on the thread that owns the parent since a child window's messages go to
its creating thread. Dispatch to the main thread, but check whether you're already
on it first or `setup()` queues work behind the thread waiting for it.

**If `listen()` never fires in a second window**, that isn't a Tauri bug. The
window just isn't covered by a capability granting `core:event`. Your own commands
keep working, since app commands aren't permission-gated by default, which makes
it look like events don't reach runtime-created windows. Add the label to the
capability's `windows` array.

## Checking your own implementation

Don't rely on a screenshot alone, and treat `PrintWindow` on WebView2 windows
with suspicion: with `PW_RENDERFULLCONTENT` it can force the webview to render
into the capture, so a healthy-looking capture isn't proof the screen is healthy.
That cost me a day on an unrelated blank-window bug where PrintWindow "passed"
twelve times while the screen stayed white. For the hole-punch it happened to be
accurate, but it isn't independent evidence.

Read the region back instead:

```
GetWindowRgn(host, rgn);
GetRegionData(rgn, size, buf);   // RGNDATAHEADER.nCount, then nCount RECTs
```

For a 1100x720 client with a 400x160 hole at (200,120) you should get exactly
four rectangles: `(0,0)-(1100,120)`, `(0,120)-(200,280)`, `(600,120)-(1100,280)`,
`(0,280)-(1100,720)`. That's what Windows will clip to, whether or not anything
is painting. `tools/verify-region.ps1` in the repo does this against the demo.

Then check compositing separately with `WindowFromPoint`: outside a hole it
should return your host, inside a hole the webview's render widget. If the
geometry is right and the pixels are still wrong, look at `WS_CLIPSIBLINGS`.

## Summary

1. A transparent webview will never composite over a sibling native window. Stop
   looking for the flag.
2. Native content inside your window: full-size opaque child plus a
   `SetWindowRgn` hole-punch. Insets are the easy version and they deform your
   content.
3. Native content you don't own: separate transparent, click-through,
   always-on-top window.
4. Set `WS_CLIPSIBLINGS` on the host and every sibling it overlaps, or you get a
   window that's present, on top, hit-testable and invisible.
5. Check whether your renderer honours the region, and try a non-flip
   presentation if it doesn't.
6. Native content on top means the web layer is blind. Give every input path a
   native fallback.
7. Verify with `GetRegionData` and `WindowFromPoint`, not screenshots.

## Examples

```bash
cargo run -p airspace-demo
```

No external dependencies. The "native content" is a D3D11 swapchain the demo
draws itself. Click *Start native content* and toggle the overlays.

```bash
cargo run -p airspace-mpv -- "C:\path\to\video.mkv"
```

The real case: mpv playing a file at full size with an HTML control bar over it.
Needs mpv on `PATH`, or set `MPV` to its full path.

Worth reading if you're wiring up mpv yourself, because three parts of it are
easy to get wrong:

- **The flags.** `--no-osc --no-osd-bar --input-default-bindings=no
  --input-vo-keyboard=no` so mpv stops competing with your page for input, and
  `--d3d11-flip=no` because gpu-next's flip presentation ignores the window
  region and paints over your holes.
- **Two IPC connections, not one.** One reads events, one writes commands.
  Sharing a single handle between a blocking reader and a writer crashes on the
  first command.
- **Drain the write connection.** mpv sends command replies to every connected
  client. Nothing reads that connection, so its pipe buffer fills, mpv blocks
  writing to it, and then it stops reading your commands. Every control dies
  while position updates keep arriving, so it looks like a UI bug.

It also has the cursor and key poller the input warning above is about, which is
what lets the auto-hidden bar come back once there are no holes left.

## Licence

MIT OR Apache-2.0.
