//! The GPUI shell: the tray icon, the panel that unfolds next to it, and the
//! update splash shown at launch.
//!
//! The panel is a borderless pop-up anchored to the tray icon, on the
//! taskbar side of the screen, like the OneDrive flyout. It hides when it
//! loses focus; GPUI cannot hide a window, so hiding removes it and showing
//! opens a new one around the same panel entity.

mod assets;
mod native;
mod panel;
mod splash;
mod text_field;
mod theme;
mod tray;
mod view;
mod widgets;

use std::borrow::Cow;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures::StreamExt;
use gpui::{
    point, px, size, App, AppContext, Application, Bounds, Entity, Global, Task, WindowBounds,
    WindowDecorations, WindowHandle, WindowKind, WindowOptions,
};

use crate::state::AppState;
use crate::updater::{self, Stage, Updater};

use native::{Anchor, NativeWindow};
use panel::Panel;
use splash::Splash;

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

struct Shell {
    state: Arc<AppState>,
    updater: Arc<Updater>,
    panel: Entity<Panel>,
    window: Option<WindowHandle<Panel>>,
    splash: Option<WindowHandle<Splash>>,
    tray: Option<tray::Tray>,
    events: UnboundedSender<AppEvent>,
    shown_at: Instant,
    hidden_at: Option<Instant>,
    had_focus: bool,
    started: bool,
    _watch: Option<Task<()>>,
}

impl Global for Shell {}

/// Runs the app until Quit.
pub fn run(
    state: impl FnOnce() -> Arc<AppState> + 'static,
    events: UnboundedSender<AppEvent>,
    mut incoming: UnboundedReceiver<AppEvent>,
) {
    Application::new()
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            native::accessory_app();
            if let Err(e) = cx
                .text_system()
                .add_fonts(vec![Cow::Borrowed(assets::INTER)])
            {
                log::warn!("font: {e}");
            }
            // The name the system registered Inter under ("Inter Variable",
            // or "Inter Variable Text" for DirectWrite).
            if let Some(name) = cx
                .text_system()
                .all_font_names()
                .into_iter()
                .find(|n| n.starts_with("Inter"))
            {
                theme::set_font(name);
            }
            #[cfg(not(target_os = "macos"))]
            keep_alive(cx);
            text_field::bind_keys(cx);
            panel::bind_keys(cx);

            let state = state();
            let panel = cx.new(|cx| Panel::new(state.clone(), cx));
            cx.set_global(Shell {
                state,
                updater: Arc::new(Updater::default()),
                panel,
                window: None,
                splash: None,
                tray: None,
                events,
                shown_at: Instant::now(),
                hidden_at: None,
                had_focus: false,
                started: false,
                _watch: None,
            });

            cx.spawn(async move |cx| {
                while let Some(event) = incoming.next().await {
                    if cx.update(|cx| handle(event, cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();

            if updater::enabled() {
                launch_splash(cx);
            } else {
                start(cx);
            }
        });
}

/// GPUI on Windows and Linux quits when its last window closes, and a tray
/// app has none open most of the time: one window that is never shown keeps
/// it up. X11 maps every window GPUI creates, so there it is a transparent
/// pixel far off screen.
#[cfg(not(target_os = "macos"))]
fn keep_alive(cx: &mut App) {
    struct Keeper;
    impl gpui::Render for Keeper {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui::Empty
        }
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(-10000.), px(-10000.)),
            size(px(1.), px(1.)),
        ))),
        titlebar: None,
        focus: false,
        show: false,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: None,
        window_background: gpui::WindowBackgroundAppearance::Transparent,
        app_id: None,
        window_min_size: None,
        window_decorations: None,
        tabbing_identifier: None,
    };
    if let Err(e) = cx.open_window(options, |_, cx| cx.new(|_| Keeper)) {
        log::error!("keeper window: {e}");
    }
}

fn handle(event: AppEvent, cx: &mut App) {
    match event {
        AppEvent::TrayClick(anchor) => toggle_panel(anchor, cx),
        AppEvent::Open => show_panel(None, cx),
        // During the startup update check, the splash is already showing.
        AppEvent::Show => show_panel(None, cx),
        AppEvent::RestartAudio => {
            // Reopens the capture and every output, and reads fresh devices.
            cx.global::<Shell>().state.restart_engine();
            let panel = cx.global::<Shell>().panel.clone();
            panel.update(cx, |p, cx| p.load(cx));
        }
        AppEvent::Quit => cx.quit(),
    }
}

