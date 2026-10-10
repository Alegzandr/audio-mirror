//! Tray icon and the panel that unfolds above it.
//!
//! Left click toggles a small undecorated window anchored to the icon, on the
//! taskbar side of the screen, like the OneDrive flyout. Right click opens a
//! menu. Linux trays do not report clicks, so the menu also offers "Open".

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, Rect, WebviewWindow, WindowEvent};

pub const PANEL: &str = "main";
const TRAY_ID: &str = "tray";
/// Gap between the panel and the taskbar, in logical pixels.
const MARGIN: f64 = 12.0;
/// A click arriving right after the panel lost focus is the click that
/// dismissed it; it must not reopen the panel.
const REOPEN_GUARD_MS: u64 = 250;

static HIDDEN_AT: AtomicU64 = AtomicU64::new(0);
/// Windows may hand focus straight back to the previous window when the panel
/// is shown without user input (launch from a terminal, second instance), or
/// refuse it outright. A blur in the first moments after showing is that, not
/// the user leaving; once they are over, an unfocused panel takes focus again,
/// since a panel that does not have it never sees the click that should hide it.
const BLUR_GRACE_MS: u64 = 400;

static SHOWN_AT: AtomicU64 = AtomicU64::new(0);
/// Whether the panel got focus since it was last shown.
static HAD_FOCUS: AtomicBool = AtomicBool::new(false);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let text = app.state::<crate::AppState>().lang.tray();
    let open = MenuItem::with_id(app, "open", text.open, true, None::<&str>)?;
    let restart = MenuItem::with_id(app, "restart", text.restart, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", text.quit, true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let sep_quit = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &sep, &restart, &sep_quit, &quit])?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Audio Mirror")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_panel(app),
            "restart" => restart_audio(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                toggle_panel(tray.app_handle(), Some(rect));
            }
        });

    if cfg!(target_os = "macos") {
        let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray-template.png"))?;
        builder = builder.icon(icon).icon_as_template(true);
    } else if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;

    if let Some(panel) = app.get_webview_window(PANEL) {
        let handle = panel.clone();
        panel.on_window_event(move |event| match event {
            WindowEvent::Focused(true) => HAD_FOCUS.store(true, Ordering::Relaxed),
            WindowEvent::Focused(false) => {
                let settled =
                    now_ms().saturating_sub(SHOWN_AT.load(Ordering::Relaxed)) > BLUR_GRACE_MS;
                if HAD_FOCUS.load(Ordering::Relaxed) && settled {
                    hide(&handle);
                }
            }
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                hide(&handle);
            }
            _ => {}
        });
    }
    Ok(())
}

/// "Restart audio": reopens the capture and every output, and reloads the
/// panel so it reads fresh devices and state.
fn restart_audio(app: &AppHandle) {
    app.state::<crate::AppState>().engine.restart();
    if let Some(panel) = app.get_webview_window(PANEL) {
        let _ = panel.reload();
    }
}

fn hide(panel: &WebviewWindow) {
    if panel.is_visible().unwrap_or(false) {
        HIDDEN_AT.store(now_ms(), Ordering::Relaxed);
        let _ = panel.hide();
    }
}

pub fn hide_panel(app: &AppHandle) {
    if let Some(panel) = app.get_webview_window(PANEL) {
        hide(&panel);
    }
}

fn toggle_panel(app: &AppHandle, anchor: Option<Rect>) {
    let Some(panel) = app.get_webview_window(PANEL) else {
        return;
    };
    if panel.is_visible().unwrap_or(false) {
        hide(&panel);
    } else if now_ms().saturating_sub(HIDDEN_AT.load(Ordering::Relaxed)) > REOPEN_GUARD_MS {
        open_at(app, &panel, anchor);
    }
}

pub fn show_panel(app: &AppHandle) {
    let Some(panel) = app.get_webview_window(PANEL) else {
        return;
    };
    let anchor = app
        .tray_by_id(TRAY_ID)
        .and_then(|t| t.rect().ok().flatten());
    open_at(app, &panel, anchor);
}

