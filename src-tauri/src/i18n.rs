//! Interface language, chosen once at startup from the system locale, and
//! every string the interface shows.
//!
//! Each language has the same keys (a test checks it). Engine messages are
//! translated by the stable code they carry (`audio::message`); a code
//! without a translation keeps the English wording the engine sent.

use crate::audio::message::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Fr,
}

/// U+202F, the narrow no-break space French puts before `:` and `%`.
const NNBSP: &str = "\u{202F}";

const EN: &[(&str, &str)] = &[
    ("source.title", "Source"),
    ("source.group.output", "Output devices"),
    ("source.group.input", "Input devices"),
    ("source.disconnected", "Disconnected device"),
    ("source.desktop", "Default output"),
    ("outputs.title", "Outputs"),
    ("outputs.searching", "Looking for audio devices…"),
    (
        "outputs.none",
        "No output device found. Plug one in and it will show up here.",
    ),
    (
        "outputs.hint",
        "Add the outputs that should play the source.",
    ),
    ("outputs.edit", "Edit"),
    ("outputs.done", "Done"),
    ("outputs.others.one", "{count} other device"),
    ("outputs.others.other", "{count} other devices"),
    ("outputs.show", "Show"),
    ("outputs.hide", "Hide"),
    ("output.add", "Add"),
    ("output.remove", "Remove"),
    ("output.rename", "Rename"),
    ("output.renameFailed", "The system refused the new name."),
    ("output.mute", "Mute"),
    ("output.muted", "Muted"),
    (
        "output.echo",
        "This is the source device. Playing into it would echo endlessly.",
    ),
    ("device.default", "{name} (default)"),
    ("state.idle", "Waiting for source"),
    ("state.starting", "Starting"),
    ("state.playing", "Playing"),
    ("state.error", "Unavailable"),
    ("state.blocked", "Skipped"),
    ("run.off", "Off"),
    ("run.sourceError", "Source unavailable"),
    ("run.mirroring.one", "Mirroring to {count}"),
    ("run.mirroring.other", "Mirroring to {count}"),
    ("run.partial", "{playing} of {total} playing"),
    ("error.retrying", "{message}. Retrying."),
    ("autostart", "Start with system"),
    ("update.restart", "Update"),
    ("update.installed", "Version {version} is installed"),
    ("quit", "Quit"),
    ("splash.checking", "Checking for updates…"),
    ("splash.downloading", "Downloading update…"),
    (
        "splash.downloadingPercent",
        "Downloading update, {percent}%",
    ),
    ("splash.installing", "Installing update…"),
    ("splash.restarting", "Restarting…"),
    ("tray.open", "Open Audio Mirror"),
    ("tray.restart", "Restart audio"),
    ("tray.quit", "Quit"),
];

const FR: &[(&str, &str)] = &[
    ("source.title", "Source"),
    ("source.group.output", "Périphériques de sortie"),
    ("source.group.input", "Périphériques d’entrée"),
    ("source.disconnected", "Périphérique déconnecté"),
    ("source.desktop", "Sortie par défaut"),
    ("outputs.title", "Sorties"),
    ("outputs.searching", "Recherche des périphériques audio…"),
    (
        "outputs.none",
        "Aucun périphérique de sortie trouvé. Branchez-en un et il apparaîtra ici.",
    ),
    (
        "outputs.hint",
        "Ajoutez les sorties qui doivent lire la source.",
    ),
    ("outputs.edit", "Modifier"),
    ("outputs.done", "Terminé"),
    ("outputs.others.one", "{count} autre périphérique"),
    ("outputs.others.other", "{count} autres périphériques"),
    ("outputs.show", "Afficher"),
    ("outputs.hide", "Masquer"),
    ("output.add", "Ajouter"),
    ("output.remove", "Retirer"),
    ("output.rename", "Renommer"),
    ("output.renameFailed", "Le système a refusé le nouveau nom."),
    ("output.mute", "Couper"),
    ("output.muted", "Son coupé"),
    (
        "output.echo",
        "C’est le périphérique de la source. Y envoyer le son créerait un écho sans fin.",
    ),
    ("device.default", "{name} (par défaut)"),
    ("state.idle", "En attente de la source"),
    ("state.starting", "Démarrage"),
    ("state.playing", "En lecture"),
    ("state.error", "Indisponible"),
    ("state.blocked", "Ignorée"),
    ("run.off", "Arrêté"),
    ("run.sourceError", "Source indisponible"),
    ("run.mirroring.one", "Diffusion sur {count} sortie"),
    ("run.mirroring.other", "Diffusion sur {count} sorties"),
    ("run.partial", "{playing} sur {total} en lecture"),
    ("error.retrying", "{message}. Nouvelle tentative."),
    ("autostart", "Lancer au démarrage"),
    ("update.restart", "Mettre à jour"),
    ("update.installed", "La version {version} est installée"),
    ("quit", "Quitter"),
    ("splash.checking", "Recherche de mises à jour…"),
    ("splash.downloading", "Téléchargement de la mise à jour…"),
    (
        "splash.downloadingPercent",
        "Téléchargement de la mise à jour, {percent}\u{202F}%",
    ),
    ("splash.installing", "Installation de la mise à jour…"),
    ("splash.restarting", "Redémarrage…"),
    ("tray.open", "Ouvrir Audio Mirror"),
    ("tray.restart", "Redémarrer l’audio"),
    ("tray.quit", "Quitter"),
];

