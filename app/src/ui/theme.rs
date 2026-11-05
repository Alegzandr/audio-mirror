//! Design tokens (`DESIGN.md`): translucent inks and fills over the window's
//! glass, one system blue, one red. The tint under everything depends on
//! the material the system draws behind the window (`native.rs`).

use gpui::{Hsla, Rgba, WindowAppearance};

/// What the panel sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glass {
    /// `NSGlassEffectView`, macOS 26 and later: it carries its own tint.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Liquid,
    /// Vibrancy (older macOS) or Acrylic (Windows).
    Native,
    /// No system material (Linux): the tint becomes an opaque ground.
    None,
}

fn rgba(r: u8, g: u8, b: u8, a: f32) -> Hsla {
    Rgba {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
        a,
    }
    .into()
}

fn black(a: f32) -> Hsla {
    rgba(0, 0, 0, a)
}

fn white(a: f32) -> Hsla {
    rgba(255, 255, 255, a)
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub tint: Hsla,
    pub platter: Hsla,
    pub rim: Hsla,
    pub edge: Hsla,
    pub hairline: Hsla,
    pub fill: Hsla,
    pub fill_active: Hsla,
    pub track: Hsla,
    pub knob: Hsla,
    pub text: Hsla,
    pub text_2: Hsla,
    pub text_3: Hsla,
    pub accent: Hsla,
    pub accent_text: Hsla,
    pub accent_soft: Hsla,
    pub danger: Hsla,
    pub meter: Hsla,
    pub level: Hsla,
    pub glyph_on_level: Hsla,
    pub menu: Hsla,
    pub shadow: Hsla,
    pub menu_shadow: Hsla,
    /// Window corner radius: Windows rounds undecorated windows itself, with
    /// a smaller radius.
    pub radius_window: f32,
}

pub const RADIUS_PLATTER: f32 = 16.0;
pub const GUTTER: f32 = 12.0;

impl Theme {
    pub fn new(appearance: WindowAppearance, glass: Glass) -> Self {
        let dark = matches!(
            appearance,
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        let radius_window = if cfg!(windows) { 8.0 } else { 18.0 };
        let mut t = if dark {
            Theme {
                tint: rgba(28, 28, 30, 0.58),
                platter: white(0.075),
                rim: white(0.16),
                edge: white(0.05),
                hairline: white(0.08),
                fill: white(0.1),
                fill_active: white(0.2),
                track: white(0.11),
                knob: white(1.0),
                text: white(0.93),
                text_2: white(0.6),
                text_3: white(0.42),
                accent: rgba(10, 132, 255, 1.0),
                accent_text: white(1.0),
                accent_soft: rgba(10, 132, 255, 0.28),
                danger: rgba(255, 105, 97, 1.0),
                meter: white(0.5),
                level: white(0.94),
                glyph_on_level: black(0.6),
                menu: rgba(40, 40, 43, 0.86),
                shadow: black(0.18),
                menu_shadow: black(0.45),
                radius_window,
            }
        } else {
            Theme {
                tint: rgba(246, 246, 248, 0.6),
                platter: white(0.55),
                rim: white(0.9),
                edge: black(0.07),
                hairline: black(0.08),
                fill: black(0.065),
                fill_active: black(0.15),
                track: black(0.09),
                knob: white(1.0),
                text: black(0.87),
                text_2: black(0.55),
                text_3: black(0.4),
                accent: rgba(0, 122, 255, 1.0),
                accent_text: white(1.0),
                accent_soft: rgba(0, 122, 255, 0.18),
                danger: rgba(224, 53, 43, 1.0),
                meter: black(0.38),
                level: white(1.0),
                glyph_on_level: black(0.6),
                menu: rgba(248, 248, 250, 0.9),
                shadow: black(0.04),
                menu_shadow: black(0.18),
                radius_window,
            }
        };
        t.tint = match (glass, dark) {
            // Real Liquid Glass already carries its own tint and contrast.
            (Glass::Liquid, false) => rgba(246, 246, 248, 0.22),
            (Glass::Liquid, true) => rgba(28, 28, 30, 0.22),
            (Glass::None, false) => rgba(236, 236, 238, 1.0),
            (Glass::None, true) => rgba(31, 31, 33, 1.0),
            (Glass::Native, _) => t.tint,
        };
        t
    }
}

static FONT: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Records the family name the system gave the bundled Inter.
pub fn set_font(name: String) {
    let _ = FONT.set(name);
}

/// The interface font: SF Pro on macOS, the bundled Inter elsewhere, since
/// SF Pro's license keeps it on Apple platforms.
pub fn font_family() -> gpui::SharedString {
    if cfg!(target_os = "macos") {
        ".SystemUIFont".into()
    } else {
        FONT.get()
            .map_or("Inter Variable", String::as_str)
            .to_owned()
            .into()
    }
}

/// Weights: Inter gets heavier ones than SF Pro, to make up for ClearType
/// drawing it thinner.
pub struct Weights {
    pub regular: f32,
    pub medium: f32,
    pub semibold: f32,
}

pub const WEIGHTS: Weights = if cfg!(target_os = "macos") {
    Weights {
        regular: 400.0,
        medium: 500.0,
        semibold: 600.0,
    }
} else {
    Weights {
        regular: 430.0,
        medium: 530.0,
        semibold: 620.0,
    }
};
