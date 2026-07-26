const { invoke } = window.__TAURI__.core;

const $ = (s) => document.querySelector(s);
const statusEl = $("#status");
const holesEl = $("#holes");

// Each element that needs to be visible over the native content gets its own
// hole. The control bar is in the list because otherwise starting the native
// content would make the whole UI unclickable: with no holes the webview gets
// no input.
const OVERLAYS = ["#bar", "#title", "#skip"];

let running = false;
let hudOn = false;
let barHideTimer = null;

// Measure the visible overlays and push them as physical-pixel rects.
function pushHoles() {
  if (!running) return;
  const s = window.devicePixelRatio; // CSS px -> physical px. Forget this and
  const holes = [];                  // your holes land in the wrong place at
  for (const sel of OVERLAYS) {      // any scaling other than 100%.
    const el = $(sel);
    if (!el || el.classList.contains("off")) continue;
    const r = el.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) continue;
    holes.push({
      x: Math.round(r.x * s),
      y: Math.round(r.y * s),
      w: Math.round(r.width * s),
      h: Math.round(r.height * s),
    });
  }
  holesEl.textContent = holes
    .map((h) => `${h.w}x${h.h}@${h.x},${h.y}`)
    .join("  ");
  invoke("plugin:airspace|set_holes", { holes });
}

function enterRunning(label) {
  running = true;
  $("#toggle-native").textContent = "Stop native content";
  $("#toggle-native").classList.remove("primary");
  for (const id of ["#toggle-title", "#toggle-skip", "#toggle-bar"]) {
    $(id).disabled = false;
  }
  statusEl.textContent = label;
  $("#title").classList.remove("off");
  $("#skip").classList.remove("off");
  pushHoles();
}

// The autostart path creates the host from Rust and hands hole management back
// to this page, so the JS side gets exercised too.
window.__TAURI__.event.listen("native-started", () => {
  enterRunning("native host started by autostart, holes pushed from JS");
});

$("#toggle-native").addEventListener("click", async () => {
  if (!running) {
    try {
      const hwnd = await invoke("start_native");
      enterRunning(
        `native host hwnd 0x${hwnd.toString(16)}, covering the whole client area`,
      );
    } catch (e) {
      statusEl.textContent = `failed: ${e}`;
    }
  } else {
    await invoke("stop_native");
    running = false;
    clearTimeout(barHideTimer);
    $("#toggle-native").textContent = "Start native content";
    $("#toggle-native").classList.add("primary");
    for (const id of ["#toggle-title", "#toggle-skip", "#toggle-bar"]) {
      $(id).disabled = true;
    }
    $("#title").classList.add("off");
    $("#skip").classList.add("off");
    $("#bar").classList.remove("off");
    statusEl.textContent = "native content stopped";
    holesEl.textContent = "";
  }
});

$("#toggle-title").addEventListener("click", () => {
  $("#title").classList.toggle("off");
  pushHoles();
});

$("#toggle-skip").addEventListener("click", () => {
  $("#skip").classList.toggle("off");
  pushHoles();
});

// Hides the bar for 3 seconds. With it gone there are fewer holes, and if you
// hid every overlay the webview would stop receiving mouse and keyboard
// entirely. That's why a real app wants a native fallback (a cursor/key poller)
// to get its UI back.
$("#toggle-bar").addEventListener("click", () => {
  $("#bar").classList.add("off");
  pushHoles();
  clearTimeout(barHideTimer);
  barHideTimer = setTimeout(() => {
    $("#bar").classList.remove("off");
    pushHoles();
  }, 3000);
});

$("#toggle-hud").addEventListener("click", async () => {
  hudOn = !hudOn;
  await invoke("toggle_overlay", { on: hudOn });
  $("#toggle-hud").textContent = hudOn ? "Hide overlay window" : "Overlay window";
});

$("#skip").addEventListener("click", () => {
  $("#skip").classList.add("off");
  pushHoles();
});

// The plugin re-fits the host to the new client size, but holes anchored to a
// window edge move, so re-push them.
window.addEventListener("resize", pushHoles);
