"use strict";

const tauri = window.__TAURI__;
const invoke = tauri ? tauri.core.invoke : demoInvoke;
const listen = tauri ? tauri.event.listen : () => Promise.resolve();

const $ = (id) => document.getElementById(id);

const MINUS = "−";
const DEVICE_REFRESH_MS = 5000;
const STATUS_POLL_MS = 100;

const STATE_LABELS = {
  idle: "Waiting for source",
  starting: "Starting",
  buffering: "Buffering",
  playing: "Playing",
  error: "Unavailable",
  blocked: "Skipped",
};

let snapshot = null;
let status = null;
const rows = new Map();
const meters = new Map();

// OBS logarithmic fader curve, same as the Rust engine.
function faderToDb(def) {
  if (def >= 1) return 0;
  if (def <= 0) return -Infinity;
  return -102 * Math.pow(17, -def) + 6;
}

function formatDb(db) {
  if (!Number.isFinite(db) || db <= -96) return `${MINUS}∞ dB`;
  const abs = Math.abs(db).toFixed(1);
  return db < -0.05 ? `${MINUS}${abs} dB` : `${abs} dB`;
}

function peakToDb(peak) {
  return peak > 0 ? 20 * Math.log10(peak) : -Infinity;
}

async function call(cmd, args) {
  try {
    return await invoke(cmd, args);
  } catch (err) {
    setRunState(String(err), "error");
    throw err;
  }
}

function setRunState(text, tone) {
  const el = $("run-state");
  if (el.textContent !== text) el.textContent = text;
  el.dataset.tone = tone;
}

/* Loading */

async function load() {
  snapshot = await call("snapshot");
  $("version").textContent = `v${snapshot.version}`;
  $("autostart").checked = snapshot.autostart;
  showUpdate(snapshot.update_ready);
  renderSource();
  renderOutputs();
}

/** Re-reads devices, but never while the user is holding a control. */
async function refresh() {
  if (document.hidden) return;
  // Rebuilding the list under an open menu or a dragged slider would drop it.
  if (document.activeElement?.matches("select, input[type=range]")) return;
  try {
    const next = await invoke("snapshot");
    const changed = JSON.stringify(next.devices) !== JSON.stringify(snapshot.devices);
    snapshot = { ...next, config: snapshot.config };
    showUpdate(next.update_ready);
    if (changed) {
      renderSource();
      renderOutputs();
    }
  } catch {
    // The next tick tries again.
  }
}

function renderSource() {
  const select = $("source");
  const { sources } = snapshot.devices;
  const groups = [
    ["desktop", null],
    ["loopback", "Output devices"],
    ["capture", "Input devices"],
  ];
  select.replaceChildren();
  for (const [kind, label] of groups) {
    const items = sources.filter((s) => s.kind === kind);
    if (!items.length) continue;
    const parent = label ? document.createElement("optgroup") : select;
    if (label) parent.label = label;
    for (const s of items) {
      const opt = document.createElement("option");
      opt.value = s.id;
      opt.textContent = s.is_default ? `${s.name} (default)` : s.name;
      parent.append(opt);
    }
    if (label) select.append(parent);
  }
  const current = snapshot.config.source;
  if (!sources.some((s) => s.id === current)) {
    const opt = document.createElement("option");
    opt.value = current;
    opt.textContent = "Disconnected device";
    select.prepend(opt);
  }
  select.value = current;
}

function outputConfig(id) {
  return snapshot.config.outputs.find((o) => o.id === id);
}

function enabledCount() {
  return snapshot.config.outputs.filter((o) => o.enabled).length;
}

/** Output captured by the current source: playing into it would feed back. */
function capturedOutput() {
  const src = snapshot.config.source;
  if (src === "desktop") return snapshot.devices.desktop_output;
  if (src.startsWith("loopback:")) return src.slice("loopback:".length);
  if (src.startsWith("pulseaudio:") && src.endsWith(".monitor")) return src.slice(0, -".monitor".length);
  return null;
}

