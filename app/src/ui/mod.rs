//! The Slint shell: the tray icon, the panel that unfolds next to it, and
//! the update splash shown at launch.
//!
//! The panel is a borderless pop-up anchored to the tray icon, on the
//! taskbar side of the screen, like the OneDrive flyout. It hides when it
//! loses focus, and the event loop keeps running with no window shown.

mod native;
mod panel;
#[cfg(test)]
mod preview;
mod tray;
mod view;

#[allow(clippy::all, clippy::pedantic)]
mod generated {
    slint::include_modules!();
}

use std::cell::RefCell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use slint::winit_030::{winit, EventResult, WinitWindowAccessor};
use slint::{ComponentHandle, PhysicalPosition, Timer, TimerMode};

use crate::state::AppState;
use crate::updater::{self, Stage, Updater};

pub(crate) use generated::{MenuEntry, OtherRow, OutputRow, Panel, Platform, Splash, Texts};
use native::{Anchor, NativeWindow};

/// What reaches the UI from outside it: the tray, a second launch.
pub enum AppEvent {
    TrayClick(Option<Anchor>),
    Open,
    RestartAudio,
    Quit,
    /// Another launch asked for the panel.
    Show,
}

/// A click arriving right after the panel lost focus is the click that
/// dismissed it; it must not reopen the panel.
const REOPEN_GUARD: Duration = Duration::from_millis(250);
/// Windows may hand focus straight back to the previous window when the
/// panel is shown without user input (launch from a terminal, second
/// instance), or refuse it outright. A blur in the first moments after
/// showing is that, not the user leaving; once they are over, an unfocused
/// panel takes focus again, since a panel that does not have it never sees
/// the blur that should hide it.
const BLUR_GRACE: Duration = Duration::from_millis(400);
const SPLASH_SIZE: (f64, f64) = (300.0, 110.0);

struct Shell {
    state: Arc<AppState>,
    updater: Arc<Updater>,
    panel: Panel,
    native: Option<NativeWindow>,
    splash: Option<(Splash, Timer)>,
    tray: Option<tray::Tray>,
    events: UnboundedSender<AppEvent>,
    visible: bool,
    shown_at: Instant,
    hidden_at: Option<Instant>,
    had_focus: bool,
    focused: bool,
    started: bool,
    /// Bumped each time the panel shows, so timers of an older showing stop.
    showing: u64,
    /// While the panel shows, on Windows: see [`watch_outside_click`].
    outside: Option<Timer>,
}

thread_local! {
    static SHELL: RefCell<Option<Shell>> = const { RefCell::new(None) };
}

/// Runs `f` on the shell, or right after the current call into it when one
/// is under way (a window event the system sends while the shell shows a
/// window).
fn with_shell<R>(f: impl FnOnce(&mut Shell) -> R) -> Option<R> {
    SHELL.with(|cell| {
        let mut shell = cell.try_borrow_mut().ok()?;
        shell.as_mut().map(f)
    })
}

fn later(f: impl FnOnce() + 'static) {
    Timer::single_shot(Duration::ZERO, f);
}

/// Runs the app until Quit.
pub fn run(
    state: impl FnOnce() -> Arc<AppState>,
    events: UnboundedSender<AppEvent>,
    incoming: UnboundedReceiver<AppEvent>,
) {
    native::init_ui_thread();
    let selected = slint::BackendSelector::new()
        .backend_name("winit".into())
        .with_winit_window_attributes_hook(window_attributes)
        .select();
    if let Err(e) = selected {
        log::error!("ui: {e}");
        return;
    }

    let state = state();
    let panel = match panel::create(state.clone()) {
        Ok(p) => p,
        Err(e) => {
            log::error!("panel: {e}");
            return;
        }
    };
    set_platform(panel.global::<Platform>());
    panel.window().on_close_requested(|| {
        // The panel hides instead of closing; only Quit ends the app.
        later(hide_panel);
        slint::CloseRequestResponse::KeepWindowShown
    });
    panel.window().on_winit_window_event(|_, event| {
        if let winit::event::WindowEvent::Focused(active) = *event {
            later(move || activation_changed(active));
        }
        EventResult::Propagate
    });

    SHELL.with(|cell| {
        *cell.borrow_mut() = Some(Shell {
            state,
            updater: Arc::new(Updater::default()),
            panel,
            native: None,
            splash: None,
            tray: None,
            events,
            visible: false,
            shown_at: Instant::now(),
            hidden_at: None,
            had_focus: false,
            focused: false,
            started: false,
            showing: 0,
            outside: None,
        })
    });

    // Tray clicks and second launches arrive on other threads.
    let forward = std::thread::Builder::new()
        .name("ui-events".into())
        .spawn(move || {
            for event in futures::executor::block_on_stream(incoming) {
                if slint::invoke_from_event_loop(move || handle(event)).is_err() {
                    break;
                }
            }
        });
    if let Err(e) = forward {
        log::error!("ui events: {e}");
    }

    later(|| {
        native::accessory_app();
        if updater::enabled() {
            launch_splash();
        } else {
            start();
        }
    });
    if let Err(e) = slint::run_event_loop_until_quit() {
        log::error!("ui: {e}");
    }
    SHELL.with(|cell| cell.borrow_mut().take());
}

