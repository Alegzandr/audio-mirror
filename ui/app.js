"use strict";

const tauri = window.__TAURI__;
/** @type {Invoke} */
const invoke = tauri ? tauri.core.invoke : demoInvoke;
/** @type {Listen} */
const listen = tauri ? tauri.event.listen : () => Promise.resolve();

/** @param {string} id */
const $ = (id) => /** @type {HTMLElement} */ (document.getElementById(id));

/**
 * @param {ParentNode} root
 * @param {string} selector
 */
const find = (root, selector) => /** @type {HTMLElement} */ (root.querySelector(selector));

const { t } = window.I18n;
const {
  faderToDb,
  formatDb,
  meterTarget,
  meterFall,
  meterDb,
  sourceName,
  outputRows,
  runState,
  outputState,
  retrying,
} = window.View;

/**
 * A level meter: `el` is the bar scaled to `level`, which falls toward `target`.
 * @typedef {{ el: HTMLElement, meter: HTMLElement, label?: HTMLElement, level: number, target?: number }} Meter
 */

/** Safety net only: the engine says when the devices changed. */
const DEVICE_REFRESH_MS = 30000;
const STATUS_POLL_MS = 100;

/** @type {Snapshot} Set by `load` before anything reads it. */
let snapshot;
/** @type {Status | null} */
let status = null;
/** Last device change the engine reported, so the list is re-read once. */
let devicesRevision = -1;
/** @type {OutputInfo[]} The devices the list shows, from `outputRows`. */
let shown = [];
/** @type {Map<string, HTMLElement>} */
const rows = new Map();
/** @type {Map<string, Meter>} */
const meters = new Map();

/**
 * @template {Command} K
 * @param {K} cmd
 * @param {CommandArgs<K>} args
 * @returns {Promise<CommandResult<K>>}
 */
async function call(cmd, ...args) {
  try {
    return await invoke(cmd, ...args);
  } catch (err) {
    setRunState(String(err), "error");
    throw err;
  }
}

/**
 * @param {string} text
 * @param {string} tone
 */
function setRunState(text, tone) {
  const el = $("run-state");
  if (el.textContent !== text) el.textContent = text;
  el.dataset.tone = tone;
}

/* Loading */

async function load() {
  snapshot = await call("snapshot");
  $("version").textContent = `v${snapshot.version}`;
  /** @type {HTMLInputElement} */ ($("autostart")).checked = snapshot.autostart;
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
  const select = /** @type {HTMLSelectElement} */ ($("source"));
  const { sources } = snapshot.devices;
  /** @type {[SourceInfo["kind"], string | null][]} */
  const groups = [
    ["desktop", null],
    ["loopback", t("source.group.output")],
    ["capture", t("source.group.input")],
  ];
  select.replaceChildren();
  for (const [kind, label] of groups) {
    const items = sources.filter((s) => s.kind === kind);
    if (!items.length) continue;
    const group = label ? document.createElement("optgroup") : null;
    if (group && label) group.label = label;
    const parent = group || select;
    for (const s of items) {
      const opt = document.createElement("option");
      opt.value = s.id;
      opt.textContent = sourceName(s);
      parent.append(opt);
    }
    if (group) select.append(group);
  }
  const current = snapshot.config.source;
  if (!sources.some((s) => s.id === current)) {
    const opt = document.createElement("option");
    opt.value = current;
    opt.textContent = t("source.disconnected");
    select.prepend(opt);
  }
  select.value = current;
}

/** @param {string} id */
function outputConfig(id) {
  return snapshot.config.outputs.find((o) => o.id === id);
}

function enabledCount() {
  return shown.filter((d) => outputConfig(d.id)?.enabled).length;
}