fn open_at(app: &AppHandle, panel: &WebviewWindow, anchor: Option<Rect>) {
    if let Some(pos) = placement(app, panel, anchor) {
        let _ = panel.set_position(pos);
    }
    HAD_FOCUS.store(false, Ordering::Relaxed);
    let shown = now_ms();
    SHOWN_AT.store(shown, Ordering::Relaxed);
    let _ = panel.show();
    let _ = panel.unminimize();
    let _ = panel.set_focus();

    let panel = panel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(BLUR_GRACE_MS + 50));
        // A no-op when the panel already is the foreground window.
        if SHOWN_AT.load(Ordering::Relaxed) == shown && panel.is_visible().unwrap_or(false) {
            let _ = panel.set_focus();
        }
        #[cfg(windows)]
        hide_on_outside_click(&panel, shown);
    });
}

/// Windows refuses focus to a window shown while the user is clicking in
/// another one, and the refocus above can be refused for the same reason. A
/// panel without focus gets no blur, so while it is not the foreground window
/// a click outside it hides it here instead. The foreground window is asked
/// from Windows: the focus state the window keeps can claim focus the panel
/// does not have, which kept this watch from ever running.
#[cfg(windows)]
fn hide_on_outside_click(panel: &WebviewWindow, shown: u64) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    let Ok(hwnd) = panel.hwnd() else {
        return;
    };
    let hwnd = hwnd.0 as isize;
    let pressed = || unsafe {
        [VK_LBUTTON, VK_RBUTTON]
            .iter()
            .any(|k| GetAsyncKeyState(i32::from(k.0)) as u16 & 0x8000 != 0)
    };
    // A button held when the watch starts is not a new click.
    let mut was_pressed = true;
    loop {
        if SHOWN_AT.load(Ordering::Relaxed) != shown || !panel.is_visible().unwrap_or(false) {
            return;
        }
        let down = pressed();
        if down && !was_pressed && !cursor_over(panel) {
            // In the foreground, the blur that follows this click hides it.
            let foreground = unsafe { GetForegroundWindow() }.0 as isize;
            if foreground != hwnd {
                log::info!("panel: hidden on a click outside it without focus");
                hide(panel);
                return;
            }
        }
        was_pressed = down;
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(windows)]
fn cursor_over(panel: &WebviewWindow) -> bool {
    let (Ok(cursor), Ok(pos), Ok(size)) = (
        panel.cursor_position(),
        panel.outer_position(),
        panel.outer_size(),
    ) else {
        return true;
    };
    let (x, y) = (cursor.x, cursor.y);
    x >= pos.x as f64
        && y >= pos.y as f64
        && x < pos.x as f64 + size.width as f64
        && y < pos.y as f64 + size.height as f64
}

/// Anchor point (center of the tray icon, else the cursor) in physical pixels.
fn anchor_point(app: &AppHandle, anchor: Option<Rect>) -> Option<(f64, f64)> {
    if let Some(rect) = anchor {
        let p: PhysicalPosition<f64> = rect.position.to_physical(1.0);
        let s: PhysicalSize<f64> = rect.size.to_physical(1.0);
        if s.width > 0.0 {
            return Some((p.x + s.width / 2.0, p.y + s.height / 2.0));
        }
    }
    app.cursor_position().ok().map(|p| (p.x, p.y))
}