/// Every window stays off the taskbar and out of Alt+Tab. On Windows it is
/// rounded, and the panel's Acrylic is asked for as it is created
/// (`native::system_backdrop`).
fn window_attributes(
    attributes: winit::window::WindowAttributes,
) -> winit::window::WindowAttributes {
    #[cfg(windows)]
    {
        use winit::platform::windows::{
            BackdropType, CornerPreference, WindowAttributesExtWindows,
        };
        let mut attributes = attributes
            .with_skip_taskbar(true)
            .with_corner_preference(CornerPreference::Round)
            .with_undecorated_shadow(true);
        if let Some(owner) = native::owner() {
            attributes = attributes.with_owner_window(owner);
        }
        if native::system_backdrop() {
            attributes.with_system_backdrop(BackdropType::TransientWindow)
        } else {
            attributes
        }
    }
    #[cfg(not(windows))]
    {
        attributes
    }
}

/// What the system gives the panel: its material, font and radius.
fn set_platform(platform: Platform) {
    let glass = native::glass();
    platform.set_glass(glass.code());
    platform.set_radius(if cfg!(windows) { 8.0 } else { 18.0 });
    if cfg!(target_os = "macos") {
        // SF Pro, left to the system, with its own weights.
        platform.set_font_family("".into());
        platform.set_regular(400);
        platform.set_medium(500);
        platform.set_semibold(600);
    } else {
        // The bundled Inter, since SF Pro's license keeps it on Apple
        // platforms; heavier weights make up for ClearType drawing it thinner.
        platform.set_font_family("Inter Variable".into());
        platform.set_regular(430);
        platform.set_medium(530);
        platform.set_semibold(620);
    }
}

fn handle(event: AppEvent) {
    match event {
        AppEvent::TrayClick(anchor) => toggle_panel(anchor),
        AppEvent::Open => show_panel(None),
        // During the startup update check, the splash is already showing.
        AppEvent::Show => show_panel(None),
        AppEvent::RestartAudio => {
            // Reopens the capture and every output, and reads fresh devices.
            with_shell(|s| s.state.restart_engine());
            panel::with(panel::PanelState::load);
        }
        AppEvent::Quit => quit(),
    }
}

pub(crate) fn quit() {
    let _ = slint::quit_event_loop();
}

/// Starts the tray, the audio and the panel, once the startup update check
/// is done.
fn start() {
    let Some((lang, events)) = with_shell(|s| {
        s.state.start_engine();
        (s.state.lang, s.events.clone())
    }) else {
        return;
    };
    // An install moved the executable: point the login item at the new path.
    crate::autostart::refresh();
    let tray = tray::setup(lang, events);
    with_shell(|s| {
        s.tray = tray;
        s.started = true;
    });
    if !std::env::args().any(|a| a == crate::autostart::ARG) {
        show_panel(None);
    }
}

fn toggle_panel(anchor: Option<Anchor>) {
    let Some((visible, hidden_at)) = with_shell(|s| (s.visible, s.hidden_at)) else {
        return;
    };
    if visible {
        hide_panel();
    } else if hidden_at.is_none_or(|t| t.elapsed() > REOPEN_GUARD) {
        show_panel(anchor);
    }
}

