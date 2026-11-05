//! Files compiled into the executable: the glyphs, the app icon and the
//! Inter font.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

pub const SPEAKER: &str = "icons/speaker.svg";
pub const SPEAKER_OFF: &str = "icons/speaker-off.svg";
pub const CHECK: &str = "icons/check.svg";
pub const CHEVRONS: &str = "icons/chevrons.svg";

/// The app icon in the header and the splash.
pub const APP_ICON: &[u8] = include_bytes!("../../icons/64x64.png");

/// Inter, the interface font outside macOS (SIL OFL, `assets/fonts`).
pub const INTER: &[u8] = include_bytes!("../../assets/fonts/InterVariable.ttf");

const FILES: &[(&str, &[u8])] = &[
    (SPEAKER, include_bytes!("../../assets/icons/speaker.svg")),
    (
        SPEAKER_OFF,
        include_bytes!("../../assets/icons/speaker-off.svg"),
    ),
    (CHECK, include_bytes!("../../assets/icons/check.svg")),
    (CHEVRONS, include_bytes!("../../assets/icons/chevrons.svg")),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(FILES
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(FILES
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| SharedString::from(*p))
            .collect())
    }
}
