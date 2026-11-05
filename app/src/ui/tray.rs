//! Tray icon and its menu.
//!
//! Left click toggles the panel, right click opens the menu. Linux trays do
//! not always report clicks, so the menu also offers "Open". Every click and
//! menu choice goes to the UI as an [`AppEvent`].

use futures::channel::mpsc::UnboundedSender;

use crate::i18n::Lang;

use super::AppEvent;

/// The icon, decoded to RGBA.
fn rgba(png: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .ok()?
        .into_rgba8();
    let (w, h) = img.dimensions();
    Some((img.into_raw(), w, h))
}

#[cfg(any(windows, target_os = "macos"))]
mod imp {
    use futures::channel::mpsc::UnboundedSender;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    };

    use crate::i18n::Lang;
    use crate::ui::native::Anchor;
    use crate::ui::AppEvent;

    pub struct Tray {
        icon: TrayIcon,
    }

    impl Tray {
        /// The middle of the icon, in physical pixels, where Windows tells
        /// it. macOS places the panel from the pointer instead (`native.rs`).
        pub fn anchor(&self) -> Option<Anchor> {
            if !cfg!(windows) {
                return None;
            }
            let rect = self.icon.rect().filter(|r| r.size.width > 0)?;
            Some(Anchor {
                x: rect.position.x + f64::from(rect.size.width) / 2.0,
                y: rect.position.y + f64::from(rect.size.height) / 2.0,
            })
        }
    }

    pub fn setup(lang: Lang, events: UnboundedSender<AppEvent>) -> Result<Tray, String> {
        let menu = Menu::new();
        let open = MenuItem::with_id("open", lang.t("tray.open"), true, None);
        let restart = MenuItem::with_id("restart", lang.t("tray.restart"), true, None);
        let quit = MenuItem::with_id("quit", lang.t("tray.quit"), true, None);
        menu.append_items(&[
            &open,
            &PredefinedMenuItem::separator(),
            &restart,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .map_err(|e| e.to_string())?;

        let png: &[u8] = if cfg!(target_os = "macos") {
            include_bytes!("../../icons/tray-template.png")
        } else {
            include_bytes!("../../icons/32x32.png")
        };
        let (data, w, h) = super::rgba(png).ok_or("tray icon")?;
        let icon = Icon::from_rgba(data, w, h).map_err(|e| e.to_string())?;
        let tray = TrayIconBuilder::new()
            .with_tooltip("Audio Mirror")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_icon(icon)
            .with_icon_as_template(cfg!(target_os = "macos"))
            .build()
            .map_err(|e| e.to_string())?;

        let tx = events.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                // Windows gives the icon's rectangle in physical pixels, the
                // unit the panel is placed in. macOS places the panel from
                // the pointer, which sits on the status item.
                let anchor = (cfg!(windows) && rect.size.width > 0).then(|| Anchor {
                    x: rect.position.x + f64::from(rect.size.width) / 2.0,
                    y: rect.position.y + f64::from(rect.size.height) / 2.0,
                });
                let _ = tx.unbounded_send(AppEvent::TrayClick(anchor));
            }
        }));
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let event = match event.id.0.as_str() {
                "open" => AppEvent::Open,
                "restart" => AppEvent::RestartAudio,
                "quit" => AppEvent::Quit,
                _ => return,
            };
            let _ = events.unbounded_send(event);
        }));
        Ok(Tray { icon: tray })
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use futures::channel::mpsc::UnboundedSender;
    use ksni::blocking::TrayMethods;
    use ksni::menu::StandardItem;
    use ksni::MenuItem;

    use crate::i18n::Lang;
    use crate::ui::AppEvent;

    pub struct Tray {
        _handle: ksni::blocking::Handle<Item>,
    }

    impl Tray {
        /// StatusNotifierItem does not say where the icon is.
        pub fn anchor(&self) -> Option<crate::ui::native::Anchor> {
            None
        }
    }

    pub struct Item {
        lang: Lang,
        events: UnboundedSender<AppEvent>,
        icon: Vec<ksni::Icon>,
    }

    impl ksni::Tray for Item {
        fn id(&self) -> String {
            crate::paths::IDENTIFIER.into()
        }

        fn title(&self) -> String {
            "Audio Mirror".into()
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            self.icon.clone()
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            let _ = self.events.unbounded_send(AppEvent::TrayClick(None));
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            let item = |label: &str, event: fn() -> AppEvent| -> MenuItem<Self> {
                StandardItem {
                    label: label.into(),
                    activate: Box::new(move |this: &mut Self| {
                        let _ = this.events.unbounded_send(event());
                    }),
                    ..Default::default()
                }
                .into()
            };
            vec![
                item(self.lang.t("tray.open"), || AppEvent::Open),
                MenuItem::Separator,
                item(self.lang.t("tray.restart"), || AppEvent::RestartAudio),
                MenuItem::Separator,
                item(self.lang.t("tray.quit"), || AppEvent::Quit),
            ]
        }
    }

    pub fn setup(lang: Lang, events: UnboundedSender<AppEvent>) -> Result<Tray, String> {
        let (rgba, w, h) =
            super::rgba(include_bytes!("../../icons/32x32.png")).ok_or("tray icon")?;
        // StatusNotifierItem wants ARGB32 in network byte order.
        let mut argb = rgba;
        for px in argb.as_chunks_mut::<4>().0 {
            px.rotate_right(1);
        }
        let item = Item {
            lang,
            events,
            icon: vec![ksni::Icon {
                width: w as i32,
                height: h as i32,
                data: argb,
            }],
        };
        let handle = item.spawn().map_err(|e| e.to_string())?;
        Ok(Tray { _handle: handle })
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod imp {
    pub struct Tray;

    impl Tray {
        pub fn anchor(&self) -> Option<crate::ui::native::Anchor> {
            None
        }
    }

    pub fn setup(
        _lang: crate::i18n::Lang,
        _events: futures::channel::mpsc::UnboundedSender<crate::ui::AppEvent>,
    ) -> Result<Tray, String> {
        Err("no tray on this system".into())
    }
}

pub use imp::Tray;

pub fn setup(lang: Lang, events: UnboundedSender<AppEvent>) -> Option<Tray> {
    match imp::setup(lang, events) {
        Ok(t) => Some(t),
        Err(e) => {
            log::error!("tray: {e}");
            None
        }
    }
}
