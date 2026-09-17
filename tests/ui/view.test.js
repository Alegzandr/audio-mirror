import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { view } from "./harness.js";

const en = view("en");
const fr = view("fr");

/** @param {Partial<OutputStatus>} [over] */
const outputStatus = (over) => ({
  id: "out",
  state: "playing",
  message: null,
  format: "48 kHz, stereo",
  peak: 0,
  ...over,
});

const status = (over) => ({
  running: true,
  devices_revision: 0,
  source: { state: "playing", message: null, format: "48 kHz, stereo", peak: 0 },
  outputs: [],
  ...over,
});

/* The fader, which has to agree with `volume::fader_to_db` in the engine. */

test("the fader curve matches the one the engine applies", () => {
  assert.equal(en.faderToDb(1), 0);
  assert.equal(en.faderToDb(0), -Infinity);
  // The points the Rust test checks `volume::fader_to_db` against.
  const { points } = JSON.parse(readFileSync(new URL("../fader-curve.json", import.meta.url), "utf8"));
  for (const [def, want] of points) {
    const db = en.faderToDb(def);
    if (want === null) assert.equal(db, -Infinity, `fader ${def}`);
    else assert.ok(Math.abs(db - want) < 0.01, `fader ${def}: ${db} dB, expected ${want}`);
  }
  let previous = -Infinity;
  for (let i = 1; i <= 100; i++) {
    const db = en.faderToDb(i / 100);
    assert.ok(db >= previous, `the curve dips at ${i}`);
    previous = db;
  }
});

test("decibels are written for the reader, in their language", () => {
  assert.equal(en.formatDb(0), "0.0 dB", "one decimal, always, so the readout does not jitter");
  assert.equal(en.formatDb(-18.74), "−18.7 dB");
  assert.equal(fr.formatDb(-18.74), "−18,7 dB");
  assert.equal(en.formatDb(-Infinity), "−∞ dB");
  // Anything under the engine's floor is silence, not a number.
  assert.equal(en.formatDb(-120), "−∞ dB");
});

/* Meters. */

test("a peak lands on the meter where its level says", () => {
  assert.equal(en.meterTarget(1), 1);
  assert.equal(en.meterTarget(0), 0, "silence is the bottom of the scale");
  assert.ok(Math.abs(en.meterTarget(0.5) - 0.9) < 0.01, "half scale is about -6 dB");
  assert.equal(en.meterTarget(1e-9), 0, "below the floor is not a negative bar");
});

test("a meter jumps to a peak and falls back slowly", () => {
  assert.equal(en.meterFall(0.2, 0.9), 0.9, "the peak is taken at once");
  assert.ok(en.meterFall(0.9, 0) < 0.9, "and it falls afterwards");
  assert.ok(en.meterFall(0.9, 0) > 0.8, "but not in one frame");
  assert.equal(en.meterFall(0.005, 0), 0, "it settles on the target, never under");
});

test("the meter reading matches the peak that moved it", () => {
  assert.equal(en.meterDb(0), -Infinity);
  assert.equal(en.meterDb(1), 0);
  assert.ok(Math.abs(en.meterDb(en.meterTarget(0.5)) - en.peakToDb(0.5)) < 0.001);
});

/* Sources. */

test("the desktop source is named in the reader's language", () => {
  const desktop = {
    id: "desktop",
    name: "Default output (Speakers)",
    kind: "desktop",
    is_default: false,
    captures_output: "out",
  };
  assert.equal(fr.sourceName(desktop), "Sortie par défaut (Speakers)");
  assert.equal(en.sourceName(desktop), "Default output (Speakers)");
});

test("a device that is the system default says so", () => {
  const mic = { id: "input:mic", name: "Mic", kind: "capture", is_default: true, captures_output: null };
  assert.equal(en.sourceName(mic), "Mic (default)");
  assert.equal(fr.sourceName(mic), "Mic (par défaut)");
});

/* The output list. */

const devices = {
  sources: [
    { id: "desktop", name: "Default output (A)", kind: "desktop", is_default: false, captures_output: "a" },
    { id: "output:b", name: "B", kind: "loopback", is_default: false, captures_output: "b" },
  ],
  outputs: [
    { id: "a", name: "A", is_default: true },
    { id: "b", name: "B", is_default: false },
  ],
};

const config = (outputs, source = "desktop") => ({ source, outputs });