function renderOutputs() {
  const list = $("outputs");
  shown = outputRows(snapshot.devices, snapshot.config);

  list.replaceChildren();
  rows.clear();
  meters.clear();

  if (!shown.length) {
    const p = document.createElement("p");
    p.className = "row empty";
    p.textContent = t("outputs.none");
    list.append(p);
    return;
  }

  if (!enabledCount()) {
    const p = document.createElement("p");
    p.className = "note";
    p.textContent = t("outputs.hint");
    list.append(p);
  }

  const tpl = /** @type {HTMLTemplateElement} */ ($("output-row"));
  shown.forEach((dev, i) => {
    const node = /** @type {HTMLElement} */ (tpl.content.firstElementChild?.cloneNode(true));
    const cfg = outputConfig(dev.id) || { enabled: false, fader: 1, muted: false };
    const sw = /** @type {HTMLInputElement} */ (find(node, ".output-enabled"));
    const name = /** @type {HTMLLabelElement} */ (find(node, ".output-name"));
    const fader = /** @type {HTMLInputElement} */ (find(node, ".fader"));
    const mute = find(node, ".output-mute");
    const meter = find(node, ".output-meter");

    node.dataset.id = dev.id;
    node.dataset.name = dev.name;
    node.dataset.enabled = String(cfg.enabled);
    sw.id = `output-${i}`;
    sw.checked = cfg.enabled;
    name.htmlFor = sw.id;
    name.textContent = dev.is_default ? t("device.default", { name: dev.name }) : dev.name;
    name.title = dev.name;
    fader.value = String(Math.round(cfg.fader * 1000));
    fader.setAttribute("aria-label", t("output.volume", { name: dev.name }));
    meter.setAttribute("aria-label", t("output.level", { name: dev.name }));
    setMuted(node, cfg.muted);
    updateFaderView(node, cfg.fader);

    list.append(node);
    rows.set(dev.id, node);
    meters.set(dev.id, { el: /** @type {HTMLElement} */ (meter.firstElementChild), meter, level: 0 });
    mute.setAttribute("aria-label", t("output.muteLabel", { name: dev.name }));
  });
  applyStatus();
}

/**
 * @param {HTMLElement} node
 * @param {number} def
 */
function updateFaderView(node, def) {
  const fader = find(node, ".fader");
  const text = formatDb(faderToDb(def));
  fader.style.setProperty("--fill", `${def * 100}%`);
  fader.setAttribute("aria-valuetext", text);
  find(node, ".output-db").textContent = text;
}

/**
 * @param {HTMLElement} node
 * @param {boolean} muted
 */
function setMuted(node, muted) {
  const btn = find(node, ".output-mute");
  btn.setAttribute("aria-pressed", String(muted));
  btn.textContent = muted ? t("output.muted") : t("output.mute");
  node.dataset.muted = String(muted);
}

/**
 * @param {HTMLElement} node
 * @param {string} text
 * @param {string} tone
 */
function setState(node, text, tone) {
  const el = find(node, ".output-state");
  if (el.textContent !== text) el.textContent = text;
  el.dataset.tone = tone;
}

/**
 * @param {HTMLElement} node
 * @param {string} text
 */
function setDetail(node, text) {
  const el = find(node, ".output-detail");
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
  // The backends already know when a device appears or goes away, so the
  // list is re-read on their word rather than on a timer.
  if (status.devices_revision !== devicesRevision) {
    const first = devicesRevision === -1;
    devicesRevision = status.devices_revision;
    if (!first) refresh();
  }
}

function applyStatus() {
  const live = status && status.running ? status : null;
  const src = live ? live.source : null;

  const run = runState(live, shown);
  setRunState(run.text, run.tone);

  const err = $("source-error");
  const msg = src && src.state === "error" ? retrying(src.message) : "";
  err.hidden = !msg;
  if (err.textContent !== msg) err.textContent = msg;
  pushLevel("__source", src ? src.peak : 0);

  const byId = new Map((live ? live.outputs : []).map((o) => [o.id, o]));
  for (const [id, node] of rows) {
    const enabled = node.dataset.enabled === "true";
    const st = byId.get(id);
    const { label, tone, detail } = outputState(st, {
      enabled,
      muted: node.dataset.muted === "true",
    });
    setState(node, label, tone);
    setDetail(node, detail);
    pushLevel(id, enabled && st ? st.peak : 0);
  }
}

/* Meters: instant peak, smooth fall */

/** @type {Meter} */
const sourceMeter = { el: /** @type {HTMLElement} */ ($("source-meter").firstElementChild), meter: $("source-meter"), label: $("source-db"), level: 0 };