/// Starts the tray, the audio and the panel, once the startup update check
/// is done.
fn start(cx: &mut App) {
    let shell = cx.global::<Shell>();
    shell.state.start_engine();
    // An install moved the executable: point the login item at the new path.
    crate::autostart::refresh();
    let tray = tray::setup(shell.state.lang, shell.events.clone());
    let shell = cx.global_mut::<Shell>();
    shell.tray = tray;
    shell.started = true;
    if !std::env::args().any(|a| a == crate::autostart::ARG) {
        show_panel(None, cx);
    }
}

fn toggle_panel(anchor: Option<Anchor>, cx: &mut App) {
    let shell = cx.global::<Shell>();
    if shell.window.is_some() {
        hide_panel(cx);
    } else if shell.hidden_at.is_none_or(|t| t.elapsed() > REOPEN_GUARD) {
        show_panel(anchor, cx);
    }
}

fn show_panel(anchor: Option<Anchor>, cx: &mut App) {
    let shell = cx.global::<Shell>();
    if !shell.started {
        return;
    }
    if let Some(window) = shell.window {
        let native = window
            .update(cx, |_, window, _| {
                let native = NativeWindow::of(window);
                if native.is_none() {
                    window.activate_window();
                }
                native
            })
            .ok()
            .flatten();
        cx.spawn(async move |_| {
            if let Some(n) = native {
                n.show();
            }
        })
        .detach();
        return;
    }
    let anchor = anchor.or_else(native::cursor);
    let panel_entity = shell.panel.clone();
    let radius =
        f64::from(theme::Theme::new(gpui::WindowAppearance::Light, native::glass()).radius_window);
    // Opened hidden where the window can be reached natively: it is rounded,
    // placed and shown once GPUI lets go of it (see `NativeWindow`).
    // Elsewhere (Linux) GPUI shows it, top right of the main display, where
    // the tray usually is.
    let native_placement = cfg!(any(windows, target_os = "macos"));
    let panel_size = size(px(panel::SIZE.0 as f32), px(panel::SIZE.1 as f32));
    let origin = match cx.primary_display().filter(|_| !native_placement) {
        Some(display) => {
            let b = display.bounds();
            let margin = px(native::MARGIN as f32);
            point(b.right() - panel_size.width - margin, b.top() + margin)
        }
        None => point(px(0.), px(0.)),
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(origin, panel_size))),
        titlebar: None,
        focus: !native_placement,
        show: !native_placement,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: None,
        window_background: native::background(),
        app_id: Some(crate::paths::IDENTIFIER.into()),
        window_min_size: None,
        window_decorations: Some(WindowDecorations::Client),
        tabbing_identifier: None,
    };
    let handle = match cx.open_window(options, move |_, _| panel_entity) {
        Ok(h) => h,
        Err(e) => {
            log::error!("panel: {e}");
            return;
        }
    };
    let shell = cx.global_mut::<Shell>();
    shell.window = Some(handle);
    shell.shown_at = Instant::now();
    shell.had_focus = false;

    let native = handle
        .update(cx, |panel, window, cx| {
            let subscriptions = vec![
                cx.observe_window_activation(window, |_, window, cx| {
                    let active = window.is_window_active();
                    cx.defer(move |cx| activation_changed(active, cx));
                }),
                cx.observe_window_appearance(window, |_, _, cx| cx.notify()),
            ];
            window.on_window_should_close(cx, |_, cx| {
                // The panel hides instead of closing; only Quit ends the app.
                cx.defer(hide_panel);
                false
            });
            panel.opened(subscriptions, window, cx);
            NativeWindow::of(window)
        })
        .ok()
        .flatten();
    let Some(native) = native else {
        return;
    };

    cx.spawn(async move |cx| {
        native.prepare(radius);
        native.place(anchor, panel::SIZE);
        native.show();
        // A display of another scale resizes the window as it lands on it:
        // place it again once it has.
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
        if !is_current(handle, cx) {
            return;
        }
        native.place(anchor, panel::SIZE);
        // Takes focus again once the grace is over, if the system refused it.
        cx.background_executor().timer(BLUR_GRACE).await;
        let refused = is_current(handle, cx)
            && handle
                .update(cx, |_, window, _| !window.is_window_active())
                .unwrap_or(false);
        if refused {
            native.show();
        }
    })
    .detach();

    #[cfg(windows)]
    watch_outside_click(handle, native, cx);
}

fn is_current(handle: WindowHandle<Panel>, cx: &mut gpui::AsyncApp) -> bool {
    cx.update(|cx| cx.global::<Shell>().window == Some(handle))
        .unwrap_or(false)
}