fn show_panel(anchor: Option<Anchor>) {
    let Some(shell) = with_shell(|s| {
        (
            s.started,
            s.visible,
            s.native,
            s.panel.clone_strong(),
            s.tray.as_ref().and_then(tray::Tray::anchor),
        )
    }) else {
        return;
    };
    let (started, visible, native, ui, tray_anchor) = shell;
    if !started {
        return;
    }
    if visible {
        if let Some(n) = native {
            n.show();
        }
        return;
    }
    // Opened without a click on the icon (launch, second launch, the
    // menu): next to the icon all the same, as a tray panel should be.
    // Without it (an icon in the overflow flyout), Windows falls back to the
    // taskbar corner; macOS uses the pointer, which sits in the menu bar
    // after a click on the status item.
    let anchor = anchor.or(tray_anchor).or_else(|| {
        if cfg!(target_os = "macos") {
            native::cursor()
        } else {
            None
        }
    });

    // Placed before it shows when it already exists, so it never flashes
    // where it was.
    if let Some(n) = native {
        n.place(anchor, panel::SIZE);
    }
    ui.set_settled(false);
    if let Err(e) = ui.show() {
        log::error!("panel: {e}");
        return;
    }
    let showing = with_shell(|s| {
        s.visible = true;
        s.shown_at = Instant::now();
        s.had_focus = false;
        s.focused = false;
        s.showing += 1;
        s.showing
    })
    .unwrap_or(0);
    panel::with(panel::PanelState::opened);
    ui.invoke_take_focus();
    // The header and footer settle in once the window is on screen.
    let settle = ui.as_weak();
    Timer::single_shot(Duration::from_millis(16), move || {
        if let Some(ui) = settle.upgrade().filter(|_| is_current(showing)) {
            ui.set_settled(true);
        }
    });
    match native {
        Some(n) => placed(showing, n, anchor, false),
        None => {
            // The first showing creates the native window, once the event
            // loop gets to it.
            let weak = ui.as_weak();
            let spawned = slint::spawn_local(async move {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                if ui.window().winit_window().await.is_err() || !is_current(showing) {
                    return;
                }
                match NativeWindow::of(ui.window()) {
                    Some(n) => {
                        with_shell(|s| s.native = Some(n));
                        placed(showing, n, anchor, true);
                    }
                    // No native placement (Linux): top right of the main
                    // display, where the tray usually is.
                    None => place_top_right(),
                }
            });
            if let Err(e) = spawned {
                log::error!("panel: {e}");
            }
        }
    }
}

/// The panel is on screen: rounded and dressed the first time, put next to
/// the tray icon, and given the keyboard.
fn placed(showing: u64, native: NativeWindow, anchor: Option<Anchor>, first: bool) {
    if first {
        native.prepare(if cfg!(windows) { 8.0 } else { 18.0 });
    }
    native.place(anchor, panel::SIZE);
    native.show();
    // A display of another scale resizes the window as it lands on it:
    // place it again once it has.
    Timer::single_shot(Duration::from_millis(50), move || {
        if is_current(showing) {
            native.place(anchor, panel::SIZE);
        }
    });
    // Takes focus again once the grace is over, if the system refused it.
    Timer::single_shot(BLUR_GRACE, move || {
        if is_current(showing) && with_shell(|s| !s.focused).unwrap_or(false) {
            native.show();
        }
    });

    #[cfg(windows)]
    watch_outside_click(showing, native);
}

fn is_current(showing: u64) -> bool {
    with_shell(|s| s.visible && s.showing == showing).unwrap_or(false)
}

/// Linux: the panel goes top right of the main display.
fn place_top_right() {
    let Some(ui) = with_shell(|s| s.panel.clone_strong()) else {
        return;
    };
    let window = ui.window();
    let size = window.size();
    let at = window.with_winit_window(|w| {
        let monitor = w.primary_monitor().or_else(|| w.current_monitor())?;
        let (pos, area) = (monitor.position(), monitor.size());
        let margin = (native::MARGIN * monitor.scale_factor()).round() as i32;
        Some(PhysicalPosition::new(
            pos.x + area.width as i32 - size.width as i32 - margin,
            pos.y + margin,
        ))
    });
    if let Some(Some(at)) = at {
        window.set_position(at);
    }
}

/// Windows refuses focus to a window shown while the user is clicking in
/// another one, and the refocus above can be refused for the same reason. A
/// panel without focus gets no blur, so while it is not the foreground window
/// a click outside it hides it here instead.
#[cfg(windows)]
fn watch_outside_click(showing: u64, native: NativeWindow) {
    let id = native.hwnd();
    // A button held when the watch starts is not a new click.
    let was_pressed = std::cell::Cell::new(true);
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(20), move || {
        if !is_current(showing) {
            return;
        }
        let outside = native::click_outside(id);
        if outside && !was_pressed.get() && !native::is_foreground(id) {
            log::info!("panel: hidden on a click outside it without focus");
            later(hide_panel);
        }
        was_pressed.set(outside);
    });
    with_shell(|s| s.outside = Some(timer));
}

fn activation_changed(active: bool) {
    let hide = with_shell(|s| {
        if !s.visible {
            return false;
        }
        s.focused = active;
        if active {
            s.had_focus = true;
            false
        } else {
            s.had_focus && s.shown_at.elapsed() > BLUR_GRACE
        }
    });
    if hide == Some(true) {
        hide_panel();
    }
}

