//! The panel: the source, the outputs that play, the other devices, and the
//! footer. Drawn as a Control Center module (`DESIGN.md`).
//!
//! The panel entity outlives its window: closing the panel removes the
//! window, and the next opening puts the same entity in a new one, so the
//! user's folding and edit mode stay as they left them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    actions, anchored, canvas, deferred, div, img, point, prelude::*, px, relative, AnimationExt,
    App, Bounds, ClickEvent, Context, Entity, FocusHandle, Focusable, Font, FontFeatures,
    FontStyle, FontWeight, Image, ImageFormat, KeyBinding, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, ScrollHandle, SharedString, Subscription,
    Task, Window,
};

use crate::audio::{self, DeviceList, OutputInfo, SourceKind, Status};
use crate::config::AppConfig;
use crate::i18n::Lang;
use crate::state::AppState;

use super::assets;
use super::native;
use super::text_field::{FieldEvent, TextField};
use super::theme::{font_family, Theme, GUTTER, WEIGHTS};
use super::view::{self, Tone};
use super::widgets::{self, button, glyph_svg, platter, round_button, ButtonKind};

actions!(panel, [HidePanel]);

const CONTEXT: &str = "Panel";
/// Logical size of the panel window.
pub const SIZE: (f64, f64) = (380.0, 560.0);
const STATUS_POLL: Duration = Duration::from_millis(100);
/// Width of the volume capsule's invisible thumb, as the range input had.
const THUMB: f32 = 28.0;
const MENU_ITEM: f32 = 26.0;
const MENU_HEADER: f32 = 24.0;
const MENU_PAD: f32 = 5.0;

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", HidePanel, Some(CONTEXT))]);
}

/// Settings changes, run one after the other off the UI thread: each one
/// writes the settings file, which a roaming profile can make slow.
type Job = Box<dyn FnOnce(&AppState) + Send>;

struct Worker(mpsc::Sender<Job>);

impl Worker {
    fn new(state: Arc<AppState>) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let spawned = std::thread::Builder::new()
            .name("settings".into())
            .spawn(move || {
                for job in rx {
                    job(&state);
                }
            });
        if let Err(e) = spawned {
            log::error!("settings thread: {e}");
        }
        Worker(tx)
    }

    fn run(&self, job: impl FnOnce(&AppState) + Send + 'static) {
        let _ = self.0.send(Box::new(job));
    }
}

#[derive(Default, Clone, Copy)]
struct Meter {
    level: f32,
    target: f32,
}

#[derive(Default)]
struct Geometry {
    faders: HashMap<String, Bounds<Pixels>>,
    picker: Option<Bounds<Pixels>>,
}

struct Rename {
    id: String,
    field: Entity<TextField>,
    suffix: String,
    _subscriptions: Vec<Subscription>,
}

/// One line of the source menu: a source, or a group header.
struct MenuEntry {
    id: Option<String>,
    label: String,
    checked: bool,
}

pub struct Panel {
    state: Arc<AppState>,
    lang: Lang,
    worker: Worker,
    devices: Option<DeviceList>,
    config: AppConfig,
    status: Option<Status>,
    seen_revision: Option<u64>,
    autostart: bool,
    /// Bumped by each switch flip, so the knob animates on a flip only.
    autostart_flips: usize,
    editing: bool,
    /// Whether the other devices are unfolded; `None` until the user says.
    others_open: Option<bool>,
    unfolds: usize,
    rename: Option<Rename>,
    rename_errors: HashMap<String, String>,
    /// The focused item of the open source menu.
    menu: Option<usize>,
    menu_opens: usize,
    drag: Option<(String, String)>,
    geometry: Rc<RefCell<Geometry>>,
    source_meter: Meter,
    meters: HashMap<String, Meter>,
    scroll: ScrollHandle,
    focus: FocusHandle,
    menu_focus: FocusHandle,
    opens: usize,
    icon: Arc<Image>,
    poll: Option<Task<()>>,
    window_subscriptions: Vec<Subscription>,
}

impl Focusable for Panel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel {
    pub fn new(state: Arc<AppState>, cx: &mut Context<Self>) -> Self {
        Panel {
            lang: state.lang,
            worker: Worker::new(state.clone()),
            config: state.config(),
            state,
            devices: None,
            status: None,
            seen_revision: None,
            autostart: false,
            autostart_flips: 0,
            editing: false,
            others_open: None,
            unfolds: 0,
            rename: None,
            rename_errors: HashMap::new(),
            menu: None,
            menu_opens: 0,
            drag: None,
            geometry: Rc::default(),
            source_meter: Meter::default(),
            meters: HashMap::new(),
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            menu_focus: cx.focus_handle(),
            opens: 0,
            icon: Arc::new(Image::from_bytes(
                ImageFormat::Png,
                assets::APP_ICON.to_vec(),
            )),
            poll: None,
            window_subscriptions: Vec::new(),
        }
    }