fn placement(
    app: &AppHandle,
    panel: &WebviewWindow,
    anchor: Option<Rect>,
) -> Option<PhysicalPosition<i32>> {
    let size = panel.outer_size().ok()?;
    let point = anchor_point(app, anchor);
    let monitor = point
        .and_then(|(x, y)| app.monitor_from_point(x, y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten())?;

    let screen = Area {
        x: monitor.position().x,
        y: monitor.position().y,
        w: monitor.size().width as i32,
        h: monitor.size().height as i32,
    };
    let wa = monitor.work_area();
    let work = Area {
        x: wa.position.x,
        y: wa.position.y,
        w: wa.size.width as i32,
        h: wa.size.height as i32,
    };
    let margin = (MARGIN * monitor.scale_factor()).round() as i32;
    // Without an anchor, the usual tray corner.
    let point = point.unwrap_or(((work.x + work.w) as f64, (work.y + work.h) as f64));
    let (x, y) = panel_position(
        screen,
        work,
        point,
        (size.width as i32, size.height as i32),
        margin,
    );
    Some(PhysicalPosition::new(x, y))
}

#[derive(Debug, Clone, Copy)]
struct Area {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

#[derive(Debug, PartialEq)]
enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

/// Where the panel goes: against the taskbar, centered on the anchor, kept
/// inside the work area.
fn panel_position(
    screen: Area,
    work: Area,
    anchor: (f64, f64),
    size: (i32, i32),
    margin: i32,
) -> (i32, i32) {
    let gaps = [
        (Edge::Bottom, (screen.y + screen.h) - (work.y + work.h)),
        (Edge::Top, work.y - screen.y),
        (Edge::Left, work.x - screen.x),
        (Edge::Right, (screen.x + screen.w) - (work.x + work.w)),
    ];
    let edge = gaps
        .into_iter()
        .filter(|(_, gap)| *gap > 0)
        .max_by_key(|(_, gap)| *gap)
        .map(|(edge, _)| edge)
        // Auto-hidden taskbar: use the side the icon sits on.
        .unwrap_or(if anchor.1 < (screen.y + screen.h / 2) as f64 {
            Edge::Top
        } else {
            Edge::Bottom
        });

    let (w, h) = size;
    let max_x = (work.x + work.w - w - margin).max(work.x);
    let max_y = (work.y + work.h - h - margin).max(work.y);
    // A panel taller or wider than the work area minus the margins (a small
    // screen at a high scale) drops the margin instead of panicking in clamp.
    let min_x = (work.x + margin).min(max_x);
    let min_y = (work.y + margin).min(max_y);
    let centered_x = (anchor.0.round() as i32 - w / 2).clamp(min_x, max_x);
    let centered_y = (anchor.1.round() as i32 - h / 2).clamp(min_y, max_y);

    match edge {
        Edge::Bottom => (centered_x, max_y),
        Edge::Top => (centered_x, min_y),
        Edge::Left => (min_x, centered_y),
        Edge::Right => (max_x, centered_y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Area = Area {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    };
    const PANEL_SIZE: (i32, i32) = (380, 560);

    fn area(x: i32, y: i32, w: i32, h: i32) -> Area {
        Area { x, y, w, h }
    }

    #[test]
    fn bottom_taskbar_puts_panel_above_the_icon() {
        let work = area(0, 0, 1920, 1032);
        let pos = panel_position(SCREEN, work, (1700.0, 1056.0), PANEL_SIZE, 12);
        assert_eq!(pos, (1510, 1032 - 560 - 12));
    }

    #[test]
    fn panel_never_leaves_the_work_area() {
        let work = area(0, 0, 1920, 1032);
        let pos = panel_position(SCREEN, work, (1910.0, 1056.0), PANEL_SIZE, 12);
        assert_eq!(pos.0, 1920 - 380 - 12);
    }

    #[test]
    fn menu_bar_puts_panel_below() {
        let work = area(0, 25, 1920, 1055);
        let pos = panel_position(SCREEN, work, (1500.0, 12.0), PANEL_SIZE, 12);
        assert_eq!(pos, (1310, 37));
    }

    #[test]
    fn side_taskbars_are_supported() {
        let left = area(60, 0, 1860, 1080);
        assert_eq!(
            panel_position(SCREEN, left, (30.0, 900.0), PANEL_SIZE, 12),
            (72, 1080 - 560 - 12)
        );
        let right = area(0, 0, 1860, 1080);
        assert_eq!(
            panel_position(SCREEN, right, (1890.0, 300.0), PANEL_SIZE, 12),
            (1860 - 380 - 12, 20)
        );
    }

    #[test]
    fn hidden_taskbar_uses_the_icon_side() {
        let pos = panel_position(SCREEN, SCREEN, (1800.0, 1075.0), PANEL_SIZE, 12);
        assert_eq!(pos.1, 1080 - 560 - 12);
        let pos = panel_position(SCREEN, SCREEN, (1800.0, 5.0), PANEL_SIZE, 12);
        assert_eq!(pos.1, 12);
    }

    #[test]
    fn a_panel_taller_than_the_work_area_does_not_panic() {
        // 1366x768 at 125%: a 700 px panel in a 708 px work area.
        let screen = area(0, 0, 1366, 768);
        let work = area(0, 0, 1366, 708);
        let pos = panel_position(screen, work, (1300.0, 740.0), (475, 700), 15);
        assert_eq!(pos.1, 0);
        let left = area(60, 0, 1306, 768);
        let pos = panel_position(screen, left, (30.0, 400.0), (475, 760), 15);
        assert_eq!(pos, (75, 0));
    }
}