function renderOutputs() {
  const list = $("outputs");
  const present = snapshot.devices.outputs;
  const absent = snapshot.config.outputs
    .filter((o) => o.enabled && !present.some((d) => d.id === o.id))
    .map((o) => ({ id: o.id, name: o.name || "Unknown device", absent: true }));
  const all = [...present, ...absent];

  list.replaceChildren();
  rows.clear();
  meters.clear();

  if (!all.length) {
    const p = document.createElement("p");
    p.className = "row empty";
    p.textContent = "No output device found. Plug one in and it will show up here.";
    list.append(p);
    return;
  }

  if (!enabledCount()) {
    const p = document.createElement("p");
    p.className = "note";
    p.textContent = "Turn on the outputs that should play the source.";
    list.append(p);
  }

  const captured = capturedOutput();
  const tpl = $("output-row");
  all.forEach((dev, i) => {
    const node = tpl.content.firstElementChild.cloneNode(true);
    const cfg = outputConfig(dev.id) || { enabled: false, fader: 1, muted: false };
    const sw = node.querySelector(".output-enabled");
    const name = node.querySelector(".output-name");
    const fader = node.querySelector(".fader");
    const mute = node.querySelector(".output-mute");
    const meter = node.querySelector(".output-meter");

    node.dataset.id = dev.id;
    node.dataset.name = dev.name;
    node.dataset.enabled = String(cfg.enabled);
    node.dataset.absent = String(Boolean(dev.absent));
    sw.id = `output-${i}`;
    sw.checked = cfg.enabled;
    name.htmlFor = sw.id;
    name.textContent = dev.is_default ? `${dev.name} (default)` : dev.name;
    name.title = dev.name;
    fader.value = Math.round(cfg.fader * 1000);
    fader.setAttribute("aria-label", `${dev.name} volume`);
    meter.setAttribute("aria-label", `${dev.name} level`);
    setMuted(node, cfg.muted);
    updateFaderView(node, cfg.fader);

    if (dev.id === captured && !cfg.enabled) {
      sw.disabled = true;
      setDetail(node, "This is the source device. Playing into it would echo endlessly.");
    }

    if (dev.absent) {
      sw.disabled = true;
      const forget = document.createElement("button");
      forget.type = "button";
      forget.className = "btn";
      forget.textContent = "Forget";
      forget.dataset.action = "forget";
      node.querySelector(".output-head").append(forget);
      setState(node, "Not connected", "muted");
    }

    list.append(node);
    rows.set(dev.id, node);
    meters.set(dev.id, { el: meter.firstElementChild, meter, level: 0 });
    mute.setAttribute("aria-label", `Mute ${dev.name}`);
  });
  applyStatus();
}

function updateFaderView(node, def) {
  const fader = node.querySelector(".fader");
  const text = formatDb(faderToDb(def));
  fader.style.setProperty("--fill", `${def * 100}%`);
  fader.setAttribute("aria-valuetext", text);
  node.querySelector(".output-db").textContent = text;
}

function setMuted(node, muted) {
  const btn = node.querySelector(".output-mute");
  btn.setAttribute("aria-pressed", String(muted));
  btn.textContent = muted ? "Muted" : "Mute";
  node.dataset.muted = String(muted);
}

function setState(node, text, tone) {
  const el = node.querySelector(".output-state");
  if (el.textContent !== text) el.textContent = text;
  el.dataset.tone = tone;
}

function setDetail(node, text) {
  const el = node.querySelector(".output-detail");
  if (el.textContent !== (text || "")) el.textContent = text || "";
}

/* Engine status */

async function poll() {
  if (document.hidden || !snapshot) return;
  try {
    status = await invoke("status");
  } catch {
    return;
  }
  applyStatus();
}

function applyStatus() {
  const running = Boolean(status && status.running);
  const src = running ? status.source : null;

  if (!running) {
    setRunState("Off", "muted");
  } else if (src.state === "error") {
    setRunState("Source unavailable", "error");
  } else {
    const playing = status.outputs.filter((o) => o.state === "playing").length;
    const total = status.outputs.length;
    setRunState(playing === total ? `Mirroring to ${total}` : `${playing} of ${total} playing`, "on");
  }

  const err = $("source-error");
  const msg = src && src.state === "error" ? `${src.message}. Retrying.` : "";
  err.hidden = !msg;
  if (err.textContent !== msg) err.textContent = msg;
  pushLevel("__source", src ? src.peak : 0);

  const byId = new Map((running ? status.outputs : []).map((o) => [o.id, o]));
  for (const [id, node] of rows) {
    if (node.dataset.absent === "true") continue;
    const enabled = node.dataset.enabled === "true";
    const st = byId.get(id);
    if (!enabled || !st) {
      setState(node, "", "muted");
      if (!node.querySelector(".output-enabled").disabled) setDetail(node, "");
      pushLevel(id, 0);
      continue;
    }
    const muted = node.dataset.muted === "true";
    let label = STATE_LABELS[st.state] || st.state;
    let tone = st.state === "playing" ? "on" : "muted";
    if (st.state === "error") tone = "error";
    if (muted && st.state === "playing") {
      label = "Muted";
      tone = "muted";
    }
    setState(node, label, tone);

    let detail = "";
    if (st.state === "error") detail = `${st.message}. Retrying.`;
    if (st.state === "blocked") detail = "This is the source device. Playing into it would echo endlessly.";
    setDetail(node, detail);
    pushLevel(id, st.peak);
  }
}

