const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (s) => document.querySelector(s);

// Anything that has to stay visible over the video needs its own hole. The bar
// is in here for a practical reason: without a hole over it, none of these
// buttons would be clickable once mpv covers the window.
const OVERLAYS = ["#bar", "#title"];

let playing = false;
let duration = 0;
let position = 0;
let scrubbing = false;
let hideTimer = null;

function pushHoles() {
  if (!playing) return;
  const s = window.devicePixelRatio;
  const holes = [];
  for (const sel of OVERLAYS) {
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
  $("#holes").textContent = holes.map((h) => `${h.w}x${h.h}@${h.x},${h.y}`).join("  ");
  invoke("plugin:airspace|set_holes", { holes });
}

function fmt(secs) {
  if (!isFinite(secs) || secs < 0) secs = 0;
  const m = Math.floor(secs / 60);
  const s = Math.floor(secs % 60);
  const h = Math.floor(m / 60);
  const mm = h > 0 ? String(m % 60).padStart(2, "0") : String(m);
  return (h > 0 ? `${h}:` : "") + `${mm}:${String(s).padStart(2, "0")}`;
}

function showBar() {
  $("#bar").classList.remove("off");
  $("#title").classList.remove("off");
  pushHoles();
  clearTimeout(hideTimer);
  hideTimer = setTimeout(hideBar, 3000);
}

function hideBar() {
  if (!playing || scrubbing) return;
  $("#bar").classList.add("off");
  $("#title").classList.add("off");
  // No holes left now, so the page stops seeing the mouse. The backend poller
  // is the only thing that can bring this back.
  pushHoles();
}

async function start(path) {
  $("#err").textContent = "";
  try {
    await invoke("open", { path });
  } catch (e) {
    $("#err").textContent = String(e);
    return;
  }
  playing = true;
  duration = 0;
  position = 0;
  $("#picker").classList.add("off");
  $("#title-text").textContent = path.split(/[\\/]/).pop();
  showBar();
}

async function stop() {
  clearTimeout(hideTimer);
  playing = false;
  await invoke("close");
  $("#bar").classList.add("off");
  $("#title").classList.add("off");
  $("#picker").classList.remove("off");
  $("#holes").textContent = "";
}

$("#play").addEventListener("click", () => {
  const path = $("#path").value.trim().replace(/^"|"$/g, "");
  if (path) start(path);
});
$("#path").addEventListener("keydown", (e) => {
  if (e.key === "Enter") $("#play").click();
});

$("#pause").addEventListener("click", () => invoke("toggle_pause"));
$("#back").addEventListener("click", () => invoke("seek_relative", { secs: -10 }));
$("#fwd").addEventListener("click", () => invoke("seek_relative", { secs: 30 }));
$("#stop").addEventListener("click", stop);
$("#vol").addEventListener("input", (e) => invoke("set_volume", { level: +e.target.value }));

$("#scrub").addEventListener("pointerdown", () => (scrubbing = true));
$("#scrub").addEventListener("change", (e) => {
  scrubbing = false;
  if (duration > 0) invoke("seek", { secs: (+e.target.value / 1000) * duration });
  showBar();
});

listen("mpv-pos", (e) => {
  position = e.payload;
  if (!scrubbing && duration > 0) {
    $("#scrub").value = Math.round((position / duration) * 1000);
  }
  $("#time").textContent = `${fmt(position)} / ${fmt(duration)}`;
});

listen("mpv-dur", (e) => {
  duration = e.payload;
  $("#time").textContent = `${fmt(position)} / ${fmt(duration)}`;
});

listen("mpv-paused", (e) => {
  $("#pause").textContent = e.payload ? "Play" : "Pause";
  if (e.payload) showBar();
});

listen("mpv-ended", stop);

// The two events from the backend poller. Without these the bar could never
// come back, because with it hidden there is no hole for the webview to see a
// mouse move through.
listen("activity", showBar);
listen("key", (e) => {
  if (e.payload === "Escape") stop();
  if (e.payload === "Space") {
    invoke("toggle_pause");
    showBar();
  }
});

window.addEventListener("resize", pushHoles);
window.addEventListener("mousemove", showBar);

invoke("initial_path").then((p) => {
  if (p) {
    $("#path").value = p;
    start(p);
  }
});
