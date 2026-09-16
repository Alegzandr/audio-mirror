"use strict";

// Startup update check, shown before the app starts (see updater.rs).

const tauri = window.__TAURI__;
/** @type {Invoke} */
const invoke = tauri ? tauri.core.invoke : demoInvoke;

const { t } = window.I18n;

const POLL_MS = 100;

/** @type {Record<Progress["stage"], string>} */
const STAGE_LABELS = {
  checking: t("splash.checking"),
  downloading: t("splash.downloading"),
  installing: t("splash.installing"),
  restarting: t("splash.restarting"),
};

const stage = /** @type {HTMLElement} */ (document.getElementById("stage"));
const bar = /** @type {HTMLElement} */ (document.getElementById("bar"));
const fill = /** @type {HTMLElement} */ (bar.firstElementChild);

async function poll() {
  let progress;
  try {
    progress = await invoke("update_progress");
  } catch {
    return;
  }
  let text = STAGE_LABELS[progress.stage] ?? STAGE_LABELS.checking;
  const percent = progress.stage === "downloading" ? progress.percent : null;
  if (percent != null) text = t("splash.downloadingPercent", { percent });
  if (stage.textContent !== text) stage.textContent = text;
  bar.hidden = percent == null;
  if (percent != null) {
    bar.setAttribute("aria-valuenow", String(percent));
    fill.style.transform = `scaleX(${percent / 100})`;
  }
}

document.addEventListener("contextmenu", (e) => e.preventDefault());

poll();
setInterval(poll, POLL_MS);

/* Demo data, only outside Tauri (browser preview): loops through the stages. */

/**
 * @template {Command} K
 * @param {K} _cmd Always `update_progress` here.
 * @param {CommandArgs<K>} _args
 * @returns {Promise<CommandResult<K>>}
 */
function demoInvoke(_cmd, ..._args) {
  const t = (performance.now() / 1000) % 8;
  /** @type {Progress} */
  let progress = { stage: "restarting", percent: null };
  if (t < 2) progress = { stage: "checking", percent: null };
  else if (t < 6) progress = { stage: "downloading", percent: Math.round(((t - 2) / 4) * 100) };
  else if (t < 7) progress = { stage: "installing", percent: null };
  return Promise.resolve(/** @type {any} */ (progress));
}
