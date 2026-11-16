//! The panel's state and every change it makes: the source, the outputs
//! that play, the other devices and the footer. `panel.slint` draws it;
//! this module feeds it and answers its callbacks.
//!
//! The panel lives as long as the app: hiding the window keeps it, so the
//! user's folding and edit mode stay as they left them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};

use crate::audio::{self, DeviceList, OutputInfo, SourceKind, Status};
use crate::config::AppConfig;
use crate::i18n::Lang;
use crate::state::AppState;

use super::view::{self, Tone};
use super::{MenuEntry, OtherRow, OutputRow, Panel, Texts};

/// Logical size of the panel window, as `panel.slint` draws it.
pub const SIZE: (f64, f64) = (380.0, 560.0);
/// How often the meters move while the panel is open: about one frame.
const FRAME: Duration = Duration::from_millis(16);
/// The engine is read every other frame, 30 times a second.
const POLL_EVERY: u32 = 2;
const MENU_ITEM: f32 = 26.0;
const MENU_HEADER: f32 = 24.0;

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

struct Rename {
    id: String,
}

/// One line of the source menu: a source, or a group header.
struct Entry {
    id: Option<String>,
    label: String,
    checked: bool,
}

pub struct PanelState {
    ui: Panel,
    state: Arc<AppState>,
    lang: Lang,
    worker: Worker,
    devices: Option<DeviceList>,
    config: AppConfig,
    status: Option<Status>,
    seen_revision: Option<u64>,
    autostart: bool,
    editing: bool,
    /// Whether the other devices are unfolded; `None` until the user says.
    others_open: Option<bool>,
    others_animate: bool,
    rename: Option<Rename>,
    rename_errors: HashMap<String, String>,
    /// The focused item of the open source menu.
    menu: Option<usize>,
    source_meter: Meter,
    meters: HashMap<String, Meter>,
    /// When the meters last moved, to step them by the time elapsed.
    meters_at: Option<Instant>,
    ticks: u32,
    frames: Option<Timer>,
    /// The ids and names of the rows the panel shows, in order.
    shown: Vec<(String, String)>,
    outputs: Rc<VecModel<OutputRow>>,
    levels: Rc<VecModel<f32>>,
    others: Rc<VecModel<OtherRow>>,
    menu_model: Rc<VecModel<MenuEntry>>,
}

thread_local! {
    static PANEL: RefCell<Option<PanelState>> = const { RefCell::new(None) };
}

/// Runs `f` on the panel. Slint can call back into the panel while it is
/// busy (a focus change while the window hides); the call then runs as soon
/// as the current one is over.
pub fn with(f: impl FnOnce(&mut PanelState) + 'static) {
    PANEL.with(|cell| match cell.try_borrow_mut() {
        Ok(mut panel) => {
            if let Some(p) = panel.as_mut() {
                f(p);
            }
        }
        Err(_) => Timer::single_shot(Duration::ZERO, move || with(f)),
    });
}

/// Sets the panel up once, and returns its window.
pub fn create(state: Arc<AppState>) -> Result<Panel, slint::PlatformError> {
    let ui = Panel::new()?;
    let lang = state.lang;
    ui.set_texts(Texts {
        source: lang.t("source.title").into(),
        outputs: lang.t("outputs.title").into(),
        edit: lang.t("outputs.edit").into(),
        done: lang.t("outputs.done").into(),
        rename: lang.t("output.rename").into(),
        remove: lang.t("output.remove").into(),
        add: lang.t("output.add").into(),
        show: lang.t("outputs.show").into(),
        hide: lang.t("outputs.hide").into(),
        autostart: lang.t("autostart").into(),
        quit: lang.t("quit").into(),
        update: lang.t("update.restart").into(),
        version: format!("v{}", env!("CARGO_PKG_VERSION")).into(),
    });
    let outputs = Rc::new(VecModel::default());
    let levels = Rc::new(VecModel::default());
    let others = Rc::new(VecModel::default());
    let menu_model = Rc::new(VecModel::default());
    ui.set_outputs(ModelRc::from(outputs.clone()));
    ui.set_levels(ModelRc::from(levels.clone()));
    ui.set_others(ModelRc::from(others.clone()));
    ui.set_menu(ModelRc::from(menu_model.clone()));
    bind(&ui);

    let panel = PanelState {
        ui: ui.clone_strong(),
        lang,
        worker: Worker::new(state.clone()),
        config: state.config(),
        state,
        devices: None,
        status: None,
        seen_revision: None,
        autostart: false,
        editing: false,
        others_open: None,
        others_animate: false,
        rename: None,
        rename_errors: HashMap::new(),
        menu: None,
        source_meter: Meter::default(),
        meters: HashMap::new(),
        meters_at: None,
        ticks: 0,
        frames: None,
        shown: Vec::new(),
        outputs,
        levels,
        others,
        menu_model,
    };
    PANEL.with(|cell| *cell.borrow_mut() = Some(panel));
    with(PanelState::refresh);
    Ok(ui)
}