/// Windows refuses focus to a window shown while the user is clicking in
/// another one, and the refocus above can be refused for the same reason. A
/// panel without focus gets no blur, so while it is not the foreground window
/// a click outside it hides it here instead.
#[cfg(windows)]
fn watch_outside_click(handle: WindowHandle<Panel>, native: NativeWindow, cx: &mut App) {
    let id = native.hwnd();
    cx.spawn(async move |cx| {
        // A button held when the watch starts is not a new click.
        let mut was_pressed = true;
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(20))
                .await;
            if !is_current(handle, cx) {
                return;
            }
            let outside = native::click_outside(id);
            if outside && !was_pressed && !native::is_foreground(id) {
                log::info!("panel: hidden on a click outside it without focus");
                let _ = cx.update(hide_panel);
                return;
            }
            was_pressed = outside;
        }
    })
    .detach();
}

fn activation_changed(active: bool, cx: &mut App) {
    let shell = cx.global_mut::<Shell>();
    if shell.window.is_none() {
        return;
    }
    if active {
        shell.had_focus = true;
    } else if shell.had_focus && shell.shown_at.elapsed() > BLUR_GRACE {
        hide_panel(cx);
    }
}

pub(crate) fn hide_panel(cx: &mut App) {
    let shell = cx.global_mut::<Shell>();
    let Some(handle) = shell.window.take() else {
        return;
    };
    shell.hidden_at = Some(Instant::now());
    let panel = shell.panel.clone();
    panel.update(cx, |p, cx| p.closed(cx));
    let _ = handle.update(cx, |_, window, _| window.remove_window());
}

/// Shows the splash, applies a pending release, then calls [`start`].
fn launch_splash(cx: &mut App) {
    let shell = cx.global::<Shell>();
    let updater = shell.updater.clone();
    let lang = shell.state.lang;
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(splash::SIZE.0), px(splash::SIZE.1)),
            cx,
        ))),
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: None,
        window_background: gpui::WindowBackgroundAppearance::Opaque,
        app_id: Some(crate::paths::IDENTIFIER.into()),
        window_min_size: None,
        window_decorations: Some(WindowDecorations::Client),
        tabbing_identifier: None,
    };
    let splash_updater = updater.clone();
    match cx.open_window(options, move |_, cx| {
        cx.new(|cx| Splash::new(splash_updater, lang, cx))
    }) {
        Ok(h) => {
            cx.global_mut::<Shell>().splash = Some(h);
            if let Ok(Some(native)) = h.update(cx, |_, window, _| NativeWindow::of(window)) {
                cx.spawn(async move |_| native.prepare(12.0)).detach();
            }
        }
        Err(e) => log::error!("splash: {e}"),
    }

    let check = cx.background_executor().spawn({
        let updater = updater.clone();
        async move {
            let shown = Instant::now();
            let result = updater.check_and_install(true);
            (result, shown.elapsed())
        }
    });
    cx.spawn(async move |cx| {
        let (result, elapsed) = check.await;
        let wait = match result {
            Ok(Some(_)) => {
                updater.set_stage(Stage::Restarting);
                updater::relaunch();
                let _ = cx.update(|cx| cx.quit());
                return;
            }
            Ok(None) => updater::CHECK_EVERY,
            Err(e) => e.wait(),
        };
        if let Some(rest) = updater::SPLASH_MIN.checked_sub(elapsed) {
            cx.background_executor().timer(rest).await;
        }
        let _ = cx.update(|cx| {
            if let Some(splash) = cx.global_mut::<Shell>().splash.take() {
                let _ = splash.update(cx, |_, window, _| window.remove_window());
            }
            start(cx);
            watch_updates(wait, cx);
        });
    })
    .detach();
}

/// Checks again while the app runs; a found version waits for a restart.
fn watch_updates(first: Duration, cx: &mut App) {
    let shell = cx.global::<Shell>();
    let updater = shell.updater.clone();
    let state = shell.state.clone();
    let panel = shell.panel.clone();
    let task = cx.spawn(async move |cx| {
        let mut wait = first;
        loop {
            cx.background_executor().timer(wait).await;
            let check = cx.background_executor().spawn({
                let updater = updater.clone();
                async move { updater.check_and_install(false) }
            });
            wait = match check.await {
                Ok(Some(version)) => {
                    *state.update_ready.lock() = Some(version);
                    let _ = panel.update(cx, |p, cx| p.update_ready(cx));
                    // Installed: nothing more to do until the next restart.
                    return;
                }
                Ok(None) => updater::CHECK_EVERY,
                Err(e) => e.wait(),
            };
        }
    });
    cx.global_mut::<Shell>()._watch = Some(task);
}