test("the outputs are the devices that are here, in the order the system gave them", () => {
  const rows = en.outputRows(devices, config([], "input:mic"));
  assert.deepEqual([...rows].map((r) => r.id), ["a", "b"]);
});

test("the output the source records is not listed", () => {
  assert.deepEqual([...en.outputRows(devices, config([], "desktop"))].map((r) => r.id), ["b"]);
  assert.deepEqual([...en.outputRows(devices, config([], "output:b"))].map((r) => r.id), ["a"]);
});

test("an unplugged output is not listed, even when it is on", () => {
  const rows = en.outputRows(devices, config([{ id: "gone", name: "Old DAC", enabled: true, fader: 1, muted: false }], "input:mic"));
  assert.deepEqual([...rows].map((r) => r.id), ["a", "b"]);
});

test("the output the source records is the one that cannot be played into", () => {
  assert.equal(en.capturedOutput(devices, config([], "desktop")), "a");
  assert.equal(en.capturedOutput(devices, config([], "output:b")), "b");
  assert.equal(en.capturedOutput(devices, config([], "input:mic")), null);
});

/* The header. */

test("the header says what the engine is doing", () => {
  const rows = [{ id: "out", name: "Out", is_default: false }, { id: "b", name: "B", is_default: false }];
  assert.deepEqual({ ...en.runState(null, rows) }, { text: "Off", tone: "muted" });
  assert.deepEqual({ ...en.runState(status({ running: false }), rows) }, { text: "Off", tone: "muted" });

  const failing = status({ source: { state: "error", message: null, format: null, peak: 0 } });
  assert.deepEqual({ ...en.runState(failing, rows) }, { text: "Source unavailable", tone: "error" });

  const two = status({ outputs: [outputStatus(), outputStatus({ id: "b" })] });
  assert.deepEqual({ ...en.runState(two, rows) }, { text: "Mirroring to 2", tone: "on" });

  const half = status({ outputs: [outputStatus(), outputStatus({ id: "b", state: "error" })] });
  assert.deepEqual({ ...en.runState(half, rows) }, { text: "1 of 2 playing", tone: "on" });
});

test("the header only counts the outputs the list shows", () => {
  const gone = status({ outputs: [outputStatus(), outputStatus({ id: "gone", state: "error" })] });
  assert.deepEqual({ ...en.runState(gone, [{ id: "out", name: "Out", is_default: false }]) }, { text: "Mirroring to 1", tone: "on" });
  assert.deepEqual({ ...en.runState(gone, []) }, { text: "Off", tone: "muted" });
});

/* One output row. */

const row = (over) => ({ enabled: true, muted: false, ...over });

test("an output that is off shows nothing at all", () => {
  assert.deepEqual({ ...en.outputState(outputStatus(), row({ enabled: false })) }, {
    label: "",
    tone: "muted",
    detail: "",
  });
});

test("an output the engine has not reached yet shows nothing either", () => {
  assert.deepEqual({ ...en.outputState(undefined, row()) }, { label: "", tone: "muted", detail: "" });
});

test("a playing output says so, and a muted one says it is muted", () => {
  assert.deepEqual({ ...en.outputState(outputStatus(), row()) }, {
    label: "Playing",
    tone: "on",
    detail: "",
  });
  assert.deepEqual({ ...en.outputState(outputStatus(), row({ muted: true })) }, {
    label: "Muted",
    tone: "muted",
    detail: "",
  });
});

test("a muted output that is not playing reports the real state, not the mute", () => {
  const state = en.outputState(outputStatus({ state: "starting" }), row({ muted: true }));
  assert.equal(state.label, "Starting");
});

test("a failed output explains itself in the reader's language", () => {
  const failed = outputStatus({
    state: "error",
    message: { code: "deviceDisconnected", text: "Device disconnected", detail: null },
  });
  const state = fr.outputState(failed, row());
  assert.equal(state.label, "Indisponible");
  assert.equal(state.tone, "error");
  assert.equal(state.detail, "Périphérique déconnecté. Nouvelle tentative.");
});

test("an output the source records explains the echo instead of an error", () => {
  const blocked = outputStatus({
    state: "blocked",
    message: { code: "capturedBySource", text: "Captured by the source", detail: null },
  });
  const state = en.outputState(blocked, row());
  assert.equal(state.tone, "muted");
  assert.equal(state.detail, "This is the source device. Playing into it would echo endlessly.");
});
