"use strict";

// What the panel shows, worked out from the snapshot and the engine status
// alone: no DOM, no state of its own. `app.js` holds the elements and the
// events; everything here is a function of its arguments, so it can be read
// and tested without a browser. Strings come from `window.I18n`, read at
// call time because the language is picked when that script loads.

(() => {
  const MINUS = "−";
  /** The engine names the desktop source "Default output (<device>)". */
  const DESKTOP_PREFIX = "Default output";
  /** Bottom of the meters, in dB. */
  const FLOOR_DB = -60;
  /** How far a meter falls per frame, once the peak has passed. */
  const FALL = 0.02;

  const i18n = () => window.I18n;

  /**
   * OBS logarithmic fader curve, same as `volume::fader_to_db`.
   * @param {number} def Fader position, 0 to 1.
   */
  function faderToDb(def) {
    if (def >= 1) return 0;
    if (def <= 0) return -Infinity;
    return -102 * Math.pow(17, -def) + 6;
  }

  /** @param {number} db */
  function formatDb(db) {
    if (!Number.isFinite(db) || db <= -96) return `${MINUS}∞ dB`;
    const abs = i18n().formatNumber(Math.abs(db));
    return db < -0.05 ? `${MINUS}${abs} dB` : `${abs} dB`;
  }

  /** @param {number} peak Sample peak, 0 to 1. */
  function peakToDb(peak) {
    return peak > 0 ? 20 * Math.log10(peak) : -Infinity;
  }

  /**
   * Where a meter should stand for that peak, 0 to 1 across the scale.
   * @param {number} peak
   */
  function meterTarget(peak) {
    const db = peakToDb(peak);
    if (!Number.isFinite(db)) return 0;
    return Math.min(1, Math.max(0, (db - FLOOR_DB) / -FLOOR_DB));
  }

  /**
   * The next position of a meter: peaks are taken at once, the fall is
   * smoothed so a meter does not flicker between two frames.
   * @param {number} level
   * @param {number} target
   */
  function meterFall(level, target) {
    return target > level ? target : Math.max(target, level - FALL);
  }

  /** @param {number} level */
  function meterDb(level) {
    return level > 0 ? level * -FLOOR_DB + FLOOR_DB : -Infinity;
  }

  /** @param {SourceInfo} source */
  function sourceName(source) {
    const { t } = i18n();
    let name = source.name;
    if (source.kind === "desktop" && name.startsWith(DESKTOP_PREFIX)) {
      name = t("source.desktop") + name.slice(DESKTOP_PREFIX.length);
    }
    return source.is_default ? t("device.default", { name }) : name;
  }

  /**
   * Every row of the output list: the devices that are here, then the ones
   * turned on that are not, so an unplugged output can be seen and dropped
   * rather than disappearing with its volume.
   * @param {DeviceList} devices
   * @param {AppConfig} config
   * @returns {OutputRow[]}
   */
  function outputRows(devices, config) {
    const present = devices.outputs;
    const absent = config.outputs
      .filter((o) => o.enabled && !present.some((d) => d.id === o.id))
      .map((o) => ({
        id: o.id,
        name: o.name || i18n().t("output.unknown"),
        is_default: false,
        absent: true,
      }));
    return [...present, ...absent];
  }

  /**
   * Output the current source records: playing into it would feed back, so
   * the engine skips it and the panel says why.
   * @param {DeviceList} devices
   * @param {AppConfig} config
   */
  function capturedOutput(devices, config) {
    const src = devices.sources.find((s) => s.id === config.source);
    return src ? src.captures_output : null;
  }

  /**
   * The line in the header: what the engine is doing right now.
   * @param {Status | null} status
   * @returns {{ text: string, tone: string }}
   */
  function runState(status) {
    const { t } = i18n();
    if (!status || !status.running) return { text: t("run.off"), tone: "muted" };
    if (status.source.state === "error") {
      return { text: t("run.sourceError"), tone: "error" };
    }
    const total = status.outputs.length;
    const playing = status.outputs.filter((o) => o.state === "playing").length;
    const text =
      playing === total ? t("run.mirroring", { count: total }) : t("run.partial", { playing, total });
    return { text, tone: "on" };
  }

  /**
   * The state and the explanation of one output row. A muted output that is
   * playing reads as muted: that is what the user did, and it is what they
   * are hearing.
   * @param {OutputStatus | undefined} status Absent while the engine opens it.
   * @param {{ enabled: boolean, muted: boolean }} row
   * @returns {{ label: string, tone: string, detail: string }}
   */
  function outputState(status, row) {
    const { t } = i18n();
    if (!row.enabled || !status) return { label: "", tone: "muted", detail: "" };

    const labels = {
      idle: t("state.idle"),
      starting: t("state.starting"),
      playing: t("state.playing"),
      error: t("state.error"),
      blocked: t("state.blocked"),
    };
    let label = labels[status.state] || status.state;
    let tone = status.state === "playing" ? "on" : "muted";
    if (status.state === "error") tone = "error";
    if (row.muted && status.state === "playing") {
      label = t("output.muted");
      tone = "muted";
    }

    let detail = "";
    if (status.state === "error") detail = retrying(status.message);
    if (status.state === "blocked") detail = t("output.echo");
    return { label, tone, detail };
  }

  /** @param {EngineMessage | null} message */
  function retrying(message) {
    const { t, engineMessage } = i18n();
    return t("error.retrying", { message: engineMessage(message) });
  }

  window.View = {
    faderToDb,
    formatDb,
    peakToDb,
    meterTarget,
    meterFall,
    meterDb,
    sourceName,
    outputRows,
    capturedOutput,
    runState,
    outputState,
    retrying,
  };
})();