/// Engine messages by code. English needs no table: the engine sends it.
const ENGINE_FR: &[(&str, &str)] = &[
    ("capturedBySource", "Capturée par la source"),
    ("deviceDisconnected", "Périphérique déconnecté"),
    ("deviceUnavailable", "Périphérique indisponible"),
    ("unknownSource", "Source inconnue"),
    ("invalidDeviceName", "Nom de périphérique invalide"),
    ("resampler", "Impossible de créer le rééchantillonneur"),
    ("pulseUnavailable", "PulseAudio n’est pas disponible"),
    (
        "serverInfo",
        "Impossible d’obtenir les informations du serveur",
    ),
    (
        "sourceInfo",
        "Une erreur s’est produite en lisant les informations de la source",
    ),
    ("sampleSpec", "Format d’échantillonnage non valide"),
    ("streamCreate", "Impossible de créer le flux"),
    ("streamConnect", "Impossible de se connecter au flux"),
    (
        "screenPermission",
        "L’autorisation Enregistrement de l’écran est nécessaire pour capturer le son du bureau",
    ),
    ("screenTimeout", "Délai dépassé pour la capture de l’écran"),
    ("mainDisplay", "Écran principal introuvable"),
    (
        "videoStreamOutput",
        "Impossible d’ajouter la sortie vidéo du flux",
    ),
    (
        "audioStreamOutput",
        "Impossible d’ajouter la sortie audio du flux",
    ),
    ("captureStart", "Impossible de démarrer la capture"),
    ("streamStopped", "Flux arrêté avec l’erreur"),
    ("waitingForDevice", "En attente du périphérique"),
    ("queueCreate", "Impossible de créer la file audio"),
    ("outputUnavailable", "Périphérique de sortie indisponible"),
    (
        "queueVolume",
        "Impossible de régler le volume de la file audio",
    ),
    ("queueBuffers", "Impossible d’allouer les tampons audio"),
    ("queueStart", "Impossible de démarrer la file audio"),
    ("wasapiEnumerator", "Impossible de créer l’énumérateur"),
    (
        "wasapiDefaultEndpoint",
        "Impossible d’obtenir le périphérique par défaut",
    ),
    (
        "wasapiEnumerateDevice",
        "Impossible d’énumérer le périphérique",
    ),
    ("wasapiGetDevice", "Impossible d’obtenir le périphérique"),
    ("wasapiActivate", "Impossible d’activer le périphérique"),
    (
        "wasapiActivateClient",
        "Impossible d’activer le client audio",
    ),
    (
        "wasapiMixFormat",
        "Impossible d’obtenir le format de mixage",
    ),
    (
        "wasapiInitialize",
        "Impossible d’initialiser le client audio",
    ),
    (
        "wasapiCaptureClient",
        "Impossible de créer le contexte de capture",
    ),
    (
        "wasapiEventHandle",
        "Impossible de définir l’événement de capture",
    ),
    ("wasapiStartCapture", "Impossible de démarrer la capture"),
    (
        "wasapiBufferSize",
        "Impossible d’obtenir la taille du tampon",
    ),
    (
        "wasapiRenderClient",
        "Impossible d’obtenir le client de lecture",
    ),
    ("wasapiGetBuffer", "Impossible d’obtenir le tampon"),
    ("wasapiStartRender", "Impossible de démarrer la lecture"),
    (
        "wasapiMonitorEnumerator",
        "Impossible de créer l’énumérateur de périphériques",
    ),
    (
        "wasapiMonitorInitialize",
        "Impossible d’initialiser le périphérique",
    ),
    (
        "wasapiMonitorRenderClient",
        "Impossible d’obtenir le client de lecture",
    ),
];