/// Every callback of the panel, routed to [`PanelState`].
fn bind(ui: &Panel) {
    ui.on_hide_panel(|| {
        Timer::single_shot(Duration::ZERO, super::hide_panel);
    });
    ui.on_quit(super::quit);
    ui.on_relaunch(|| {
        crate::updater::relaunch();
        super::quit();
    });
    ui.on_toggle_edit(|| {
        with(|p| {
            p.editing = !p.editing;
            if p.rename.is_some() {
                p.end_rename(None);
            }
            p.refresh();
        })
    });
    ui.on_toggle_others(|| {
        with(|p| {
            let open = p.others_open();
            p.others_open = Some(!open);
            p.others_animate = !open;
            p.refresh();
        })
    });
    ui.on_toggle_autostart(|| with(PanelState::toggle_autostart));
    ui.on_set_enabled(|id, enabled| with(move |p| p.set_enabled(&id, enabled)));
    ui.on_toggle_mute(|id| with(move |p| p.toggle_mute(&id)));
    ui.on_fader(|id, value, persist| with(move |p| p.set_fader(&id, value, persist)));
    ui.on_step(|id, delta| {
        with(move |p| {
            let fader = p.output_config(&id).0 + delta;
            p.set_fader(&id, fader, true);
        })
    });
    ui.on_start_rename(|id| with(move |p| p.start_rename(&id)));
    ui.on_rename_done(|id, text, save| {
        with(move |p| {
            if p.rename.as_ref().is_some_and(|r| r.id == id.as_str()) {
                p.end_rename(save.then(|| text.to_string()));
            }
        })
    });
    ui.on_open_menu(|| with(PanelState::open_menu));
    ui.on_close_menu(|| with(PanelState::close_menu));
    ui.on_menu_move(|delta| with(move |p| p.menu_move(delta)));
    ui.on_menu_hover(|index| {
        with(move |p| {
            if p.menu.is_some() && p.menu != Some(index as usize) {
                p.menu = Some(index as usize);
                p.ui.set_menu_index(index);
            }
        })
    });
    ui.on_menu_choose(|index| with(move |p| p.menu_choose(index as usize)));
}

impl PanelState {
    /// The window was shown: read fresh devices and follow the engine.
    pub fn opened(&mut self) {
        self.load();
        self.ticks = 0;
        let timer = Timer::default();
        timer.start(TimerMode::Repeated, FRAME, || with(PanelState::tick));
        self.frames = Some(timer);
        self.tick();
    }

    /// The window was hidden: stop following the engine, drop what was held.
    pub fn closed(&mut self) {
        self.frames = None;
        self.meters_at = None;
        self.menu = None;
        if self.rename.is_some() {
            self.end_rename(None);
        }
        self.refresh();
    }

    pub fn update_ready(&mut self) {
        self.refresh();
    }

    /// Reads the devices and the settings again, off the UI thread:
    /// enumerating is a full pass over the system's devices.
    pub fn load(&mut self) {
        std::thread::spawn(|| {
            let devices = audio::enumerate();
            let autostart = crate::autostart::is_enabled();
            let _ = slint::invoke_from_event_loop(move || {
                with(move |p| {
                    match devices {
                        Ok(d) => p.devices = Some(d),
                        Err(e) => log::warn!("devices: {e}"),
                    }
                    p.config = p.state.config();
                    p.autostart = autostart;
                    p.drop_stale();
                    p.refresh();
                })
            });
        });
    }

    /// A rename or a menu over a device that went away ends with it.
    fn drop_stale(&mut self) {
        let Some(devices) = &self.devices else {
            return;
        };
        if self
            .rename
            .as_ref()
            .is_some_and(|r| !devices.outputs.iter().any(|o| o.id == r.id))
        {
            self.rename = None;
        }
    }

    /// One frame: the engine every other frame, the meters every frame.
    fn tick(&mut self) {
        if self.ticks.is_multiple_of(POLL_EVERY) {
            self.poll();
        }
        self.ticks = self.ticks.wrapping_add(1);
        self.step_meters();
    }