pub(crate) fn hide_panel() {
    let Some(ui) = with_shell(|s| {
        if !s.visible {
            return None;
        }
        s.visible = false;
        s.hidden_at = Some(Instant::now());
        s.outside = None;
        Some(s.panel.clone_strong())
    })
    .flatten() else {
        return;
    };
    panel::with(panel::PanelState::closed);
    if let Err(e) = ui.hide() {
        log::error!("panel: {e}");
    }
}

/// Shows the splash, applies a pending release, then calls [`start`].
fn launch_splash() {
    let Some((updater, lang)) = with_shell(|s| (s.updater.clone(), s.state.lang)) else {
        return;
    };
    match Splash::new() {
        Ok(splash) => {
            set_platform(splash.global::<Platform>());
            if let Some((x, y)) = native::centered(SPLASH_SIZE) {
                splash.window().set_position(PhysicalPosition::new(x, y));
            }
            let text = {
                let updater = updater.clone();
                move |splash: &Splash| {
                    let progress = updater.progress();
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
                    splash.set_text(text.into());
                    splash.set_percent(percent.map_or(-1, i32::from));
                }
            };
            text(&splash);
            if let Err(e) = splash.show() {
                log::error!("splash: {e}");
            }
            // Rounded and centered once its native window exists.
            let weak = splash.as_weak();
            let spawned = slint::spawn_local(async move {
                let Some(splash) = weak.upgrade() else {
                    return;
                };
                if splash.window().winit_window().await.is_err() {
                    return;
                }
                if let Some(native) = NativeWindow::of(splash.window()) {
                    native.prepare(12.0);
                }
                centre_if_unplaced(&splash);
            });
            if let Err(e) = spawned {
                log::error!("splash: {e}");
            }
            let timer = Timer::default();
            let weak = splash.as_weak();
            timer.start(TimerMode::Repeated, Duration::from_millis(100), move || {
                if let Some(s) = weak.upgrade() {
                    text(&s);
                }
            });
            with_shell(|s| s.splash = Some((splash, timer)));
        }
        Err(e) => log::error!("splash: {e}"),
    }

    std::thread::spawn(move || {
        let shown = Instant::now();
        let result = updater.check_and_install(true);
        let elapsed = shown.elapsed();
        let wait = match result {
            Ok(Some(_)) => {
                updater.set_stage(Stage::Restarting);
                updater::relaunch();
                let _ = slint::invoke_from_event_loop(quit);
                return;
            }
            Ok(None) => updater::CHECK_EVERY,
            Err(e) => e.wait(),
        };
        if let Some(rest) = updater::SPLASH_MIN.checked_sub(elapsed) {
            std::thread::sleep(rest);
        }
        let _ = slint::invoke_from_event_loop(move || {
            if let Some((splash, _timer)) = with_shell(|s| s.splash.take()).flatten() {
                let _ = splash.hide();
            }
            start();
            watch_updates(wait);
        });
    });
}

/// Centers the splash on its display where the system could not say where
/// before it showed (macOS, Linux).
fn centre_if_unplaced(splash: &Splash) {
    if cfg!(windows) {
        return;
    }
    let window = splash.window();
    let size = window.size();
    let at = window.with_winit_window(|w| {
        let monitor = w.current_monitor().or_else(|| w.primary_monitor())?;
        let (pos, area) = (monitor.position(), monitor.size());
        Some(PhysicalPosition::new(
            pos.x + (area.width as i32 - size.width as i32) / 2,
            pos.y + (area.height as i32 - size.height as i32) / 2,
        ))
    });
    if let Some(Some(at)) = at {
        window.set_position(at);
    }
}

/// Checks again while the app runs; a found version waits for a restart.
fn watch_updates(first: Duration) {
    let Some((updater, state)) = with_shell(|s| (s.updater.clone(), s.state.clone())) else {
        return;
    };
    let spawned = std::thread::Builder::new()
        .name("updates".into())
        .spawn(move || {
            let mut wait = first;
            loop {
                std::thread::sleep(wait);
                wait = match updater.check_and_install(false) {
                    Ok(Some(version)) => {
                        *state.update_ready.lock() = Some(version);
                        let _ = slint::invoke_from_event_loop(|| {
                            panel::with(panel::PanelState::update_ready)
                        });
                        // Installed: nothing more to do until the next restart.
                        return;
                    }
                    Ok(None) => updater::CHECK_EVERY,
                    Err(e) => e.wait(),
                };
            }
        });
    if let Err(e) = spawned {
        log::error!("updates: {e}");
    }
}
