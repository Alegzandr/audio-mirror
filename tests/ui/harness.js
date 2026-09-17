// Loads a panel script the way the browser does, into a throwaway global.
//
// `ui/` holds plain scripts that hang an object off `window`, with no build
// step and no module system: the point of the panel is that Tauri serves it
// as it is. So a test runs one in a fresh V8 context, with just enough of a
// window for the script to install itself, and reads back what it exported.

import { readFileSync } from "node:fs";
import { createContext, runInContext } from "node:vm";

// A value that comes back from one of these scripts was built in another
// realm, so its prototype is not this file's: copy an array (`[...xs]`)
// before a strict deep comparison.

const UI = new URL("../../ui/", import.meta.url);

/**
 * Runs `ui/<name>.js` and returns its context, `window` included.
 * @param {string} name
 * @param {Record<string, unknown>} [globals] Extra globals, such as a fake `I18n`.
 */
export function load(name, globals = {}) {
  const sandbox = { ...globals };
  sandbox.window = sandbox;
  sandbox.globalThis = sandbox;
  const context = createContext(sandbox);
  runInContext(readFileSync(new URL(`${name}.js`, UI), "utf8"), context, {
    filename: `ui/${name}.js`,
  });
  return context;
}

/** The real `ui/i18n.js`, in the language asked for. */
export function i18n(lang = "en") {
  return load("i18n", {
    __AUDIO_MIRROR_LANG__: lang,
    document: { documentElement: {}, querySelectorAll: () => [] },
    Intl,
    location: { search: "" },
    navigator: { languages: [lang] },
  }).I18n;
}

/** `ui/view.js`, wired to the real translations. */
export function view(lang = "en") {
  return load("view", { I18n: i18n(lang) }).View;
}
