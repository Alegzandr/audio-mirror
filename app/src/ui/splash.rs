//! The startup window that checks for an update before anything else starts
//! (`updater.rs`).

use std::sync::Arc;
use std::time::Duration;

use gpui::{div, img, prelude::*, px, Context, FontWeight, Image, ImageFormat, Window};

use crate::i18n::Lang;
use crate::updater::{Stage, Updater};

use super::assets;
use super::theme::{font_family, Theme, WEIGHTS};

pub const SIZE: (f32, f32) = (300.0, 110.0);

pub struct Splash {
    updater: Arc<Updater>,
    lang: Lang,
    icon: Arc<Image>,
    _poll: gpui::Task<()>,
}

impl Splash {
    pub fn new(updater: Arc<Updater>, lang: Lang, cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
            if this.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        });
        Splash {
            updater,
            lang,
            icon: Arc::new(Image::from_bytes(
                ImageFormat::Png,
                assets::APP_ICON.to_vec(),
            )),
            _poll: poll,
        }
    }
}

impl Render for Splash {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // The splash is opaque on every system: it shows for a moment, alone.
        let t = Theme::new(window.appearance(), super::theme::Glass::None);
        let lang = self.lang;
        let progress = self.updater.progress();
        let percent = progress
            .percent
            .filter(|_| progress.stage == Stage::Downloading);
        let text = match (progress.stage, percent) {
            (Stage::Downloading, Some(p)) => {
                lang.tf("splash.downloadingPercent", &[("percent", &p.to_string())])
            }
            (Stage::Checking, _) => lang.t("splash.checking").to_owned(),
            (Stage::Downloading, None) => lang.t("splash.downloading").to_owned(),
            (Stage::Installing, _) => lang.t("splash.installing").to_owned(),
            (Stage::Restarting, _) => lang.t("splash.restarting").to_owned(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .justify_center()
            .gap(px(6.))
            .px(px(22.))
            .bg(t.tint)
            .font_family(font_family())
            .text_size(px(13.))
            .text_color(t.text)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(img(self.icon.clone()).size(px(20.)))
                    .child(
                        div()
                            .text_size(px(17.))
                            .font_weight(FontWeight(WEIGHTS.semibold))
                            .child("Audio Mirror"),
                    ),
            )
            .child(div().text_color(t.text_2).child(text))
            .child(
                div()
                    .mt(px(8.))
                    .h(px(4.))
                    .rounded_full()
                    .bg(if percent.is_some() {
                        t.track
                    } else {
                        gpui::transparent_black()
                    })
                    .overflow_hidden()
                    .child(
                        div()
                            .h_full()
                            .w(gpui::relative(f32::from(percent.unwrap_or(0)) / 100.0))
                            .bg(t.accent),
                    ),
            )
    }
}
