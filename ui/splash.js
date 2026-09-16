"use strict";

// Startup update check, shown before the app starts (see updater.rs).

const tauri = window.__TAURI__;
const invoke = tauri ? tauri.core.invoke : demoInvoke;

const POLL_MS = 100;

const STAGE_LABELS = {
  checking: "Checking for updates…",
  downloading: "Downloading update…",
  installing: "Installing update…",
  restarting: "Restarting…",
};

const stage = document.getElementById("stage");
const bar = document.getElementById("bar");

async function poll() {
  let progress;
  try {
    progress = await invoke("update_progress");
  } catch {
    return;
  }
  let text = STAGE_LABELS[progress.stage] ?? STAGE_LABELS.checking;
  const known = progress.stage === "downloading" && progress.percent != null;
  if (known) text = `Downloading update, ${progress.percent}%`;
  if (stage.textContent !== text) stage.textContent = text;
  bar.hidden = !known;
  if (known) {
    bar.setAttribute("aria-valuenow", progress.percent);
    bar.firstElementChild.style.transform = `scaleX(${progress.percent / 100})`;
  }
}

document.addEventListener("contextmenu", (e) => e.preventDefault());

poll();
setInterval(poll, POLL_MS);

/* Demo data, only outside Tauri (browser preview): loops through the stages. */

function demoInvoke() {
  const t = (performance.now() / 1000) % 8;
  if (t < 2) return Promise.resolve({ stage: "checking", percent: null });
  if (t < 6) return Promise.resolve({ stage: "downloading", percent: Math.round(((t - 2) / 4) * 100) });
  if (t < 7) return Promise.resolve({ stage: "installing", percent: null });
  return Promise.resolve({ stage: "restarting", percent: null });
}