    fn poll(&mut self) {
        let status = self.state.status();
        // The backends know when a device appears or goes away, so the list
        // is read again on their word rather than on a timer.
        if self
            .seen_revision
            .is_some_and(|r| r != status.devices_revision)
        {
            self.load();
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
        let changed = self
            .status
            .as_ref()
            .is_none_or(|s| !same_status(s, &status));
        self.status = Some(status);
        if changed {
            self.refresh();
        }
    }

    /// Meters: eased rise, steady fall, by the time since the last frame
    /// (capped, so a panel shown again does not jump).
    fn step_meters(&mut self) {
        let now = Instant::now();
        let dt = self
            .meters_at
            .map_or(0.0, |t| now.duration_since(t).as_secs_f32().min(0.1));
        self.meters_at = Some(now);
        for m in std::iter::once(&mut self.source_meter).chain(self.meters.values_mut()) {
            m.level = view::meter_step(m.level, m.target, dt);
        }
        let level = self.source_meter.level;
        if self.ui.get_source_level() != level {
            self.ui.set_source_level(level);
            self.ui
                .set_source_db(view::format_db(self.lang, view::meter_db(level)).into());
        }
        for (i, (id, _)) in self.shown.iter().enumerate() {
            let level = self.meters.get(id).map_or(0.0, |m| m.level);
            if self.levels.row_data(i) != Some(level) {
                self.levels.set_row_data(i, level);
            }
        }
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

    fn name_of(&self, id: &str) -> String {
        self.devices
            .as_ref()
            .and_then(|d| d.outputs.iter().find(|o| o.id == id))
            .map(|o| o.name.clone())
            .unwrap_or_default()
    }

    fn others_open(&self) -> bool {
        let rows = self.rows();
        let nothing_plays = view::split_outputs(&rows, &self.config).0.is_empty();
        self.others_open.unwrap_or(nothing_plays)
    }

    /* Actions */

    fn set_enabled(&mut self, id: &str, enabled: bool) {
        let name = self.name_of(id);
        self.config.output_mut(id, &name).enabled = enabled;
        if enabled {
            // The list stays unfolded for the next one.
            self.others_open.get_or_insert(true);
        }
        let id = id.to_owned();
        self.worker
            .run(move |s| s.set_output_enabled(&id, &name, enabled));
        let rows = self.rows();
        if view::split_outputs(&rows, &self.config).0.is_empty() {
            self.editing = false;
        }
        self.refresh();
    }

    fn toggle_mute(&mut self, id: &str) {
        let name = self.name_of(id);
        let muted = !self.output_config(id).1;
        self.config.output_mut(id, &name).muted = muted;
        let id = id.to_owned();
        self.worker
            .run(move |s| s.set_output_muted(&id, &name, muted));
        self.refresh();
    }

    /// Moves a fader. While it is dragged (`persist` false) the volume goes
    /// straight to the audio thread, in memory only; the final position is
    /// written to disk.
    fn set_fader(&mut self, id: &str, fader: f32, persist: bool) {
        if self.rename.is_some() {
            self.end_rename(None);
        }
        let fader = fader.clamp(0.0, 1.0);
        let name = self.name_of(id);
        let unchanged = self.output_config(id).0 == fader;
        self.config.output_mut(id, &name).fader = fader;
        if persist {
            let id = id.to_owned();
            self.worker
                .run(move |s| s.set_output_volume(&id, &name, fader, true));
        } else if !unchanged {
            self.state.set_output_volume(id, &name, fader, false);
        }
        if !unchanged {
            self.refresh();
        }
    }

    fn choose_source(&mut self, id: String) {
        if id == self.config.source {
            return;
        }
        self.config.source = id.clone();
        self.worker.run(move |s| s.set_source(id));
        self.refresh();
    }

    fn toggle_autostart(&mut self) {
        let wanted = !self.autostart;
        self.autostart = wanted;
        std::thread::spawn(move || {
            let result = crate::autostart::set(wanted);
            let _ = slint::invoke_from_event_loop(move || {
                with(move |p| {
                    match result {
                        Ok(now) => p.autostart = now,
                        Err(e) => {
                            log::warn!("autostart: {e}");
                            p.autostart = !wanted;
                        }
                    }
                    p.refresh();
                })
            });
        });
        self.refresh();
    }

    /* Rename: the name changes in the system, so every application shows it. */

    fn start_rename(&mut self, id: &str) {
        let Some(dev) = self
            .devices
            .as_ref()
            .and_then(|d| d.outputs.iter().find(|o| o.id == id))
            .cloned()
        else {
            return;
        };
        let Some(description) = dev.description.clone() else {
            return;
        };
        self.rename_errors.remove(id);
        // What the system keeps, such as the adapter in "Speakers (Realtek Audio)".
        let suffix = dev
            .name
            .strip_prefix(description.as_str())
            .map(|s| s.trim().to_owned())
            .unwrap_or_default();
        self.ui.set_rename_text(description.into());
        self.ui.set_rename_suffix(suffix.into());
        self.rename = Some(Rename { id: id.to_owned() });
        self.refresh();
    }

    /// Ends the rename; `text` is the name to save, if any.
    fn end_rename(&mut self, text: Option<String>) {
        let Some(rename) = self.rename.take() else {
            return;
        };
        self.refresh();
        let Some(text) = text.map(|t| t.trim().to_owned()) else {
            return;
        };
        let current = self
            .devices
            .as_ref()
            .and_then(|d| d.outputs.iter().find(|o| o.id == rename.id))
            .and_then(|o| o.description.clone());
        if text.is_empty() || Some(&text) == current.as_ref() {
            return;
        }
        let id = rename.id;
        std::thread::spawn(move || {
            let result = audio::rename_output(&id, &text);
            let _ = slint::invoke_from_event_loop(move || {
                with(move |p| {
                    match result {
                        Ok(()) => {
                            p.rename_errors.remove(&id);
                            // The system names it differently now: read the list again.
                            p.load();
                        }
                        Err(e) => {
                            log::warn!("rename: {e}");
                            p.rename_errors
                                .insert(id, p.lang.t("output.renameFailed").to_owned());
                        }
                    }
                    p.refresh();
                })
            });
        });
    }

    /* Source menu */

    fn menu_entries(&self) -> Vec<Entry> {
        let lang = self.lang;
        let mut entries = Vec::new();
        let Some(devices) = &self.devices else {
            return entries;
        };
        let current = &self.config.source;
        if !devices.sources.iter().any(|s| &s.id == current) {
            entries.push(Entry {
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
                entries.push(Entry {
                    id: None,
                    label: lang.t(h).to_owned(),
                    checked: false,
                });
            }
            for s in items {
                entries.push(Entry {
                    id: Some(s.id.clone()),
                    label: view::source_name(lang, s),
                    checked: &s.id == current,
                });
            }
        }
        entries
    }

    fn open_menu(&mut self) {
        let entries = self.menu_entries();
        let at = entries
            .iter()
            .position(|e| e.checked)
            .or_else(|| entries.iter().position(|e| e.id.is_some()));
        if at.is_none() {
            return;
        }
        self.menu = at;
        self.refresh();
    }

    fn close_menu(&mut self) {
        if self.menu.take().is_some() {
            self.refresh();
            self.ui.invoke_focus_picker();
        }
    }

    fn menu_move(&mut self, delta: i32) {
        let Some(at) = self.menu else {
            return;
        };
        let entries = self.menu_entries();
        let items: Vec<usize> = (0..entries.len())
            .filter(|&i| entries[i].id.is_some())
            .collect();
        let Some(last) = items.len().checked_sub(1) else {
            return;
        };
        let pos = items.iter().position(|&i| i == at).unwrap_or(0);
        let next = (pos as i64 + i64::from(delta)).clamp(0, last as i64) as usize;
        self.menu = Some(items[next]);
        self.ui.set_menu_index(items[next] as i32);
    }

    fn menu_choose(&mut self, index: usize) {
        let id = self
            .menu_entries()
            .into_iter()
            .nth(index)
            .and_then(|e| e.id);
        let Some(id) = id else {
            return;
        };
        self.close_menu();
        self.choose_source(id);
    }

    /* Display */

    /// Hands the window everything it shows.
    pub fn refresh(&mut self) {
        let lang = self.lang;
        let ui = &self.ui;
        let rows = self.rows();
        let (active, others) = self
            .devices
            .as_ref()
            .map(|_| view::split_outputs(&rows, &self.config))
            .unwrap_or_default();
        let live = self.status.as_ref().filter(|s| s.running);
        let (run_text, run_tone) = view::run_state(lang, live, &rows);
        ui.set_run_text(run_text.into());
        ui.set_run_tone(match run_tone {
            Tone::On => 0,
            Tone::Muted => 1,
            Tone::Error => 2,
        });
        ui.set_update_ready(self.state.update_ready.lock().is_some());

        // Source.
        let entries = self.menu_entries();
        let label = entries
            .iter()
            .find(|e| e.checked)
            .map(|e| e.label.clone())
            .unwrap_or_default();
        ui.set_source_label(label.into());
        let source_error = live
            .filter(|s| s.source.state == audio::NodeState::Error)
            .map(|s| view::retrying(lang, s.source.message.as_ref()))
            .unwrap_or_default();
        ui.set_source_error(source_error.into());
        ui.set_source_db(view::format_db(lang, view::meter_db(self.source_meter.level)).into());

        // Outputs.
        let empty = if self.devices.is_none() {
            lang.t("outputs.searching")
        } else if active.is_empty() && rows.is_empty() {
            lang.t("outputs.none")
        } else if active.is_empty() {
            lang.t("outputs.hint")
        } else {
            ""
        };
        ui.set_empty_text(empty.into());
        let statuses: HashMap<&str, &audio::OutputStatus> = live
            .map(|s| s.outputs.iter().map(|o| (o.id.as_str(), o)).collect())
            .unwrap_or_default();
        let new_rows: Vec<OutputRow> = active
            .iter()
            .map(|dev| {
                let (fader, muted) = self.output_config(&dev.id);
                let state =
                    view::output_state(lang, statuses.get(dev.id.as_str()).copied(), true, muted);
                let detail = [self.rename_errors.get(&dev.id).cloned(), Some(state.detail)]
                    .into_iter()
                    .flatten()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                OutputRow {
                    id: dev.id.as_str().into(),
                    name: view::device_name(lang, dev).into(),
                    state: state.label.into(),
                    error: state.tone == Tone::Error,
                    detail: detail.into(),
                    fader,
                    muted,
                    db: view::fader_db(lang, fader).into(),
                    can_rename: dev.description.is_some(),
                }
            })
            .collect();
        self.shown = active
            .iter()
            .map(|d| (d.id.clone(), d.name.clone()))
            .collect();
        sync(&self.outputs, new_rows);
        let levels: Vec<f32> = self
            .shown
            .iter()
            .map(|(id, _)| self.meters.get(id).map_or(0.0, |m| m.level))
            .collect();
        sync(&self.levels, levels);
        ui.set_editing(self.editing && !active.is_empty());
        ui.set_renaming_id(
            self.rename
                .as_ref()
                .map_or(SharedString::new(), |r| r.id.as_str().into()),
        );

        // Other devices.
        let open = self.others_open.unwrap_or(active.is_empty());
        ui.set_others_label(lang.plural("outputs.others", others.len(), &[]).into());
        ui.set_others_open(open);
        ui.set_others_animate(self.others_animate);
        sync(
            &self.others,
            others
                .iter()
                .map(|d| OtherRow {
                    id: d.id.as_str().into(),
                    name: view::device_name(lang, d).into(),
                })
                .collect(),
        );

        // Menu.
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
        let current = entries
            .iter()
            .position(|e| e.checked)
            .or(self.menu)
            .unwrap_or(0);
        ui.set_menu_content(heights.iter().sum());
        ui.set_menu_current(heights[..current.min(heights.len())].iter().sum());
        sync(
            &self.menu_model,
            entries
                .into_iter()
                .map(|e| MenuEntry {
                    header: e.id.is_none(),
                    id: e.id.unwrap_or_default().into(),
                    label: e.label.into(),
                    checked: e.checked,
                })
                .collect(),
        );
        ui.set_menu_index(self.menu.map_or(-1, |m| m as i32));
        ui.set_menu_open(self.menu.is_some());

        ui.set_autostart(self.autostart);
    }
}

/// Puts `rows` in `model`, touching only the rows that changed, so the
/// elements on screen (and the focus inside them) stay where they are.
fn sync<T: Clone + PartialEq + 'static>(model: &VecModel<T>, rows: Vec<T>) {
    if model.row_count() != rows.len() {
        model.set_vec(rows);
        return;
    }
    for (i, row) in rows.into_iter().enumerate() {
        if model.row_data(i).as_ref() != Some(&row) {
            model.set_row_data(i, row);
        }
    }
}

/// Whether two readings of the engine show the same thing, peaks aside.
fn same_status(a: &Status, b: &Status) -> bool {
    a.running == b.running
        && a.source.state == b.source.state
        && a.source.message == b.source.message
        && a.outputs.len() == b.outputs.len()
        && a.outputs
            .iter()
            .zip(&b.outputs)
            .all(|(x, y)| x.id == y.id && x.state == y.state && x.message == y.message)
}
