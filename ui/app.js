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

// The material under the page (`glass.rs`): Liquid Glass, vibrancy or
// Acrylic, or nothing on Linux and in the browser preview, where the page
// paints its own ground. The OS sets the window's corner radius and the
// font: SF Pro on macOS, the bundled Inter elsewhere.
const os = /Windows/.test(navigator.userAgent) ? "windows" : /Macintosh/.test(navigator.userAgent) ? "macos" : "other";
document.documentElement.dataset.os = os;
document.documentElement.dataset.glass = window.__AUDIO_MIRROR_GLASS__ || "none";
const {
  faderToDb,
  formatDb,
  meterTarget,
  meterFall,
  meterDb,
  sourceName,
  outputRows,
  splitOutputs,
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
/** @type {OutputInfo[]} The devices the panel lists, from `outputRows`. */
let shown = [];
/** Edit mode: the output rows offer Rename and Remove instead of their controls. */
let editing = false;
/** @type {boolean | null} Whether the other devices are unfolded; null until the user decides. */
let othersOpen = null;
/** @type {Map<string, string>} A rename the system refused, shown under its row until the next one. */
const renameErrors = new Map();
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

/** Newest `snapshot` request: an older answer that arrives late is dropped. */
let snapshotRequest = 0;

async function load() {
  const request = ++snapshotRequest;
  const next = await call("snapshot");
  if (request !== snapshotRequest) return;
  snapshot = next;
  $("version").textContent = `v${snapshot.version}`;
  /** @type {HTMLInputElement} */ ($("autostart")).checked = snapshot.autostart;
  showUpdate(snapshot.update_ready);
  renderSource();
  renderOutputs();
}

/** Until the first snapshot arrives, asks again every second. */
function start() {
  load().catch(() => setTimeout(start, 1000));
}

/** A fader held under the pointer, or the source menu open. */
let dragging = false;
let menuOpen = false;
/** A refresh the user's hold put off, run once they let go. */
let refreshPending = false;

/** Rebuilding the list under an open menu, a dragged slider or a name being typed would drop it. */
function holding() {
  return dragging || menuOpen || Boolean($("outputs").querySelector(".output-rename:not([hidden])"));
}

function release() {
  if (refreshPending && !holding()) refresh();
}

/** Re-reads devices, but never while the user is holding a control. */
async function refresh() {
  if (document.hidden) return;
  if (!snapshot) {
    start();
    return;
  }
  if (holding()) {
    refreshPending = true;
    return;
  }
  refreshPending = false;
  const request = ++snapshotRequest;
  try {
    const next = await invoke("snapshot");
    if (request !== snapshotRequest) return;
    const changed = JSON.stringify(next.devices) !== JSON.stringify(snapshot.devices);
    snapshot = { ...next, config: snapshot.config };
    showUpdate(next.update_ready);
    if (changed) {
      if (holding()) {
        // The user took hold of a control while the list was being read.
        refreshPending = true;
        return;
      }
      renderSource();
      renderOutputs();
    }
  } catch {
    // The next tick tries again.
  }
}

document.addEventListener("pointerdown", (e) => {
  const target = /** @type {Element} */ (e.target);
  if (target.matches(".fader")) dragging = true;
  if (target.matches("#source")) menuOpen = true;
});
for (const type of ["pointerup", "pointercancel"]) {
  window.addEventListener(type, () => {
    if (!dragging) return;
    dragging = false;
    release();
  });
}
for (const type of ["change", "blur"]) {
  $("source").addEventListener(type, () => {
    menuOpen = false;
    release();
  });
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
  renderPicker();
  // Width and items are known now; the map waits for an idle moment.
  const idle = window.requestIdleCallback || ((/** @type {() => void} */ f) => setTimeout(f, 200));
  idle(prewarmMenu);
}

/* Source picker: a macOS pop-up menu drawn over the select, which stays
   the model the rest of the panel reads and listens to. */

const picker = $("source-button");
const menu = $("source-menu");
const CHECK = '<svg width="11" height="11" viewBox="0 0 11 11" aria-hidden="true"><path d="M1.5 5.75l2.75 2.75L9.5 2.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>';

function renderPicker() {
  const select = /** @type {HTMLSelectElement} */ ($("source"));
  find(picker, ".picker-label").textContent = select.selectedOptions[0]?.textContent || "";
}

/** @returns {HTMLElement[]} */
function menuItems() {
  return [...menu.querySelectorAll("[role=option]")].map((el) => /** @type {HTMLElement} */ (el));
}

/** @param {HTMLOptionElement} opt */
function menuItem(opt) {
  const item = document.createElement("div");
  item.className = "menu-item";
  item.setAttribute("role", "option");
  item.setAttribute("aria-selected", String(opt.selected));
  item.tabIndex = -1;
  item.dataset.value = opt.value;
  const check = document.createElement("span");
  check.className = "menu-check";
  if (opt.selected) check.innerHTML = CHECK;
  const label = document.createElement("span");
  label.className = "menu-label";
  label.textContent = opt.textContent;
  item.append(check, label);
  return item;
}

/** Fills the page menu from the select. */
function buildMenu() {
  const select = /** @type {HTMLSelectElement} */ ($("source"));
  menu.replaceChildren();
  for (const child of select.children) {
    if (child instanceof HTMLOptGroupElement) {
      const head = document.createElement("div");
      head.className = "menu-header";
      head.setAttribute("role", "presentation");
      head.textContent = child.label;
      menu.append(head);
      for (const opt of child.querySelectorAll("option")) menu.append(menuItem(opt));
    } else if (child instanceof HTMLOptionElement) {
      menu.append(menuItem(child));
    }
  }
}

/**
 * Computes the menu's refraction map while nothing moves, so the first
 * opening does not stall on it (some 25 ms, four times that on a slow CPU).
 */
function prewarmMenu() {
  if (!window.Refraction.supported || !menu.hidden || (tauri && os === "macos")) return;
  buildMenu();
  menu.style.visibility = "hidden";
  menu.hidden = false;
  menu.style.maxHeight = `${window.innerHeight - 12}px`;
  menu.style.width = `${Math.min(picker.getBoundingClientRect().width + 21, window.innerWidth - 12)}px`;
  window.Refraction.apply(menu);
  menu.style.removeProperty("backdrop-filter");
  menu.hidden = true;
  menu.style.visibility = "";
}

function openMenu() {
  buildMenu();
  // As macOS pops a menu up: over the button, the current item laid on
  // it with its label where the button's is, kept inside the window.
  const r = picker.getBoundingClientRect();
  const margin = 6;
  menu.style.visibility = "hidden";
  menu.hidden = false;
  menu.style.maxHeight = `${window.innerHeight - 2 * margin}px`;
  const labelShift = 21;
  const width = Math.min(r.width + labelShift, window.innerWidth - 2 * margin);
  menu.style.width = `${width}px`;
  menu.style.left = `${Math.max(margin, Math.min(r.left - labelShift, window.innerWidth - width - margin))}px`;
  const current = menuItems().find((i) => i.getAttribute("aria-selected") === "true") || menuItems()[0];
  const itemTop = current ? current.offsetTop : 0;
  const itemHeight = current ? current.offsetHeight : 0;
  const wanted = r.top + (r.height - itemHeight) / 2 - itemTop;
  const top = Math.max(margin, Math.min(wanted, window.innerHeight - menu.offsetHeight - margin));
  menu.style.top = `${top}px`;
  menu.style.transformOrigin = `center ${r.top + r.height / 2 - top}px`;
  menu.style.visibility = "";
  // The refracting filter is computed over the backdrop on every frame the
  // menu moves: it goes on once the pop-up has settled.
  menu.style.removeProperty("backdrop-filter");
  menu.addEventListener("animationend", () => {
    if (!menu.hidden) window.Refraction.apply(menu);
  }, { once: true });
  picker.setAttribute("aria-expanded", "true");
  menuOpen = true;
  (menuItems().find((i) => i.getAttribute("aria-selected") === "true") || menuItems()[0])?.focus();
}

/** @param {boolean} [refocus] */
function closeMenu(refocus = true) {
  if (menu.hidden) return;
  menu.hidden = true;
  picker.setAttribute("aria-expanded", "false");
  menuOpen = false;
  if (refocus) picker.focus();
  release();
}

/** @param {string} value */
function choose(value) {
  const select = /** @type {HTMLSelectElement} */ ($("source"));
  closeMenu();
  if (value === select.value) return;
  select.value = value;
  renderPicker();
  select.dispatchEvent(new Event("change"));
}

/**
 * On macOS the menu is the system's own (`source_menu`): it can leave the
 * window and is the real thing. Elsewhere a native menu would look like the
 * host system, so the page draws macOS's.
 */
async function openNativeMenu() {
  const select = /** @type {HTMLSelectElement} */ ($("source"));
  /** @type {{ id: string | null, label: string, checked: boolean }[]} */
  const entries = [];
  for (const child of select.children) {
    if (child instanceof HTMLOptGroupElement) {
      entries.push({ id: null, label: child.label, checked: false });
      for (const opt of child.querySelectorAll("option")) entries.push({ id: opt.value, label: opt.textContent || "", checked: opt.selected });
    } else if (child instanceof HTMLOptionElement) {
      entries.push({ id: child.value, label: child.textContent || "", checked: child.selected });
    }
  }
  // macOS lays the current item over the button: rows of 22 px under a 5 px inset.
  const at = Math.max(0, entries.findIndex((e) => e.checked));
  const r = picker.getBoundingClientRect();
  await call("source_menu", { entries, x: r.left - 9, y: r.top + (r.height - 22) / 2 - 5 - at * 22 });
}

picker.addEventListener("click", () => {
  if (tauri && os === "macos") openNativeMenu().catch(() => {});
  else if (menu.hidden) openMenu();
  else closeMenu();
});
listen("source-picked", ({ payload }) => choose(payload));
picker.addEventListener("keydown", (e) => {
  if (["ArrowDown", "ArrowUp", "Enter", " "].includes(e.key) && menu.hidden) {
    e.preventDefault();
    picker.click();
  }
});
menu.addEventListener("click", (e) => {
  const item = /** @type {HTMLElement | null} */ (/** @type {Element} */ (e.target).closest("[role=option]"));
  if (item) choose(item.dataset.value || "");
});
menu.addEventListener("pointermove", (e) => {
  const item = /** @type {HTMLElement | null} */ (/** @type {Element} */ (e.target).closest("[role=option]"));
  if (item && document.activeElement !== item) item.focus();
});
menu.addEventListener("keydown", (e) => {
  const items = menuItems();
  const at = items.indexOf(/** @type {HTMLElement} */ (document.activeElement));
  /** @type {HTMLElement | undefined} */
  let next;
  if (e.key === "ArrowDown") next = items[Math.min(items.length - 1, at + 1)];
  else if (e.key === "ArrowUp") next = items[Math.max(0, at - 1)];
  else if (e.key === "Home") next = items[0];
  else if (e.key === "End") next = items[items.length - 1];
  else if (e.key === "Enter" || e.key === " ") {
    e.preventDefault();
    if (at >= 0) choose(items[at].dataset.value || "");
    return;
  } else if (e.key === "Escape") {
    // Closes the menu instead of hiding the panel.
    e.stopPropagation();
    closeMenu();
    return;
  } else if (e.key === "Tab") {
    closeMenu(false);
    return;
  } else return;
  e.preventDefault();
  next?.focus();
});
document.addEventListener("pointerdown", (e) => {
  const target = /** @type {Element} */ (e.target);
  if (!menu.hidden && !menu.contains(target) && !picker.contains(target)) closeMenu(false);
});
window.addEventListener("blur", () => closeMenu(false));

/** @param {string} id */
function outputConfig(id) {
  return snapshot.config.outputs.find((o) => o.id === id);
}

/** @param {OutputInfo} dev */
function deviceName(dev) {
  return dev.is_default ? t("device.default", { name: dev.name }) : dev.name;
}

/** The row controls focus can be put back on after a rebuild. */
const ROW_CONTROLS = [".fader", ".output-mute", ".output-rename-btn", ".output-remove", ".other-add"];

/** @param {Element | null | undefined} el */
const visible = (el) => el instanceof HTMLElement && el.offsetParent !== null;

/** The focused row control, as a device id and selector that survive a rebuild. */
function focusedControl() {
  const el = document.activeElement;
  const row = /** @type {HTMLElement | null | undefined} */ (el?.closest("#outputs [data-id], #others [data-id]"));
  const control = el && ROW_CONTROLS.find((s) => el.matches(s));
  return row && control ? { id: row.dataset.id || "", control } : null;
}

/**
 * Puts focus back where it was before the lists were rebuilt: on the same
 * control, else on another one of the same row, else on the list's header.
 * @param {{ id: string, control: string } | null} was
 */
function restoreFocus(was) {
  if (!was || (document.activeElement && document.activeElement !== document.body)) return;
  const row = document.querySelector(`#outputs [data-id="${CSS.escape(was.id)}"], #others [data-id="${CSS.escape(was.id)}"]`);
  const candidates = [
    row?.querySelector(was.control),
    ...(row ? ROW_CONTROLS.map((s) => row.querySelector(s)) : []),
    $("edit"),
    $("others-toggle"),
  ];
  /** @type {HTMLElement | undefined} */ (candidates.find(visible))?.focus();
}

function renderOutputs() {
  const list = $("outputs");
  const focus = focusedControl();
  shown = outputRows(snapshot.devices, snapshot.config);
  const { active, others } = splitOutputs(shown, snapshot.config);

  list.replaceChildren();
  rows.clear();
  meters.clear();

  if (!active.length) {
    const p = document.createElement("p");
    p.className = "row empty";
    p.textContent = shown.length ? t("outputs.hint") : t("outputs.none");
    list.append(p);
  }

  const tpl = /** @type {HTMLTemplateElement} */ ($("output-row"));
  for (const dev of active) {
    const node = /** @type {HTMLElement} */ (tpl.content.firstElementChild?.cloneNode(true));
    const cfg = outputConfig(dev.id) || { enabled: true, fader: 1, muted: false };
    const fader = /** @type {HTMLInputElement} */ (find(node, ".fader"));
    const meter = find(node, ".output-meter");
    const name = find(node, ".output-name");

    node.dataset.id = dev.id;
    node.dataset.name = dev.name;
    name.textContent = deviceName(dev);
    name.title = dev.name;
    fader.value = String(Math.round(cfg.fader * 1000));
    fader.setAttribute("aria-label", t("output.volume", { name: dev.name }));
    meter.setAttribute("aria-label", t("output.level", { name: dev.name }));
    find(node, ".output-mute").setAttribute("aria-label", t("output.muteLabel", { name: dev.name }));
    find(node, ".output-remove").setAttribute("aria-label", t("output.removeLabel", { name: dev.name }));
    find(node, ".output-rename").setAttribute("aria-label", t("output.renameLabel", { name: dev.name }));
    find(node, ".output-rename-btn").setAttribute("aria-label", t("output.renameButtonLabel", { name: dev.name }));
    // Only where the system lets a program rename its devices.
    find(node, ".output-rename-btn").hidden = dev.description === null;
    setMuted(node, cfg.muted);
    updateFaderView(node, cfg.fader);

    list.append(node);
    rows.set(dev.id, node);
    meters.set(dev.id, { el: /** @type {HTMLElement} */ (meter.firstElementChild), meter, level: 0 });
  }

  if (!active.length) editing = false;
  const edit = $("edit");
  edit.hidden = !active.length;
  edit.textContent = editing ? t("outputs.done") : t("outputs.edit");
  list.dataset.editing = String(editing);

  renderOthers(others, !active.length);
  applyStatus();
  restoreFocus(focus);
}

/**
 * The devices that are not playing, folded under one row.
 * @param {OutputInfo[]} others
 * @param {boolean} nothingPlays Unfolded by default when no output is on.
 */
function renderOthers(others, nothingPlays) {
  const group = $("others-group");
  const box = $("others");
  const toggle = $("others-toggle");
  group.hidden = !others.length;
  box.replaceChildren();
  if (!others.length) return;

  const open = othersOpen ?? nothingPlays;
  find(toggle, ".disclosure-label").textContent = t("outputs.others", { count: others.length });
  find(toggle, ".disclosure-action").textContent = open ? t("outputs.hide") : t("outputs.show");
  toggle.setAttribute("aria-expanded", String(open));
  box.hidden = !open;

  const tpl = /** @type {HTMLTemplateElement} */ ($("other-row"));
  for (const dev of others) {
    const node = /** @type {HTMLElement} */ (tpl.content.firstElementChild?.cloneNode(true));
    const name = find(node, ".other-name");
    node.dataset.id = dev.id;
    node.dataset.name = dev.name;
    name.textContent = deviceName(dev);
    name.title = dev.name;
    find(node, ".other-add").setAttribute("aria-label", t("output.addLabel", { name: dev.name }));
    box.append(node);
  }
}

/**
 * @param {HTMLElement} node
 * @param {number} def
 */
function updateFaderView(node, def) {
  const fader = find(node, ".fader");
  const text = formatDb(faderToDb(def));
  find(node, ".capsule").style.setProperty("--fill", `${def * 100}%`);
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
  btn.title = muted ? t("output.muted") : t("output.mute");
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
    const st = byId.get(id);
    const { label, tone, detail } = outputState(st, {
      enabled: true,
      muted: node.dataset.muted === "true",
    });
    setState(node, label, tone);
    // A refused rename sits next to the engine's word, never in its place.
    setDetail(node, [renameErrors.get(id), detail].filter(Boolean).join(" "));
    pushLevel(id, st ? st.peak : 0);
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
  const select = /** @type {HTMLSelectElement} */ (e.target);
  const { value } = select;
  try {
    await call("set_source", { id: value });
  } catch {
    // Back to the source that is still in use.
    select.value = snapshot.config.source;
    renderPicker();
    return;
  }
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

$("outputs").addEventListener("change", (e) => {
  const target = /** @type {HTMLInputElement} */ (e.target);
  if (!target.classList.contains("fader")) return;
  // Released: save the final position.
  const { id, name } = outputOf(target);
  const fader = Number(target.value) / 1000;
  call("set_output_volume", { id, name, fader, persist: true });
});

/**
 * Turns an output on or off, then redraws the two lists it moves between.
 * @param {string} id
 * @param {string} name
 * @param {boolean} enabled
 */
async function setEnabled(id, name, enabled) {
  await call("set_output_enabled", { id, name, enabled });
  let o = outputConfig(id);
  if (!o) {
    o = { id, name, enabled, fader: 1, muted: false };
    snapshot.config.outputs.push(o);
  }
  o.enabled = enabled;
  renderOutputs();
}

/**
 * Shown at once, so a second press before the first answer toggles back
 * instead of sending the same state twice.
 * @param {HTMLElement} node
 */
async function toggleMute(node) {
  const { id, name } = outputOf(node);
  const muted = node.dataset.muted !== "true";
  localOutput(node).muted = muted;
  setMuted(node, muted);
  applyStatus();
  try {
    await call("set_output_muted", { id, name, muted });
  } catch (err) {
    localOutput(node).muted = !muted;
    // The row may have been rebuilt meanwhile.
    const row = rows.get(id);
    if (row) setMuted(row, !muted);
    applyStatus();
    throw err;
  }
}

/* Rename: the name changes in the system, so every application shows it. */

/** @param {HTMLElement} node */
function startRename(node) {
  const dev = shown.find((d) => d.id === node.dataset.id);
  if (!dev || dev.description === null) return;
  renameErrors.delete(dev.id);
  const input = /** @type {HTMLInputElement} */ (find(node, ".output-rename"));
  input.value = dev.description;
  input.hidden = false;
  find(node, ".output-name").hidden = true;
  // What the system keeps, such as the adapter in "Speakers (Realtek Audio)".
  const suffix = find(node, ".output-suffix");
  suffix.textContent = dev.name.startsWith(dev.description) ? dev.name.slice(dev.description.length).trim() : "";
  suffix.hidden = !suffix.textContent;
  input.focus();
  input.select();
}

/**
 * @param {HTMLInputElement} input
 * @param {boolean} save
 */
async function endRename(input, save) {
  if (input.hidden) return;
  const { node, id } = outputOf(input);
  const dev = shown.find((d) => d.id === id);
  const description = input.value.trim();
  input.hidden = true;
  find(node, ".output-suffix").hidden = true;
  find(node, ".output-name").hidden = false;
  if (!save || !dev || !description || description === dev.description) return;
  try {
    await invoke("rename_output", { id, description });
    renameErrors.delete(id);
  } catch {
    renameErrors.set(id, t("output.renameFailed"));
    applyStatus();
    return;
  }
  // The system names it differently now: read the list again.
  await refresh();
}

$("outputs").addEventListener("keydown", (e) => {
  const target = /** @type {HTMLElement} */ (e.target);
  if (target.classList.contains("output-rename")) {
    const input = /** @type {HTMLInputElement} */ (target);
    const button = find(outputOf(input).node, ".output-rename-btn");
    if (e.key === "Enter") {
      endRename(input, true);
      button.focus();
    }
    if (e.key === "Escape") {
      // Cancels the rename instead of hiding the panel.
      e.stopPropagation();
      endRename(input, false);
      button.focus();
    }
    return;
  }
  if (e.key.toLowerCase() === "m" && !e.ctrlKey && !e.metaKey && !e.altKey && !editing) {
    const node = /** @type {HTMLElement | null} */ (target.closest(".output"));
    if (node) {
      e.preventDefault();
      toggleMute(node);
    }
  }
});

$("outputs").addEventListener("focusout", (e) => {
  const target = /** @type {HTMLElement} */ (e.target);
  if (target.classList.contains("output-rename")) endRename(/** @type {HTMLInputElement} */ (target), true);
});

$("edit").addEventListener("click", () => {
  editing = !editing;
  renderOutputs();
});

$("others-toggle").addEventListener("click", () => {
  othersOpen = $("others-toggle").getAttribute("aria-expanded") !== "true";
  renderOutputs();
  // The rows animate in when the user unfolds them, not on every rebuild.
  const box = $("others");
  if (othersOpen) {
    box.classList.add("unfolding");
    // After the last, delayed row: one spring and its stagger.
    window.setTimeout(() => box.classList.remove("unfolding"), 900);
  }
});

$("others").addEventListener("click", async (e) => {
  const node = /** @type {HTMLElement | null} */ (/** @type {Element} */ (e.target).closest(".other-add")?.closest(".other") ?? null);
  if (!node) return;
  const id = node.dataset.id || "";
  // The list stays unfolded for the next one, even once an output plays.
  othersOpen ??= true;
  try {
    await setEnabled(id, node.dataset.name || "", true);
  } catch {
    return;
  }
  const row = $("outputs").querySelector(`[data-id="${CSS.escape(id)}"]`);
  // Edit mode hides the fader: Remove is the row's control then.
  /** @type {HTMLElement | undefined} */ ([row?.querySelector(".fader"), row?.querySelector(".output-remove")].find(visible))?.focus();
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

$("outputs").addEventListener("click", (e) => {
  const btn = /** @type {Element} */ (e.target).closest("button");
  if (!btn) return;
  const { node, id, name } = outputOf(btn);
  if (btn.classList.contains("output-mute")) toggleMute(node).catch(() => {});
  if (btn.classList.contains("output-rename-btn")) startRename(node);
  if (btn.classList.contains("output-remove")) setEnabled(id, name, false).catch(() => {});
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

/** Overlay scroll bars, as macOS draws them: shown while the list moves. */
let scrollFade = 0;
$("scroller").addEventListener("scroll", () => {
  const el = $("scroller");
  el.classList.add("scrolling");
  clearTimeout(scrollFade);
  scrollFade = window.setTimeout(() => el.classList.remove("scrolling"), 900);
  closeMenu(false);
}, { passive: true });

$("scroller").addEventListener("scroll", (e) => {
  $("head").classList.toggle("scrolled", /** @type {Element} */ (e.target).scrollTop > 0);
}, { passive: true });

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") call("hide_panel");
});

document.addEventListener("contextmenu", (e) => e.preventDefault());

/** The panel settles in each time it opens. */
function enter() {
  const panel = /** @type {HTMLElement} */ (document.querySelector(".panel"));
  panel.classList.remove("enter");
  void panel.offsetWidth;
  panel.classList.add("enter");
}

document.addEventListener("visibilitychange", () => {
  if (!document.hidden) {
    enter();
    refresh();
  }
});
window.addEventListener("focus", refresh);

/* Start */

enter();
start();
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
    // Description and adapter of each output, as Windows composes the name.
    names: {
      "wasapi:speakers": ["Speakers", "Realtek Audio"],
      "wasapi:headset": ["Headphones", "USB Audio"],
      "wasapi:cable": ["CABLE Input", "VB-Audio Virtual Cable"],
      "wasapi:hdmi": ["LG ULTRAGEAR", "NVIDIA High Definition Audio"],
      "wasapi:vm": ["Voicemeeter Input", "VB-Audio Voicemeeter VAIO"],
      "wasapi:dock": ["Speakers", "Dell USB Dock"],
    },
  });
  /** @param {string} id */
  const nameOf = (id) => `${demo.names[id][0]} (${demo.names[id][1]})`;
  /**
   * @param {string} id
   * @param {boolean} [is_default]
   * @returns {OutputInfo}
   */
  const output = (id, is_default = false) => ({ id, name: nameOf(id), is_default, description: demo.names[id][0] });
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
          { id: "desktop", name: `Default output (${nameOf("wasapi:speakers")})`, kind: "desktop", is_default: false, captures_output: "wasapi:speakers" },
          { id: "output:wasapi:speakers", name: nameOf("wasapi:speakers"), kind: "loopback", is_default: true, captures_output: "wasapi:speakers" },
          { id: "output:wasapi:headset", name: nameOf("wasapi:headset"), kind: "loopback", is_default: false, captures_output: "wasapi:headset" },
          { id: "input:wasapi:mic", name: "Microphone (Shure MV7)", kind: "capture", is_default: true, captures_output: null },
        ],
        outputs: [
          output("wasapi:speakers", true),
          output("wasapi:headset"),
          output("wasapi:cable"),
          output("wasapi:hdmi"),
          output("wasapi:vm"),
          output("wasapi:dock"),
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
    rename_output: ({ id, description }) => {
      demo.names[id][0] = description;
    },
    set_source: ({ id }) => {
      demo.config.source = id;
    },
    set_output_volume: ({ id, fader }) => {
      const o = demo.config.outputs.find((x) => x.id === id);
      if (o) o.fader = fader;
    },
    set_output_muted: ({ id, muted }) => {
      const o = demo.config.outputs.find((x) => x.id === id);
      if (o) o.muted = muted;
    },
    set_autostart: ({ enabled }) => enabled,
  };
  // The handler matches `cmd`, which TypeScript cannot follow through the lookup.
  const handler = /** @type {((args: unknown) => any) | undefined} */ (handlers[cmd]);
  return Promise.resolve(handler?.(args[0]));
}
