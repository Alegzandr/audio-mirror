import assert from "node:assert/strict";
import { test } from "node:test";

import { i18n } from "./harness.js";

const en = i18n("en");
const fr = i18n("fr");

test("every language carries the same keys", () => {
  const missing = en.keys().filter((k) => !fr.keys().includes(k));
  const extra = fr.keys().filter((k) => !en.keys().includes(k));
  assert.deepEqual([...missing], [], "keys missing from fr");
  assert.deepEqual([...extra], [], "keys in fr that en does not have");
});

test("a string fills its placeholders", () => {
  assert.equal(en.t("update.installed", { version: "1.2.3" }), "Version 1.2.3 is installed");
  assert.equal(en.t("output.volume", { name: "Speakers" }), "Speakers volume");
});

test("plural forms follow the count", () => {
  assert.equal(fr.t("run.mirroring", { count: 1 }), "Diffusion sur 1 sortie");
  assert.equal(fr.t("run.mirroring", { count: 3 }), "Diffusion sur 3 sorties");
});

test("an unknown key is shown as itself rather than as nothing", () => {
  assert.equal(en.t("nope.not.here"), "nope.not.here");
});

test("an engine message is translated by its code", () => {
  const message = { code: "deviceDisconnected", text: "Device disconnected", detail: null };
  assert.equal(fr.engineMessage(message), "Périphérique déconnecté");
  assert.equal(en.engineMessage(message), "Device disconnected");
});

test("an untranslated code keeps the wording the engine sent", () => {
  const message = { code: "somethingNew", text: "A new failure", detail: null };
  assert.equal(fr.engineMessage(message), "A new failure");
});

test("a detail is appended after the message", () => {
  const message = { code: "wasapiGetDevice", text: "Failed to get device", detail: "80070005" };
  assert.equal(en.engineMessage(message), "Failed to get device: 80070005");
  assert.equal(fr.engineMessage(message), "Impossible d’obtenir le périphérique : 80070005");
});

test("a message that is only a detail is shown as it is", () => {
  const message = { code: "system", text: "", detail: "Accès refusé" };
  assert.equal(fr.engineMessage(message), "Accès refusé");
  assert.equal(fr.engineMessage(null), "");
});