/* Meters: instant peak, smooth fall */

const sourceMeter = { el: $("source-meter").firstElementChild, meter: $("source-meter"), label: $("source-db"), level: 0 };

function pushLevel(id, peak) {
  const m = id === "__source" ? sourceMeter : meters.get(id);
  if (!m) return;
  const db = peakToDb(peak);
  m.target = Number.isFinite(db) ? Math.min(1, Math.max(0, (db + 60) / 60)) : 0;
}

function animateMeters() {
  for (const m of [sourceMeter, ...meters.values()]) {
    const target = m.target || 0;
    m.level = target > m.level ? target : Math.max(target, m.level - 0.02);
    m.el.style.transform = `scaleX(${m.level.toFixed(3)})`;
    const db = m.level > 0 ? m.level * 60 - 60 : -Infinity;
    const now = Number.isFinite(db) ? String(Math.round(db)) : "-60";
    if (m.meter.getAttribute("aria-valuenow") !== now) m.meter.setAttribute("aria-valuenow", now);
    if (m.label) {
      const text = formatDb(db);
      if (m.label.textContent !== text) m.label.textContent = text;
    }
  }
  requestAnimationFrame(animateMeters);
}

/* Updates */

function showUpdate(version) {
  const restart = $("restart");
  restart.hidden = !version;
  $("version").hidden = Boolean(version);
  if (version) restart.title = `Version ${version} is installed`;
}

listen("update-ready", ({ payload }) => showUpdate(payload));

/* Actions */

$("source").addEventListener("change", async (e) => {
  await call("set_source", { id: e.target.value });
  snapshot.config.source = e.target.value;
  renderOutputs();
});

function localOutput(node) {
  const id = node.dataset.id;
  let o = outputConfig(id);
  if (!o) {
    o = { id, name: node.dataset.name, enabled: false, fader: 1, muted: false };
    snapshot.config.outputs.push(o);
  }
  return o;
}

$("outputs").addEventListener("change", async (e) => {
  if (!e.target.classList.contains("output-enabled")) return;
  const node = e.target.closest(".output");
  const enabled = e.target.checked;
  await call("set_output_enabled", { id: node.dataset.id, name: node.dataset.name, enabled });
  localOutput(node).enabled = enabled;
  const hadNote = Boolean($("outputs").querySelector(".note"));
  if (hadNote !== !enabledCount()) {
    renderOutputs();
    $("outputs").querySelector(`[data-id="${CSS.escape(node.dataset.id)}"] .output-enabled`)?.focus();
  } else {
    node.dataset.enabled = String(enabled);
    applyStatus();
  }
});

const pendingVolume = new Map();
$("outputs").addEventListener("input", (e) => {
  if (!e.target.classList.contains("fader")) return;
  const node = e.target.closest(".output");
  const id = node.dataset.id;
  const def = Number(e.target.value) / 1000;
  updateFaderView(node, def);
  localOutput(node).fader = def;
  // At most one call per frame while dragging.
  const queued = pendingVolume.has(id);
  pendingVolume.set(id, def);
  if (queued) return;
  requestAnimationFrame(() => {
    const fader = pendingVolume.get(id);
    pendingVolume.delete(id);
    call("set_output_volume", { id, name: node.dataset.name, fader });
  });
});

$("outputs").addEventListener("click", async (e) => {
  const btn = e.target.closest("button");
  if (!btn) return;
  const node = btn.closest(".output");
  const id = node.dataset.id;
  if (btn.classList.contains("output-mute")) {
    const muted = node.dataset.muted !== "true";
    await call("set_output_muted", { id, name: node.dataset.name, muted });
    localOutput(node).muted = muted;
    setMuted(node, muted);
    applyStatus();
  } else if (btn.dataset.action === "forget") {
    await call("forget_output", { id });
    snapshot.config.outputs = snapshot.config.outputs.filter((o) => o.id !== id);
    renderOutputs();
  }
});