/**
 * @param {string} id
 * @param {number} peak
 */
function pushLevel(id, peak) {
  const m = id === "__source" ? sourceMeter : meters.get(id);
  if (!m) return;
  m.target = meterTarget(peak);
}

function animateMeters() {
  for (const m of [sourceMeter, ...meters.values()]) {
    m.level = meterFall(m.level, m.target || 0);
    m.el.style.transform = `scaleX(${m.level.toFixed(3)})`;
    const db = meterDb(m.level);
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

/** @param {string | null} version */
function showUpdate(version) {
  const restart = $("restart");
  restart.hidden = !version;
  $("version").hidden = Boolean(version);
  if (version) restart.title = t("update.installed", { version });
}

listen("update-ready", ({ payload }) => showUpdate(payload));

/* Actions */

$("source").addEventListener("change", async (e) => {
  const { value } = /** @type {HTMLSelectElement} */ (e.target);
  await call("set_source", { id: value });
  snapshot.config.source = value;
  renderOutputs();
});

/** @param {HTMLElement} node */
function localOutput(node) {
  const id = node.dataset.id || "";
  let o = outputConfig(id);
  if (!o) {
    o = { id, name: node.dataset.name || "", enabled: false, fader: 1, muted: false };
    snapshot.config.outputs.push(o);
  }
  return o;
}

/**
 * The output row an event happened in, with its device id and name.
 * @param {Element} target
 */
function outputOf(target) {
  const node = /** @type {HTMLElement} */ (target.closest(".output"));
  return { node, id: node.dataset.id || "", name: node.dataset.name || "" };
}

$("outputs").addEventListener("change", async (e) => {
  const target = /** @type {HTMLInputElement} */ (e.target);
  if (target.classList.contains("fader")) {
    // Released: save the final position.
    const { id, name } = outputOf(target);
    const fader = Number(target.value) / 1000;
    call("set_output_volume", { id, name, fader, persist: true });
    return;
  }
  if (!target.classList.contains("output-enabled")) return;
  const { node, id, name } = outputOf(target);
  const enabled = target.checked;
  try {
    await call("set_output_enabled", { id, name, enabled });
  } catch {
    // The engine kept the old value: put the switch back where it was.
    target.checked = !enabled;
    return;
  }
  localOutput(node).enabled = enabled;
  const hadNote = Boolean($("outputs").querySelector(".note"));
  if (hadNote !== !enabledCount()) {
    renderOutputs();
    /** @type {HTMLElement | null} */ ($("outputs").querySelector(`[data-id="${CSS.escape(id)}"] .output-enabled`))?.focus();
  } else {
    node.dataset.enabled = String(enabled);
    applyStatus();
  }
});

/** @type {Map<string, number>} */
const pendingVolume = new Map();
$("outputs").addEventListener("input", (e) => {
  const target = /** @type {HTMLInputElement} */ (e.target);
  if (!target.classList.contains("fader")) return;
  const { node, id, name } = outputOf(target);
  const def = Number(target.value) / 1000;
  updateFaderView(node, def);
  localOutput(node).fader = def;
  // At most one call per frame while dragging.
  const queued = pendingVolume.has(id);
  pendingVolume.set(id, def);
  if (queued) return;
  requestAnimationFrame(() => {
    const fader = pendingVolume.get(id) ?? def;
    pendingVolume.delete(id);
    call("set_output_volume", { id, name, fader, persist: false });
  });
});

$("outputs").addEventListener("click", async (e) => {
  const btn = /** @type {Element} */ (e.target).closest("button");
  if (!btn) return;
  const { node, id, name } = outputOf(btn);
  if (btn.classList.contains("output-mute")) {
    const muted = node.dataset.muted !== "true";
    await call("set_output_muted", { id, name, muted });
    localOutput(node).muted = muted;
    setMuted(node, muted);
    applyStatus();
  }
});

$("autostart").addEventListener("change", async (e) => {
  const box = /** @type {HTMLInputElement} */ (e.target);
  try {
    box.checked = await call("set_autostart", { enabled: box.checked });
  } catch {
    box.checked = !box.checked;
  }
});

$("restart").addEventListener("click", () => call("restart"));
$("quit").addEventListener("click", () => call("quit"));

$("scroller").addEventListener("scroll", (e) => {
  $("head").classList.toggle("scrolled", /** @type {Element} */ (e.target).scrollTop > 0);
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

/**
 * @template {Command} K
 * @param {K} cmd
 * @param {CommandArgs<K>} args
 * @returns {Promise<CommandResult<K>>}
 */
function demoInvoke(cmd, ...args) {
  const demo = (window.__demo ||= {
    config: {
      source: "desktop",
      outputs: [
        { id: "wasapi:headset", name: "Headphones (USB Audio)", enabled: true, fader: 0.82, muted: false },
        { id: "wasapi:cable", name: "CABLE Input (VB-Audio Virtual Cable)", enabled: true, fader: 1, muted: false },
        { id: "wasapi:hdmi", name: "LG ULTRAGEAR (NVIDIA High Definition Audio)", enabled: true, fader: 0.6, muted: true },
        { id: "wasapi:old", name: "Bluetooth Speaker", enabled: true, fader: 1, muted: false },
      ],
    },
  });
  const t = performance.now() / 1000;
  /** @param {number} k */
  const wave = (k) => 0.35 + 0.3 * Math.abs(Math.sin(t * 3.1 + k)) * Math.abs(Math.sin(t * 0.7 + k));
  const enabled = demo.config.outputs.filter((o) => o.enabled && o.id !== "wasapi:old");
  /** @type {{ [C in Command]?: (args: Commands[C]["args"]) => CommandResult<C> }} */
  const handlers = {
    snapshot: () => ({
      version: "0.1.0",
      autostart: true,
      update_ready: new URLSearchParams(location.search).get("update"),
      config: structuredClone(demo.config),
      devices: {
        sources: [
          { id: "desktop", name: "Default output (Speakers (Realtek Audio))", kind: "desktop", is_default: false, captures_output: "wasapi:speakers" },
          { id: "output:wasapi:speakers", name: "Speakers (Realtek Audio)", kind: "loopback", is_default: true, captures_output: "wasapi:speakers" },
          { id: "output:wasapi:headset", name: "Headphones (USB Audio)", kind: "loopback", is_default: false, captures_output: "wasapi:headset" },
          { id: "input:wasapi:mic", name: "Microphone (Shure MV7)", kind: "capture", is_default: true, captures_output: null },
        ],
        outputs: [
          { id: "wasapi:speakers", name: "Speakers (Realtek Audio)", is_default: true },
          { id: "wasapi:headset", name: "Headphones (USB Audio)", is_default: false },
          { id: "wasapi:cable", name: "CABLE Input (VB-Audio Virtual Cable)", is_default: false },
          { id: "wasapi:hdmi", name: "LG ULTRAGEAR (NVIDIA High Definition Audio)", is_default: false },
        ],
      },
    }),
    status: () => ({
      running: enabled.length > 0,
      devices_revision: 0,
      source: { state: "playing", message: null, format: "48 kHz, stereo", peak: wave(0) },
      outputs: enabled.map((o) =>
        o.id === "wasapi:hdmi"
          ? {
              id: o.id,
              state: "error",
              message: { code: "deviceDisconnected", text: "Device disconnected", detail: null },
              format: null,
              peak: 0,
            }
          : { id: o.id, state: "playing", message: null, format: "48 kHz, stereo", peak: wave(0) * o.fader },
      ),
    }),
    set_output_enabled: ({ id, name, enabled }) => {
      const o = demo.config.outputs.find((x) => x.id === id);
      if (o) o.enabled = enabled;
      else demo.config.outputs.push({ id, name, enabled, fader: 1, muted: false });
    },
    set_autostart: ({ enabled }) => enabled,
  };
  // The handler matches `cmd`, which TypeScript cannot follow through the lookup.
  const handler = /** @type {((args: unknown) => any) | undefined} */ (handlers[cmd]);
  return Promise.resolve(handler?.(args[0]));
}