fn lookup(table: &[(&str, &'static str)], key: &str) -> Option<&'static str> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// Fills `{name}` placeholders from `vars`; unknown ones stay as they are.
fn fill(text: &str, vars: &[(&str, &str)]) -> String {
    let mut out = text.to_owned();
    for (k, v) in vars {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

impl Lang {
    /// First supported language in the user's preferred list, else English.
    pub fn detect() -> Self {
        sys_locale::get_locales()
            .find_map(|tag| Self::from_tag(&tag))
            .unwrap_or(Lang::En)
    }

    /// Reads a BCP 47 or POSIX locale tag (`fr-CA`, `fr_FR.UTF-8`).
    fn from_tag(tag: &str) -> Option<Self> {
        let primary = tag.split(['-', '_', '.', '@']).next()?;
        match primary.to_ascii_lowercase().as_str() {
            "en" => Some(Lang::En),
            "fr" => Some(Lang::Fr),
            _ => None,
        }
    }

    fn table(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Lang::En => EN,
            Lang::Fr => FR,
        }
    }

    /// The string for `key`, in English when this language lacks it, and
    /// the key itself when no language has it.
    pub fn t(self, key: &'static str) -> &'static str {
        lookup(self.table(), key)
            .or_else(|| lookup(EN, key))
            .unwrap_or(key)
    }

    /// [`Lang::t`] with its `{placeholders}` filled.
    pub fn tf(self, key: &'static str, vars: &[(&str, &str)]) -> String {
        fill(self.t(key), vars)
    }

    /// A string with plural forms (`key.one`, `key.other`), chosen by
    /// `count`, which also fills `{count}`.
    pub fn plural(self, key: &'static str, count: usize, vars: &[(&str, &str)]) -> String {
        let one = match self {
            Lang::En => count == 1,
            // CLDR: French uses the singular for 0 and 1.
            Lang::Fr => count <= 1,
        };
        let form = format!("{key}.{}", if one { "one" } else { "other" });
        let text = lookup(self.table(), &form)
            .or_else(|| lookup(EN, &form))
            .unwrap_or(key);
        let count = count.to_string();
        let mut all = vec![("count", count.as_str())];
        all.extend_from_slice(vars);
        fill(text, &all)
    }

    /// A number with one decimal, in the reader's convention.
    pub fn number(self, n: f32) -> String {
        let text = format!("{n:.1}");
        match self {
            Lang::En => text,
            Lang::Fr => text.replace('.', ","),
        }
    }

    /// Translates a status message from the audio engine, keeping the
    /// detail it carries as is.
    pub fn engine(self, message: &Message) -> String {
        let text = match self {
            Lang::En => message.text,
            Lang::Fr => lookup(ENGINE_FR, message.code).unwrap_or(message.text),
        };
        let Some(detail) = &message.detail else {
            return text.to_owned();
        };
        if text.is_empty() {
            return detail.clone();
        }
        let colon = match self {
            Lang::En => ": ".to_owned(),
            Lang::Fr => format!("{NNBSP}: "),
        };
        format!("{text}{colon}{detail}")
    }

    /// Whether an engine message has a translation, for the test that keeps
    /// `audio::message` and this table in step.
    #[cfg(test)]
    pub fn translates(self, code: &str) -> bool {
        match self {
            Lang::En => true,
            Lang::Fr => lookup(ENGINE_FR, code).is_some(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::message;

    #[test]
    fn reads_bcp47_and_posix_tags() {
        assert_eq!(Lang::from_tag("fr"), Some(Lang::Fr));
        assert_eq!(Lang::from_tag("fr-CA"), Some(Lang::Fr));
        assert_eq!(Lang::from_tag("fr_FR.UTF-8"), Some(Lang::Fr));
        assert_eq!(Lang::from_tag("FR-be"), Some(Lang::Fr));
        assert_eq!(Lang::from_tag("en-US"), Some(Lang::En));
        assert_eq!(Lang::from_tag("de-DE"), None);
        assert_eq!(Lang::from_tag(""), None);
    }

    #[test]
    fn every_language_has_the_same_keys() {
        fn keys(t: &[(&'static str, &'static str)]) -> Vec<&'static str> {
            let mut k: Vec<&str> = t.iter().map(|(k, _)| *k).collect();
            k.sort_unstable();
            k
        }
        assert_eq!(keys(EN), keys(FR));
        let mut all = keys(EN);
        let n = all.len();
        all.dedup();
        assert_eq!(all.len(), n, "a key is listed twice");
    }

    #[test]
    fn no_em_dash_in_the_interface() {
        for (k, v) in EN.iter().chain(FR).chain(ENGINE_FR) {
            assert!(!v.contains('\u{2014}'), "em dash in {k}");
        }
    }

    #[test]
    fn plurals_follow_each_language() {
        assert_eq!(Lang::En.plural("outputs.others", 1, &[]), "1 other device");
        assert_eq!(Lang::En.plural("outputs.others", 0, &[]), "0 other devices");
        assert_eq!(
            Lang::Fr.plural("outputs.others", 0, &[]),
            "0 autre périphérique"
        );
        assert_eq!(
            Lang::Fr.plural("run.mirroring", 2, &[]),
            "Diffusion sur 2 sorties"
        );
    }

    #[test]
    fn engine_messages_keep_their_detail() {
        let m: Message = message::DEVICE_DISCONNECTED.into();
        assert_eq!(Lang::Fr.engine(&m), "Périphérique déconnecté");
        let m = message::WASAPI_GET_DEVICE.detail("80070005");
        assert_eq!(Lang::En.engine(&m), "Failed to get device: 80070005");
        assert_eq!(
            Lang::Fr.engine(&m),
            "Impossible d’obtenir le périphérique\u{202F}: 80070005"
        );
        let m = message::SYSTEM.detail("Accès refusé");
        assert_eq!(Lang::Fr.engine(&m), "Accès refusé");
    }

    #[test]
    fn numbers_use_the_reader_decimal_mark() {
        assert_eq!(Lang::En.number(18.74), "18.7");
        assert_eq!(Lang::Fr.number(18.74), "18,7");
    }
}
