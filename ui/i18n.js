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
      "output.volume": "{name} volume",
      "output.level": "{name} level",
      "output.mute": "Mute",
      "output.muteLabel": "Mute {name}",
      "output.muted": "Muted",
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
      "output.volume": "Volume de {name}",
      "output.level": "Niveau de {name}",
      "output.mute": "Couper",
      "output.muteLabel": "Couper le son de {name}",
      "output.muted": "Son coupé",
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

  // Engine messages carry a stable code (`audio::message` on the Rust side)
  // and their English wording. The code is the translation key; a message
  // without a translation falls back to the English the engine sent.
  /** @type {Record<string, Record<string, string>>} */
  const ENGINE = {
    fr: {
      "capturedBySource": "Capturée par la source",
      "deviceDisconnected": "Périphérique déconnecté",
      "deviceUnavailable": "Périphérique indisponible",
      "unknownSource": "Source inconnue",
      "invalidDeviceName": "Nom de périphérique invalide",
      "resampler": "Impossible de créer le rééchantillonneur",
      "pulseUnavailable": "PulseAudio n’est pas disponible",
      "serverInfo": "Impossible d’obtenir les informations du serveur",
      "sourceInfo": "Une erreur s’est produite en lisant les informations de la source",
      "sampleSpec": "Format d’échantillonnage non valide",
      "streamCreate": "Impossible de créer le flux",
      "streamConnect": "Impossible de se connecter au flux",
      "screenPermission":
        "L’autorisation Enregistrement de l’écran est nécessaire pour capturer le son du bureau",
      "screenTimeout": "Délai dépassé pour la capture de l’écran",
      "mainDisplay": "Écran principal introuvable",
      "videoStreamOutput": "Impossible d’ajouter la sortie vidéo du flux",
      "audioStreamOutput": "Impossible d’ajouter la sortie audio du flux",
      "captureStart": "Impossible de démarrer la capture",
      "streamStopped": "Flux arrêté avec l’erreur",
      "waitingForDevice": "En attente du périphérique",
      "queueCreate": "Impossible de créer la file audio",
      "outputUnavailable": "Périphérique de sortie indisponible",
      "queueVolume": "Impossible de régler le volume de la file audio",
      "queueBuffers": "Impossible d’allouer les tampons audio",
      "queueStart": "Impossible de démarrer la file audio",
      "wasapiEnumerator": "Impossible de créer l’énumérateur",
      "wasapiDefaultEndpoint": "Impossible d’obtenir le périphérique par défaut",
      "wasapiEnumerateDevice": "Impossible d’énumérer le périphérique",
      "wasapiGetDevice": "Impossible d’obtenir le périphérique",
      "wasapiActivate": "Impossible d’activer le périphérique",
      "wasapiActivateClient": "Impossible d’activer le client audio",
      "wasapiMixFormat": "Impossible d’obtenir le format de mixage",
      "wasapiInitialize": "Impossible d’initialiser le client audio",
      "wasapiCaptureClient": "Impossible de créer le contexte de capture",
      "wasapiEventHandle": "Impossible de définir l’événement de capture",
      "wasapiStartCapture": "Impossible de démarrer la capture",
      "wasapiBufferSize": "Impossible d’obtenir la taille du tampon",
      "wasapiRenderClient": "Impossible d’obtenir le client de lecture",
      "wasapiGetBuffer": "Impossible d’obtenir le tampon",
      "wasapiStartRender": "Impossible de démarrer la lecture",
      "wasapiMonitorEnumerator": "Impossible de créer l’énumérateur de périphériques",
      "wasapiMonitorInitialize": "Impossible d’initialiser le périphérique",
      "wasapiMonitorRenderClient": "Impossible d’obtenir le client de lecture",
    },
  };

  /**
   * Separator before the untranslatable detail of a message.
   * @type {Record<string, string>}
   */
  const COLON = { fr: `${NNBSP}: ` };

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
   * Translates a status message coming from the audio engine. An unknown
   * code keeps the English wording the engine sent with it.
   * @param {EngineMessage | null} message
   */
  function engineMessage(message) {
    if (!message) return "";
    const text = (ENGINE[lang] || {})[message.code] || message.text;
    if (!message.detail) return text;
    return text ? text + (COLON[lang] || ": ") + message.detail : message.detail;
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
    /** The keys this language carries, so a test can compare two languages. */
    keys: () => Object.keys(strings),
  };
})();
