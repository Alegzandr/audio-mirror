//! Interface language, chosen once at startup from the system locale.
//!
//! The same choice drives the tray menu (here) and the web pages: every
//! webview gets it as `window.__AUDIO_MIRROR_LANG__` before its scripts run,
//! and `ui/i18n.js` holds the page strings.

use tauri::plugin::TauriPlugin;
use tauri::Runtime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Fr,
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

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Fr => "fr",
        }
    }

    pub fn tray(self) -> TrayText {
        match self {
            Lang::En => TrayText {
                open: "Open Audio Mirror",
                restart: "Restart audio",
                quit: "Quit",
            },
            Lang::Fr => TrayText {
                open: "Ouvrir Audio Mirror",
                restart: "Redémarrer l’audio",
                quit: "Quitter",
            },
        }
    }
}

pub struct TrayText {
    pub open: &'static str,
    pub restart: &'static str,
    pub quit: &'static str,
}

/// Hands the language to every webview before its scripts run.
pub fn plugin<R: Runtime>(lang: Lang) -> TauriPlugin<R> {
    tauri::plugin::Builder::new("i18n")
        .js_init_script(format!(
            "window.__AUDIO_MIRROR_LANG__ = \"{}\";",
            lang.code()
        ))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
