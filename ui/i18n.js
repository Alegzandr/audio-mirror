"use strict";

// Interface strings. The language is detected from the system locale on the
// Rust side (i18n.rs, which also holds the tray menu strings) and injected as
// `window.__AUDIO_MIRROR_LANG__`. A browser preview uses `?lang=fr` or the
// browser language. Every language must have the same keys as `en`.

(() => {
  const NNBSP = "\u202F";

  /**
   * A string, or its plural forms keyed by `Intl.PluralRules` category.
   * @typedef {string | ({ other: string } & Partial<Record<Intl.LDMLPluralRule, string>>)} Message
   */

  /** @type {Record<string, Record<string, Message>>} */
  const MESSAGES = {
    en: {
      "source.title": "Source",
      "source.level": "Source level",
      "source.group.output": "Output devices",
      "source.group.input": "Input devices",
      "source.disconnected": "Disconnected device",
      "source.desktop": "Default output",
      "outputs.title": "Outputs",
      "outputs.searching": "Looking for audio devices…",
      "outputs.none": "No output device found. Plug one in and it will show up here.",
      "outputs.hint": "Turn on the outputs that should play the source.",
      "output.unknown": "Unknown device",
      "output.volume": "{name} volume",
      "output.level": "{name} level",
      "output.mute": "Mute",
      "output.muteLabel": "Mute {name}",
      "output.muted": "Muted",
      "output.forget": "Forget",
      "output.notConnected": "Not connected",
      "output.echo": "This is the source device. Playing into it would echo endlessly.",
      "device.default": "{name} (default)",
      "state.idle": "Waiting for source",
      "state.starting": "Starting",
      "state.playing": "Playing",
      "state.error": "Unavailable",
      "state.blocked": "Skipped",
      "run.off": "Off",
      "run.sourceError": "Source unavailable",
      "run.mirroring": "Mirroring to {count}",
      "run.partial": "{playing} of {total} playing",
      "error.retrying": "{message}. Retrying.",
      "autostart": "Start with system",
      "update.restart": "Restart to update",
      "update.installed": "Version {version} is installed",
      "quit": "Quit",
      "splash.checking": "Checking for updates…",
      "splash.downloading": "Downloading update…",
      "splash.downloadingPercent": "Downloading update, {percent}%",
      "splash.installing": "Installing update…",
      "splash.restarting": "Restarting…",
      "splash.download": "Download",
    },
    fr: {
      "source.title": "Source",
      "source.level": "Niveau de la source",
      "source.group.output": "Périphériques de sortie",
      "source.group.input": "Périphériques d’entrée",
      "source.disconnected": "Périphérique déconnecté",
      "source.desktop": "Sortie par défaut",
      "outputs.title": "Sorties",
      "outputs.searching": "Recherche des périphériques audio…",
      "outputs.none": "Aucun périphérique de sortie trouvé. Branchez-en un et il apparaîtra ici.",
      "outputs.hint": "Activez les sorties qui doivent lire la source.",
      "output.unknown": "Périphérique inconnu",
      "output.volume": "Volume de {name}",
      "output.level": "Niveau de {name}",
      "output.mute": "Couper",
      "output.muteLabel": "Couper le son de {name}",
      "output.muted": "Son coupé",
      "output.forget": "Oublier",
      "output.notConnected": "Non connectée",
      "output.echo": "C’est le périphérique de la source. Y envoyer le son créerait un écho sans fin.",
      "device.default": "{name} (par défaut)",
      "state.idle": "En attente de la source",
      "state.starting": "Démarrage",
      "state.playing": "En lecture",
      "state.error": "Indisponible",
      "state.blocked": "Ignorée",
      "run.off": "Arrêté",
      "run.sourceError": "Source indisponible",
      "run.mirroring": { one: "Diffusion sur {count} sortie", other: "Diffusion sur {count} sorties" },
      "run.partial": "{playing} sur {total} en lecture",
      "error.retrying": "{message}. Nouvelle tentative.",
      "autostart": "Lancer au démarrage",
      "update.restart": "Mettre à jour",
      "update.installed": "La version {version} est installée",
      "quit": "Quitter",
      "splash.checking": "Recherche de mises à jour…",
      "splash.downloading": "Téléchargement de la mise à jour…",
      "splash.downloadingPercent": `Téléchargement de la mise à jour, {percent}${NNBSP}%`,
      "splash.installing": "Installation de la mise à jour…",
      "splash.restarting": "Redémarrage…",
      "splash.download": "Téléchargement",
    },
  };

  // Engine messages stay in English in the Rust port (they mirror OBS) and
  // are translated here. Unknown messages, such as system errors that are
  // already localized, are shown as they are.
  /**
   * @type {Record<string, {
   *   exact: Record<string, string>,
   *   contexts: Record<string, string>,
   *   patterns: [RegExp, string][],
   *   colon: string,
   * }>}
   */
  const ENGINE = {
    fr: {
      exact: {
        "Device disconnected": "Périphérique déconnecté",
        "Device unavailable": "Périphérique indisponible",
        "Output device unavailable": "Périphérique de sortie indisponible",
        "Waiting for the device": "En attente du périphérique",
        "Captured by the source": "Capturée par la source",
        "Screen Recording permission is required for desktop audio":
          "L’autorisation Enregistrement de l’écran est nécessaire pour capturer le son du bureau",
        "Screen capture content timed out": "Délai dépassé pour la capture de l’écran",
        "Main display not found": "Écran principal introuvable",
        "Failed to start capture": "Impossible de démarrer la capture",
        "Failed to create resampler": "Impossible de créer le rééchantillonneur",
        "Failed to create the audio queue": "Impossible de créer la file audio",
        "Failed to set the queue volume": "Impossible de régler le volume de la file audio",
        "Failed to allocate audio buffers": "Impossible d’allouer les tampons audio",
        "Failed to start the audio queue": "Impossible de démarrer la file audio",
        "PulseAudio is not available": "PulseAudio n’est pas disponible",
        "Unable to get server info": "Impossible d’obtenir les informations du serveur",
        "An error occurred while getting the source info":
          "Une erreur s’est produite en lisant les informations de la source",
        "Sample spec is not valid": "Format d’échantillonnage non valide",
        "Unable to create stream": "Impossible de créer le flux",
        "Unable to connect to stream": "Impossible de se connecter au flux",
      },
      // `hr()` in wasapi.rs: "<context>: <HRESULT>".
      contexts: {
        "Failed to create enumerator": "Impossible de créer l’énumérateur",
        "Failed to create IMMDeviceEnumerator": "Impossible de créer l’énumérateur de périphériques",
        "Failed GetDefaultAudioEndpoint": "Impossible d’obtenir le périphérique par défaut",
        "Failed to enumerate device": "Impossible d’énumérer le périphérique",
        "Failed to get device": "Impossible d’obtenir le périphérique",
        "Failed to activate device": "Impossible d’activer le périphérique",
        "Failed to activate client context": "Impossible d’activer le client audio",
        "Failed to get mix format": "Impossible d’obtenir le format de mixage",
        "Failed to initialize audio client": "Impossible d’initialiser le client audio",
        "Failed to initialize": "Impossible d’initialiser le périphérique",
        "Failed to create capture context": "Impossible de créer le contexte de capture",
        "Failed to set event handle": "Impossible de définir l’événement de capture",
        "Failed to start capture client": "Impossible de démarrer la capture",
        "Failed to get buffer size": "Impossible d’obtenir la taille du tampon",
        "Failed to get render client": "Impossible d’obtenir le client de lecture",
        "Failed to get IAudioRenderClient": "Impossible d’obtenir le client de lecture",
        "Failed to get buffer": "Impossible d’obtenir le tampon",
        "Failed to start audio": "Impossible de démarrer la lecture",
        "Failed to add video stream output": "Impossible d’ajouter la sortie vidéo du flux",
        "Failed to add audio stream output": "Impossible d’ajouter la sortie audio du flux",
      },
      patterns: [
        [/^Unknown source (.*)$/s, "Source inconnue {1}"],
        [/^Invalid device name (.*)$/s, "Nom de périphérique invalide {1}"],
        [/^Stream stopped with error (.*)$/s, "Flux arrêté avec l’erreur {1}"],
      ],
      colon: `${NNBSP}: `,
    },
  };

  /** @returns {string} */
  function pick() {
    const injected = window.__AUDIO_MIRROR_LANG__;
    if (injected && MESSAGES[injected]) return injected;
    if (!window.__TAURI__) {
      const wanted = [new URLSearchParams(location.search).get("lang"), ...navigator.languages];
      for (const tag of wanted) {
        const primary = (tag || "").toLowerCase().split(/[-_]/)[0];
        if (MESSAGES[primary]) return primary;
      }
    }
    return "en";
  }

  const lang = pick();
  const strings = MESSAGES[lang];
  const numbers = new Intl.NumberFormat(lang, { minimumFractionDigits: 1, maximumFractionDigits: 1 });
  const plurals = new Intl.PluralRules(lang);

  /**
   * Looks up `key` and fills `{name}` placeholders from `vars`. A string with
   * plural forms (`{ one, other }`) is chosen by `vars.count`.
   * @param {string} key
   * @param {Record<string, string | number>} [vars]
   */
  function t(key, vars) {
    const entry = strings[key] ?? MESSAGES.en[key] ?? key;
    const text = typeof entry === "string" ? entry : entry[plurals.select(Number(vars?.count ?? 0))] ?? entry.other;
    return vars ? fill(text, vars) : text;
  }

  /**
   * @param {string} text
   * @param {Record<string, string | number>} vars
   */
  function fill(text, vars) {
    return text.replace(/\{(\w+)\}/g, (m, k) => (k in vars ? String(vars[k]) : m));
  }

  /**
   * Translates a status message coming from the audio engine.
   * @param {string | null} message
   */
  function engineMessage(message) {
    const table = ENGINE[lang];
    if (!message) return "";
    if (!table) return message;
    if (table.exact[message]) return table.exact[message];
    const sep = message.lastIndexOf(": ");
    if (sep > 0) {
      const context = table.contexts[message.slice(0, sep)];
      if (context) return context + table.colon + message.slice(sep + 2);
    }
    for (const [re, template] of table.patterns) {
      const m = message.match(re);
      if (m) return fill(template, Object.fromEntries(m.entries()));
    }
    return message;
  }

  /**
   * Translates the static markup: `data-i18n` sets the text, `data-i18n-<attr>` an attribute.
   * @param {ParentNode} root
   */
  function apply(root) {
    document.documentElement.lang = lang;
    for (const el of root.querySelectorAll("*")) {
      for (const { name, value } of [...el.attributes]) {
        if (name === "data-i18n") el.textContent = t(value);
        else if (name.startsWith("data-i18n-")) el.setAttribute(name.slice(10), t(value));
      }
    }
  }

  apply(document);
  for (const tpl of document.querySelectorAll("template")) apply(tpl.content);

  window.I18n = {
    lang,
    t,
    engineMessage,
    formatNumber: (n) => numbers.format(n),
  };
})();