$("autostart").addEventListener("change", async (e) => {
  try {
    e.target.checked = await call("set_autostart", { enabled: e.target.checked });
  } catch {
    e.target.checked = !e.target.checked;
  }
});

$("restart").addEventListener("click", () => call("restart"));
$("quit").addEventListener("click", () => call("quit"));

$("scroller").addEventListener("scroll", (e) => {
  $("head").classList.toggle("scrolled", e.target.scrollTop > 0);
}, { passive: true });

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") call("hide_panel");
});

document.addEventListener("contextmenu", (e) => e.preventDefault());

document.addEventListener("visibilitychange", () => {
  if (!document.hidden) refresh();
});
window.addEventListener("focus", refresh);

/* Start */

load().catch(() => {});
setInterval(poll, STATUS_POLL_MS);
setInterval(refresh, DEVICE_REFRESH_MS);
requestAnimationFrame(animateMeters);

/* Demo data, only outside Tauri (browser preview). */

function demoInvoke(cmd, args) {
  const demo = (window.__demo ||= {
    config: {
      source: "desktop",
      onboarded: true,
      outputs: [
        { id: "wasapi:headset", name: "Headphones (USB Audio)", enabled: true, fader: 0.82, muted: false },
        { id: "wasapi:cable", name: "CABLE Input (VB-Audio Virtual Cable)", enabled: true, fader: 1, muted: false },
        { id: "wasapi:hdmi", name: "LG ULTRAGEAR (NVIDIA High Definition Audio)", enabled: true, fader: 0.6, muted: true },
        { id: "wasapi:old", name: "Bluetooth Speaker", enabled: true, fader: 1, muted: false },
      ],
    },
  });
  const t = performance.now() / 1000;
  const wave = (k) => 0.35 + 0.3 * Math.abs(Math.sin(t * 3.1 + k)) * Math.abs(Math.sin(t * 0.7 + k));
  const enabled = demo.config.outputs.filter((o) => o.enabled && o.id !== "wasapi:old");
  switch (cmd) {
    case "snapshot":
      return Promise.resolve({
        version: "0.1.0",
        autostart: true,
        update_ready: new URLSearchParams(location.search).get("update"),
        config: structuredClone(demo.config),
        devices: {
          desktop_output: "wasapi:speakers",
          sources: [
            { id: "desktop", name: "Desktop audio", kind: "desktop", is_default: false },
            { id: "loopback:wasapi:speakers", name: "Speakers (Realtek Audio)", kind: "loopback", is_default: true },
            { id: "loopback:wasapi:headset", name: "Headphones (USB Audio)", kind: "loopback", is_default: false },
            { id: "wasapi:mic", name: "Microphone (Shure MV7)", kind: "capture", is_default: true },
          ],
          outputs: [
            { id: "wasapi:speakers", name: "Speakers (Realtek Audio)", is_default: true },
            { id: "wasapi:headset", name: "Headphones (USB Audio)", is_default: false },
            { id: "wasapi:cable", name: "CABLE Input (VB-Audio Virtual Cable)", is_default: false },
            { id: "wasapi:hdmi", name: "LG ULTRAGEAR (NVIDIA High Definition Audio)", is_default: false },
          ],
        },
      });
    case "status":
      return Promise.resolve({
        running: enabled.length > 0,
        source: { state: "playing", message: null, format: "48 kHz, stereo, f32", peak: wave(0) },
        outputs: enabled.map((o) =>
          o.id === "wasapi:hdmi"
            ? { id: o.id, state: "error", message: "Device disconnected", format: null, peak: 0 }
            : { id: o.id, state: "playing", message: null, format: "48 kHz, stereo, f32", peak: wave(0) * o.fader },
        ),
      });
    case "set_output_enabled": {
      const o = demo.config.outputs.find((x) => x.id === args.id);
      if (o) o.enabled = args.enabled;
      else demo.config.outputs.push({ id: args.id, name: args.name, enabled: args.enabled, fader: 1, muted: false });
      return Promise.resolve();
    }
    case "set_autostart":
      return Promise.resolve(args.enabled);
    default:
      return Promise.resolve();
  }
}