    /// The panel was put in a new window: settle in, read fresh devices and
    /// follow the engine.
    pub fn opened(
        &mut self,
        subscriptions: Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.window_subscriptions = subscriptions;
        self.opens += 1;
        window.focus(&self.focus);
        self.load(cx);
        self.poll = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(STATUS_POLL).await;
            if this.update(cx, |p, cx| p.poll(cx)).is_err() {
                break;
            }
        }));
    }

    /// The window went away: stop following the engine, drop what was held.
    pub fn closed(&mut self, cx: &mut Context<Self>) {
        self.poll = None;
        self.window_subscriptions.clear();
        self.menu = None;
        self.end_drag(cx);
        if self.rename.is_some() {
            self.end_rename(true, cx);
        }
    }

    pub fn update_ready(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    /// Reads the devices and the settings again, off the UI thread:
    /// enumerating is a full pass over the system's devices.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        let task = cx.background_spawn(async move {
            let devices = audio::enumerate();
            (devices, crate::autostart::is_enabled())
        });
        cx.spawn(async move |this, cx| {
            let (devices, autostart) = task.await;
            let _ = this.update(cx, |p, cx| {
                match devices {
                    Ok(d) => p.devices = Some(d),
                    Err(e) => log::warn!("devices: {e}"),
                }
                p.config = p.state.config();
                p.autostart = autostart;
                p.drop_stale();
                cx.notify();
            });
        })
        .detach();
    }

    /// A drag, a rename or a menu over a device that went away ends with it.
    fn drop_stale(&mut self) {
        let Some(devices) = &self.devices else {
            return;
        };
        let here = |id: &str| devices.outputs.iter().any(|o| o.id == id);
        if self.drag.as_ref().is_some_and(|(id, _)| !here(id)) {
            self.drag = None;
        }
        if self.rename.as_ref().is_some_and(|r| !here(&r.id)) {
            self.rename = None;
        }
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        let status = self.state.status();
        // The backends know when a device appears or goes away, so the list
        // is read again on their word rather than on a timer.
        if self
            .seen_revision
            .is_some_and(|r| r != status.devices_revision)
        {
            self.load(cx);
        }
        self.seen_revision = Some(status.devices_revision);
        let live = status.running;
        self.source_meter.target = if live {
            view::meter_target(status.source.peak)
        } else {
            0.0
        };
        for m in self.meters.values_mut() {
            m.target = 0.0;
        }
        if live {
            for o in &status.outputs {
                self.meters.entry(o.id.clone()).or_default().target = view::meter_target(o.peak);
            }
        }
        self.status = Some(status);
        cx.notify();
    }

    fn rows(&self) -> Vec<OutputInfo> {
        self.devices
            .as_ref()
            .map(|d| view::output_rows(d, &self.config))
            .unwrap_or_default()
    }

    fn output_config(&self, id: &str) -> (f32, bool) {
        self.config
            .outputs
            .iter()
            .find(|o| o.id == id)
            .map_or((1.0, false), |o| (o.fader, o.muted))
    }

    /* Actions */

    fn set_enabled(&mut self, id: String, name: String, enabled: bool, cx: &mut Context<Self>) {
        self.config.output_mut(&id, &name).enabled = enabled;
        self.worker
            .run(move |s| s.set_output_enabled(&id, &name, enabled));
        if !self.rows().iter().any(|r| {
            self.config
                .outputs
                .iter()
                .any(|o| o.enabled && o.id == r.id)
        }) {
            self.editing = false;
        }
        cx.notify();
    }

    fn toggle_mute(&mut self, id: String, name: String, cx: &mut Context<Self>) {
        let muted = !self.output_config(&id).1;
        self.config.output_mut(&id, &name).muted = muted;
        self.worker
            .run(move |s| s.set_output_muted(&id, &name, muted));
        cx.notify();
    }

    fn set_fader(&mut self, x: Pixels, persist: bool, cx: &mut Context<Self>) {
        let Some((id, name)) = self.drag.clone() else {
            return;
        };
        let Some(bounds) = self.geometry.borrow().faders.get(&id).copied() else {
            return;
        };
        let span = (f32::from(bounds.size.width) - THUMB).max(1.0);
        let fader = ((f32::from(x - bounds.origin.x) - THUMB / 2.0) / span).clamp(0.0, 1.0);
        self.config.output_mut(&id, &name).fader = fader;
        let state = self.state.clone();
        if persist {
            self.worker
                .run(move |s| s.set_output_volume(&id, &name, fader, true));
        } else {
            // Straight to the audio thread, in memory only.
            state.set_output_volume(&id, &name, fader, false);
        }
        cx.notify();
    }

    fn start_drag(
        &mut self,
        id: String,
        name: String,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        if self.rename.is_some() {
            self.end_rename(true, cx);
        }
        self.drag = Some((id, name));
        self.set_fader(event.position.x, false, cx);
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        let Some((id, name)) = self.drag.take() else {
            return;
        };
        // Released: save the final position.
        let fader = self.output_config(&id).0;
        self.worker
            .run(move |s| s.set_output_volume(&id, &name, fader, true));
        cx.notify();
    }

    fn choose_source(&mut self, id: String, cx: &mut Context<Self>) {
        self.menu = None;
        if id == self.config.source {
            return;
        }
        self.config.source = id.clone();
        self.worker.run(move |s| s.set_source(id));
        cx.notify();
    }

    fn toggle_autostart(&mut self, cx: &mut Context<Self>) {
        let wanted = !self.autostart;
        self.autostart = wanted;
        self.autostart_flips += 1;
        let task = cx.background_spawn(async move { crate::autostart::set(wanted) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |p, cx| {
                match result {
                    Ok(now) => p.autostart = now,
                    Err(e) => {
                        log::warn!("autostart: {e}");
                        p.autostart = !wanted;
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /* Rename: the name changes in the system, so every application shows it. */

    fn start_rename(&mut self, dev: &OutputInfo, window: &mut Window, cx: &mut Context<Self>) {
        let Some(description) = dev.description.clone() else {
            return;
        };
        self.rename_errors.remove(&dev.id);
        let t = Theme::new(window.appearance(), native::glass());
        let field = cx.new(|cx| TextField::new(&description, t.accent, t.accent_soft, cx));
        let subscriptions = vec![
            cx.subscribe(&field, |this, _, event: &FieldEvent, cx| {
                this.end_rename(matches!(event, FieldEvent::Submit), cx);
            }),
            cx.on_blur(&field.focus_handle(cx), window, |this, _, cx| {
                this.end_rename(true, cx);
            }),
        ];
        // What the system keeps, such as the adapter in "Speakers (Realtek Audio)".
        let suffix = dev
            .name
            .strip_prefix(description.as_str())
            .map(|s| s.trim().to_owned())
            .unwrap_or_default();
        window.focus(&field.focus_handle(cx));
        self.rename = Some(Rename {
            id: dev.id.clone(),
            field,
            suffix,
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    fn end_rename(&mut self, save: bool, cx: &mut Context<Self>) {
        let Some(rename) = self.rename.take() else {
            return;
        };
        let text = rename.field.read(cx).text().trim().to_owned();
        let current = self
            .devices
            .as_ref()
            .and_then(|d| d.outputs.iter().find(|o| o.id == rename.id))
            .and_then(|o| o.description.clone());
        cx.notify();
        if !save || text.is_empty() || Some(&text) == current.as_ref() {
            return;
        }
        let id = rename.id.clone();
        let task = cx.background_spawn({
            let id = id.clone();
            async move { audio::rename_output(&id, &text) }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |p, cx| {
                match result {
                    Ok(()) => {
                        p.rename_errors.remove(&id);
                        // The system names it differently now: read the list again.
                        p.load(cx);
                    }
                    Err(e) => {
                        log::warn!("rename: {e}");
                        p.rename_errors
                            .insert(id, p.lang.t("output.renameFailed").to_owned());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /* Source menu */

    fn menu_entries(&self) -> Vec<MenuEntry> {
        let lang = self.lang;
        let mut entries = Vec::new();
        let Some(devices) = &self.devices else {
            return entries;
        };
        let current = &self.config.source;
        if !devices.sources.iter().any(|s| &s.id == current) {
            entries.push(MenuEntry {
                id: Some(current.clone()),
                label: lang.t("source.disconnected").to_owned(),
                checked: true,
            });
        }
        let groups = [
            (SourceKind::Desktop, None),
            (SourceKind::Loopback, Some("source.group.output")),
            (SourceKind::Capture, Some("source.group.input")),
        ];
        for (kind, header) in groups {
            let items: Vec<_> = devices.sources.iter().filter(|s| s.kind == kind).collect();
            if items.is_empty() {
                continue;
            }
            if let Some(h) = header {
                entries.push(MenuEntry {
                    id: None,
                    label: lang.t(h).to_owned(),
                    checked: false,
                });
            }
            for s in items {
                entries.push(MenuEntry {
                    id: Some(s.id.clone()),
                    label: view::source_name(lang, s),
                    checked: &s.id == current,
                });
            }
        }
        entries
    }

    fn picker_label(&self) -> String {
        self.menu_entries()
            .into_iter()
            .find(|e| e.checked)
            .map(|e| e.label)
            .unwrap_or_default()
    }

    fn open_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entries = self.menu_entries();
        let at = entries
            .iter()
            .position(|e| e.checked)
            .or_else(|| entries.iter().position(|e| e.id.is_some()));
        let Some(at) = at else {
            return;
        };
        self.menu = Some(at);
        self.menu_opens += 1;
        window.focus(&self.menu_focus);
        cx.notify();
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            window.focus(&self.focus);
            cx.notify();
        }
    }

    fn menu_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self.menu else {
            return;
        };
        let entries = self.menu_entries();
        let items: Vec<usize> = (0..entries.len())
            .filter(|&i| entries[i].id.is_some())
            .collect();
        let pos = items.iter().position(|&i| i == at).unwrap_or(0);
        let next = match event.keystroke.key.as_str() {
            "down" => items.get(pos + 1).copied(),
            "up" => pos.checked_sub(1).and_then(|p| items.get(p).copied()),
            "home" => items.first().copied(),
            "end" => items.last().copied(),
            "enter" | "space" => {
                if let Some(id) = entries[at].id.clone() {
                    self.choose_source(id, cx);
                    window.focus(&self.focus);
                }
                cx.stop_propagation();
                return;
            }
            "escape" | "tab" => {
                // Closes the menu instead of hiding the panel.
                self.close_menu(window, cx);
                cx.stop_propagation();
                return;
            }
            _ => return,
        };
        if let Some(n) = next {
            self.menu = Some(n);
            cx.notify();
        }
        cx.stop_propagation();
    }
}

/// Text styling shared by the whole panel: the system font, 13 px, tabular
/// numerals so dB values hold still.
fn base_text(d: gpui::Div, t: &Theme) -> gpui::Div {
    d.font(Font {
        family: font_family(),
        features: FontFeatures(Arc::new(vec![("tnum".into(), 1)])),
        fallbacks: None,
        weight: FontWeight(WEIGHTS.regular),
        style: FontStyle::Normal,
    })
    .text_size(px(13.))
    .line_height(relative(1.35))
    .text_color(t.text)
}

fn tone_color(t: &Theme, tone: Tone) -> gpui::Hsla {
    match tone {
        Tone::On => t.text,
        Tone::Muted => t.text_2,
        Tone::Error => t.danger,
    }
}

/// Fades a block in and lifts it into place as the panel opens, on
/// `.snappy`, `delay` after the others.
fn settle(el: gpui::Div, id: (&'static str, usize), delay: f32) -> impl IntoElement {
    let (anim, progress) = widgets::animation(widgets::snappy(), delay);
    el.relative().with_animation(id, anim, move |el, d| {
        let p = progress(d);
        el.top(px(6.0 * (1.0 - p)))
            .opacity(0.35 + 0.65 * (p / 0.3).clamp(0.0, 1.0))
    })
}

impl Render for Panel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::new(window.appearance(), native::glass());
        let lang = self.lang;

        // Meters: instant peak, smooth fall, one step per frame.
        let mut moving = false;
        for m in std::iter::once(&mut self.source_meter).chain(self.meters.values_mut()) {
            m.level = view::meter_fall(m.level, m.target);
            moving |= m.level > 0.0;
        }
        if moving {
            window.request_animation_frame();
        }

        let rows = self.rows();
        let (active, others) = self
            .devices
            .as_ref()
            .map(|_| view::split_outputs(&rows, &self.config))
            .unwrap_or_default();
        let status = self.status.clone();
        let live = status.as_ref().filter(|s| s.running);
        let (run_text, run_tone) = view::run_state(lang, live, &rows);
        let scrolled = self.scroll.offset().y < px(0.);

        let root = base_text(div(), &t)
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &HidePanel, _, cx| {
                cx.defer(super::hide_panel);
            }))
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(t.tint)
            .rounded(px(t.radius_window))
            .border_1()
            .border_color(t.rim.opacity(0.5))
            .overflow_hidden()
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                if this.drag.is_some() {
                    this.set_fader(e.position.x, false, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| this.end_drag(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| this.end_drag(cx)),
            );

        // Header: title, run state, update.
        let update = self.state.update_ready.lock().clone();
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .pt(px(16.))
            .px(px(18.))
            .pb(px(10.))
            .border_b_1()
            .border_color(if scrolled {
                t.hairline
            } else {
                gpui::transparent_black()
            })
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(7.))
                    .child(img(self.icon.clone()).size(px(18.)).flex_none())
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight(WEIGHTS.semibold))
                            .whitespace_nowrap()
                            .child("Audio Mirror"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_w_0()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(tone_color(&t, run_tone))
                            .child(run_text),
                    )
                    .when_some(update, |d, _version| {
                        d.child(
                            button(
                                &t,
                                "update",
                                lang.t("update.restart"),
                                ButtonKind::Primary,
                                true,
                            )
                            .on_click(|_, _, cx| {
                                crate::updater::relaunch();
                                cx.quit();
                            }),
                        )
                    }),
            );

        // Source.
        let picker_geo = self.geometry.clone();
        let menu_open = self.menu.is_some();
        let source_error = live
            .filter(|s| s.source.state == audio::NodeState::Error)
            .map(|s| view::retrying(lang, s.source.message.as_ref()));
        let source_level = self.source_meter.level;
        let source = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(group_title(&t, lang.t("source.title")))
            .child(
                platter(&t)
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .pt(px(8.))
                    .px(px(8.))
                    .pb(px(12.))
                    .child(
                        div()
                            .id("picker")
                            .relative()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .w_full()
                            .min_h(px(32.))
                            .pl(px(12.))
                            .pr(px(10.))
                            .py(px(6.))
                            .rounded(px(10.))
                            .bg(if menu_open { t.fill_active } else { t.fill })
                            .font_weight(FontWeight(WEIGHTS.medium))
                            .cursor_default()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(self.picker_label()),
                            )
                            .child(glyph_svg(assets::CHEVRONS, 11., t.text_2))
                            .child(
                                canvas(
                                    move |bounds, _, _| {
                                        picker_geo.borrow_mut().picker = Some(bounds)
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            )
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                if this.menu.is_some() {
                                    this.close_menu(window, cx);
                                } else {
                                    this.open_menu(window, cx);
                                }
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .px(px(6.))
                            .child(widgets::meter(&t, source_level, 4., false))
                            .child(db_label(
                                &t,
                                view::format_db(lang, view::meter_db(source_level)),
                                false,
                            )),
                    )
                    .when_some(source_error, |d, e| {
                        d.child(div().px(px(6.)).text_color(t.danger).child(e))
                    }),
            );

        // Outputs.
        let mut list = div().flex().flex_col().gap(px(8.));
        if self.devices.is_none() {
            list = list.child(empty_row(&t, lang.t("outputs.searching")));
        } else if active.is_empty() {
            let text = if rows.is_empty() {
                lang.t("outputs.none")
            } else {
                lang.t("outputs.hint")
            };
            list = list.child(empty_row(&t, text));
        }
        let statuses: HashMap<&str, &audio::OutputStatus> = live
            .map(|s| s.outputs.iter().map(|o| (o.id.as_str(), o)).collect())
            .unwrap_or_default();
        for (ix, dev) in active.iter().enumerate() {
            let row = self.output_row(
                &t,
                ix,
                dev,
                statuses.get(dev.id.as_str()).copied(),
                window,
                cx,
            );
            list = list.child(row);
        }
        let outputs = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .min_h(px(24.))
                    .child(group_title(&t, lang.t("outputs.title")))
                    .when(!active.is_empty(), |d| {
                        let label = if self.editing {
                            lang.t("outputs.done")
                        } else {
                            lang.t("outputs.edit")
                        };
                        d.child(button(&t, "edit", label, ButtonKind::Link, true).on_click(
                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.editing = !this.editing;
                                if this.rename.is_some() {
                                    this.end_rename(true, cx);
                                }
                                cx.notify();
                            }),
                        ))
                    }),
            )
            .child(list)
            .when(!others.is_empty(), |d| {
                d.child(self.others(&t, &others, active.is_empty(), cx))
            });

        let main = div()
            .id("scroller")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px(px(GUTTER))
            .pt(px(2.))
            .pb(px(12.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .child(source)
                    .child(outputs),
            );

        // Footer.
        let flips = self.autostart_flips;
        let footer = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .pt(px(10.))
            .px(px(16.))
            .pb(px(12.))
            .border_t_1()
            .border_color(t.hairline)
            .child(
                div()
                    .id("autostart")
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .whitespace_nowrap()
                    .cursor_default()
                    .child(widgets::switch(
                        &t,
                        ("knob", flips),
                        self.autostart,
                        flips > 0,
                    ))
                    .child(lang.t("autostart"))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_autostart(cx))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_color(t.text_3)
                            .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
                    )
                    .child(
                        button(&t, "quit", lang.t("quit"), ButtonKind::Plain, false)
                            .on_click(|_, _, cx| cx.quit()),
                    ),
            );

        let opens = self.opens;
        root.child(settle(header, ("settle-head", opens), 0.0))
            .child(main)
            .child(settle(footer, ("settle-foot", opens), 0.04))
            .when(self.menu.is_some(), |d| {
                d.child(self.menu_overlay(&t, window, cx))
            })
    }
}

fn group_title(t: &Theme, text: &'static str) -> impl IntoElement {
    div()
        .pl(px(6.))
        .text_size(px(12.))
        .font_weight(FontWeight(WEIGHTS.semibold))
        .text_color(t.text_2)
        .child(text)
}

fn empty_row(t: &Theme, text: &'static str) -> impl IntoElement {
    platter(t)
        .flex()
        .items_center()
        .min_h(px(44.))
        .px(px(14.))
        .py(px(10.))
        .text_color(t.text_2)
        .child(text)
}

fn db_label(t: &Theme, text: String, dim: bool) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .justify_end()
        .w(px(62.))
        .whitespace_nowrap()
        .text_color(if dim { t.text_3 } else { t.text_2 })
        .child(text)
}

impl Panel {
    fn output_row(
        &mut self,
        t: &Theme,
        ix: usize,
        dev: &OutputInfo,
        status: Option<&audio::OutputStatus>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let lang = self.lang;
        let (fader, muted) = self.output_config(&dev.id);
        let state = view::output_state(lang, status, true, muted);
        let detail = [
            self.rename_errors.get(&dev.id).cloned(),
            Some(state.detail.clone()),
        ]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
        let level = self.meters.get(&dev.id).map_or(0.0, |m| m.level);
        let id = dev.id.clone();
        let name = dev.name.clone();

        // Head: name (or the rename field), state.
        let renaming = self.rename.as_ref().filter(|r| r.id == dev.id);
        let head = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .min_w_0()
            .min_h(px(20.))
            .map(|d| match renaming {
                Some(r) => d
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .mx(px(-7.))
                            .my(px(-4.))
                            .px(px(7.))
                            .py(px(3.))
                            .rounded(px(7.))
                            .bg(t.fill)
                            .border_1()
                            .border_color(t.accent)
                            .font_weight(FontWeight(WEIGHTS.semibold))
                            .child(r.field.clone()),
                    )
                    .when(!r.suffix.is_empty(), |d| {
                        d.child(
                            div()
                                .flex_none()
                                .max_w(relative(0.45))
                                .ml(px(6.))
                                .truncate()
                                .text_color(t.text_3)
                                .child(r.suffix.clone()),
                        )
                    }),
                None => d.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight(WEIGHTS.semibold))
                        .child(view::device_name(lang, dev)),
                ),
            })
            .child(
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(if state.tone == Tone::Error {
                        t.danger
                    } else {
                        t.text_2
                    })
                    .child(state.label.clone()),
            );

        let body = if self.editing {
            let dev_for_rename = dev.clone();
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .when(dev.description.is_some(), |d| {
                    d.child(
                        button(
                            t,
                            ("rename", ix),
                            lang.t("output.rename"),
                            ButtonKind::Normal,
                            true,
                        )
                        .on_click(cx.listener(
                            move |this, _: &ClickEvent, window, cx| {
                                this.start_rename(&dev_for_rename, window, cx);
                            },
                        )),
                    )
                })
                .child({
                    let (id, name) = (id.clone(), name.clone());
                    button(
                        t,
                        ("remove", ix),
                        lang.t("output.remove"),
                        ButtonKind::Danger,
                        true,
                    )
                    .on_click(cx.listener(
                        move |this, _: &ClickEvent, _, cx| {
                            this.set_enabled(id.clone(), name.clone(), false, cx);
                        },
                    ))
                })
        } else {
            let geo = self.geometry.clone();
            let key = id.clone();
            let (drag_id, drag_name) = (id.clone(), name.clone());
            let capsule = div()
                .id(("fader", ix))
                .relative()
                .h(px(28.))
                .rounded_full()
                .bg(t.track)
                .overflow_hidden()
                .cursor_default()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left_0()
                        .min_w(px(THUMB))
                        .w(relative(fader))
                        .rounded_full()
                        .bg(if muted { t.fill_active } else { t.level }),
                )
                .child(div().absolute().left(px(9.)).top(px(7.)).child(glyph_svg(
                    assets::SPEAKER,
                    14.,
                    if muted { t.text_3 } else { t.glyph_on_level },
                )))
                .child(
                    canvas(
                        move |bounds, _, _| {
                            geo.borrow_mut().faders.insert(key, bounds);
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        this.start_drag(drag_id.clone(), drag_name.clone(), e, cx);
                    }),
                );
            let (mute_id, mute_name) = (id.clone(), name.clone());
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(5.))
                        .child(capsule)
                        .child(
                            div()
                                .flex()
                                .mx(px(14.))
                                .child(widgets::meter(t, level, 3., muted)),
                        ),
                )
                .child(db_label(t, view::fader_db(lang, fader), muted))
                .child(
                    round_button(
                        t,
                        ("mute", ix),
                        if muted {
                            assets::SPEAKER_OFF
                        } else {
                            assets::SPEAKER
                        },
                        muted,
                    )
                    .on_click(cx.listener(
                        move |this, _: &ClickEvent, _, cx| {
                            this.toggle_mute(mute_id.clone(), mute_name.clone(), cx);
                        },
                    )),
                )
        };

        platter(t)
            .flex()
            .flex_col()
            .gap(px(10.))
            .pt(px(12.))
            .pr(px(12.))
            .pb(px(12.))
            .pl(px(14.))
            .child(head)
            .child(body)
            .when(!detail.is_empty(), |d| {
                d.child(div().text_size(px(12.)).text_color(t.text_2).child(detail))
            })
    }

    /// The devices that are not playing, folded under one row.
    fn others(
        &self,
        t: &Theme,
        others: &[OutputInfo],
        nothing_plays: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let lang = self.lang;
        let open = self.others_open.unwrap_or(nothing_plays);
        let fill = t.fill;
        let toggle = div()
            .id("others-toggle")
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .min_h(px(44.))
            .px(px(14.))
            .py(px(10.))
            .cursor_default()
            .active(move |s| s.bg(fill))
            .child(
                div()
                    .font_weight(FontWeight(WEIGHTS.medium))
                    .child(lang.plural("outputs.others", others.len(), &[])),
            )
            .child(
                div()
                    .font_weight(FontWeight(WEIGHTS.medium))
                    .text_color(t.accent)
                    .child(if open {
                        lang.t("outputs.hide")
                    } else {
                        lang.t("outputs.show")
                    }),
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.others_open = Some(!open);
                if !open {
                    this.unfolds += 1;
                }
                cx.notify();
            }));
        let unfolds = self.unfolds;
        platter(t)
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(toggle)
            .when(open, |d| {
                d.children(others.iter().enumerate().map(|(ix, dev)| {
                    let (id, name) = (dev.id.clone(), dev.name.clone());
                    let row = div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .min_h(px(44.))
                        .ml(px(14.))
                        .pr(px(14.))
                        .py(px(10.))
                        .border_t_1()
                        .border_color(t.hairline)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(view::device_name(lang, dev)),
                        )
                        .child(
                            button(
                                t,
                                ("add", ix),
                                lang.t("output.add"),
                                ButtonKind::Accent,
                                true,
                            )
                            .on_click(cx.listener(
                                move |this, _: &ClickEvent, _, cx| {
                                    // The list stays unfolded for the next one.
                                    this.others_open.get_or_insert(true);
                                    this.set_enabled(id.clone(), name.clone(), true, cx);
                                },
                            )),
                        );
                    if unfolds == 0 {
                        return row.into_any_element();
                    }
                    // The rows come in when the user unfolds them, in a
                    // short stagger.
                    let delay = (ix.min(2) as f32) * 0.025;
                    let (anim, progress) = widgets::animation(widgets::smooth(), delay);
                    row.relative()
                        .with_animation(("unfold", unfolds * 1000 + ix), anim, move |el, d| {
                            let p = progress(d);
                            el.top(px(-6.0 * (1.0 - p))).opacity(p.clamp(0.0, 1.0))
                        })
                        .into_any_element()
                }))
            })
    }

    /// The source menu, drawn as macOS pops one up: over the button, the
    /// current item laid on it, kept inside the window.
    fn menu_overlay(
        &self,
        t: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let entries = self.menu_entries();
        let focused = self.menu.unwrap_or(0);
        let viewport = window.viewport_size();
        let (vw, vh) = (f32::from(viewport.width), f32::from(viewport.height));
        let margin = 6.0;
        let picker = self.geometry.borrow().picker.unwrap_or_default();
        let (bx, by, bw, bh) = (
            f32::from(picker.origin.x),
            f32::from(picker.origin.y),
            f32::from(picker.size.width),
            f32::from(picker.size.height),
        );
        let label_shift = 21.0;
        let width = (bw + label_shift).min(vw - 2.0 * margin);
        let left = (bx - label_shift).min(vw - width - margin).max(margin);
        let heights: Vec<f32> = entries
            .iter()
            .map(|e| {
                if e.id.is_some() {
                    MENU_ITEM
                } else {
                    MENU_HEADER
                }
            })
            .collect();
        let content: f32 = heights.iter().sum::<f32>() + 2.0 * MENU_PAD;
        let height = content.min(vh - 2.0 * margin);
        let current = entries.iter().position(|e| e.checked).unwrap_or(focused);
        let item_top = MENU_PAD + heights[..current.min(heights.len())].iter().sum::<f32>();
        let wanted = by + (bh - MENU_ITEM) / 2.0 - item_top;
        let top = wanted.min(vh - height - margin).max(margin);

        let (anim, progress) = widgets::animation(widgets::snappy(), 0.0);
        let accent = t.accent;
        let accent_text = t.accent_text;
        let menu = div()
            .id("source-menu")
            .key_context("SourceMenu")
            .track_focus(&self.menu_focus)
            .on_key_down(
                cx.listener(|this, e: &KeyDownEvent, window, cx| this.menu_key(e, window, cx)),
            )
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.close_menu(window, cx);
            }))
            .occlude()
            .w(px(width))
            .max_h(px(height))
            .overflow_y_scroll()
            .p(px(MENU_PAD))
            .bg(t.menu)
            .rounded(px(14.))
            .border_1()
            .border_color(t.edge)
            .shadow(vec![gpui::BoxShadow {
                color: t.menu_shadow,
                offset: point(px(0.), px(10.)),
                blur_radius: px(30.),
                spread_radius: px(0.),
            }])
            .children(entries.into_iter().enumerate().map(|(ix, e)| {
                match e.id {
                    None => div()
                        .h(px(MENU_HEADER))
                        .pt(px(6.))
                        .pl(px(30.))
                        .text_size(px(12.))
                        .font_weight(FontWeight(WEIGHTS.semibold))
                        .text_color(t.text_2)
                        .child(e.label)
                        .into_any_element(),
                    Some(id) => {
                        let focused = ix == focused;
                        let fg = if focused { accent_text } else { t.text };
                        div()
                            .id(("menu-item", ix))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .h(px(MENU_ITEM))
                            .pl(px(8.))
                            .pr(px(10.))
                            .rounded(px(8.))
                            .when(focused, |d| d.bg(accent))
                            .text_color(fg)
                            .cursor_default()
                            .child(
                                div()
                                    .flex()
                                    .flex_none()
                                    .justify_center()
                                    .w(px(14.))
                                    .when(e.checked, |d| {
                                        d.child(glyph_svg(assets::CHECK, 11., fg))
                                    }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(SharedString::from(e.label)),
                            )
                            .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                                if this.menu != Some(ix) {
                                    this.menu = Some(ix);
                                    cx.notify();
                                }
                            }))
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.choose_source(id.clone(), cx);
                                window.focus(&this.focus);
                            }))
                            .into_any_element()
                    }
                }
            }))
            .with_animation(("menu-in", self.menu_opens), anim, move |el, d| {
                el.opacity((progress(d) / 0.35).clamp(0.0, 1.0))
            });
        deferred(anchored().position(point(px(left), px(top))).child(menu)).with_priority(1)
    }
}
