//! What GPUI does not do for a tray panel, done on the native window.
//!
//! The material behind the panel: on macOS 26 and later the real Liquid
//! Glass (`NSGlassEffectView`) is put under GPUI's view; older macOS and
//! Windows get GPUI's own blurred background (vibrancy, Acrylic); Linux has
//! none and the panel paints an opaque ground. Placement is done here too,
//! in the system's own units: GPUI positions windows in logical pixels of
//! one display, which lands wrong across displays of different scales.

use gpui::{Window, WindowBackgroundAppearance};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::theme::Glass;

/// Gap between the panel and the taskbar or menu bar, in logical pixels.
pub const MARGIN: f64 = 12.0;

/// The material this system gives the panel.
pub fn glass() -> Glass {
    #[cfg(target_os = "macos")]
    {
        if mac::liquid_glass() {
            Glass::Liquid
        } else {
            Glass::Native
        }
    }
    #[cfg(windows)]
    {
        Glass::Native
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Glass::None
    }
}

/// The background GPUI is asked for. Liquid Glass, and the system backdrop
/// of Windows 11, are added under a transparent window instead; GPUI's own
/// blur covers older Windows and macOS.
pub fn background() -> WindowBackgroundAppearance {
    #[cfg(windows)]
    if win::system_backdrop() {
        return WindowBackgroundAppearance::Transparent;
    }
    match glass() {
        Glass::Native => WindowBackgroundAppearance::Blurred,
        Glass::Liquid | Glass::None => WindowBackgroundAppearance::Transparent,
    }
}

/// Where the user clicked to open the panel, in the system's screen units
/// (physical pixels on Windows, points on macOS).
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
#[derive(Debug, Clone, Copy)]
pub struct Anchor {
    pub x: f64,
    pub y: f64,
}

/// A window's native handle (`HWND`, or the `NSView` on macOS), copied out
/// of GPUI.
///
/// What is done to it here makes the system call back into GPUI at once
/// (a move, a focus change), so it must run while GPUI holds nothing: from
/// a task between two awaits, never inside an update of the app or of a
/// window. Main thread only.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeWindow(usize);

impl NativeWindow {
    pub fn of(window: &Window) -> Option<Self> {
        match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            #[cfg(windows)]
            RawWindowHandle::Win32(h) => Some(NativeWindow(h.hwnd.get() as usize)),
            #[cfg(target_os = "macos")]
            RawWindowHandle::AppKit(h) => Some(NativeWindow(h.ns_view.as_ptr() as usize)),
            _ => None,
        }
    }

    /// Rounds the window and lays the material under it. `radius` is in
    /// logical pixels.
    pub fn prepare(self, radius: f64) {
        #[cfg(windows)]
        win::prepare(self.0 as isize);
        #[cfg(target_os = "macos")]
        // SAFETY: a live NSView, used on the main thread (see the type).
        unsafe {
            mac::prepare(self.0 as *mut std::ffi::c_void, radius)
        };
        let _ = radius;
    }

    /// Puts the panel against the taskbar or the menu bar, near `anchor`
    /// (the tray icon, else the cursor), inside the work area. `size` is
    /// logical.
    pub fn place(self, anchor: Option<Anchor>, size: (f64, f64)) {
        #[cfg(windows)]
        win::place(self.0 as isize, anchor, size);
        #[cfg(target_os = "macos")]
        // SAFETY: as in `prepare`.
        unsafe {
            mac::place(self.0 as *mut std::ffi::c_void, anchor, size)
        };
        let _ = (anchor, size);
    }

    /// Shows the window in front, with the keyboard.
    pub fn show(self) {
        #[cfg(windows)]
        win::show(self.0 as isize);
        #[cfg(target_os = "macos")]
        // SAFETY: as in `prepare`.
        unsafe {
            mac::show(self.0 as *mut std::ffi::c_void)
        };
    }

    #[cfg(windows)]
    pub fn hwnd(self) -> isize {
        self.0 as isize
    }
}

#[cfg(windows)]
pub use win::{click_outside, is_foreground};

/// Where the pointer is, as an anchor when the tray gives none.
pub fn cursor() -> Option<Anchor> {
    #[cfg(windows)]
    {
        win::cursor()
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: a class method returning a plain struct.
        Some(unsafe { mac::cursor() })
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        None
    }
}

/// Makes the app a menu bar accessory: no Dock icon, no app menu.
pub fn accessory_app() {
    #[cfg(target_os = "macos")]
    // SAFETY: NSApp exists once GPUI runs, and this runs on the main thread.
    unsafe {
        mac::accessory_app()
    };
}

/// A rectangle in screen units.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
#[derive(Debug, PartialEq)]
enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

/// Where the panel goes: against the taskbar, centered on the anchor, kept
/// inside the work area. Screen units with y growing downwards.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub fn panel_position(
    screen: Area,
    work: Area,
    anchor: (f64, f64),
    size: (f64, f64),
    margin: f64,
) -> (f64, f64) {
    let gaps = [
        (Edge::Bottom, (screen.y + screen.h) - (work.y + work.h)),
        (Edge::Top, work.y - screen.y),
        (Edge::Left, work.x - screen.x),
        (Edge::Right, (screen.x + screen.w) - (work.x + work.w)),
    ];
    let edge = gaps
        .into_iter()
        .filter(|(_, gap)| *gap > 0.0)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(edge, _)| edge)
        // Auto-hidden taskbar: use the side the icon sits on.
        .unwrap_or(if anchor.1 < screen.y + screen.h / 2.0 {
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
    let centered_x = (anchor.0 - w / 2.0).round().clamp(min_x, max_x);
    let centered_y = (anchor.1 - h / 2.0).round().clamp(min_y, max_y);

    match edge {
        Edge::Bottom => (centered_x, max_y),
        Edge::Top => (centered_x, min_y),
        Edge::Left => (min_x, centered_y),
        Edge::Right => (max_x, centered_y),
    }
}

#[cfg(windows)]
mod win {
    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::Graphics::Dwm::{
        DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMSBT_TRANSIENTWINDOW,
        DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
        DWM_SYSTEMBACKDROP_TYPE, DWM_WINDOW_CORNER_PREFERENCE,
    };
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        MONITOR_DEFAULTTOPRIMARY,
    };
    use windows::Win32::UI::Controls::MARGINS;
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, GetForegroundWindow, GetWindowRect, SetForegroundWindow, SetWindowPos,
        ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE, SW_SHOW,
    };

    use super::{panel_position, Anchor, Area, MARGIN};

    fn hwnd(id: isize) -> HWND {
        HWND(id as *mut _)
    }

    /// Windows 11 22H2 and later draw Acrylic themselves under a window
    /// (`DWMSBT_TRANSIENTWINDOW`), with a stronger blur than the older
    /// composition attribute GPUI uses.
    pub fn system_backdrop() -> bool {
        static BUILD: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
        let build = *BUILD.get_or_init(|| {
            winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
                .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
                .and_then(|k| k.get_value::<String, _>("CurrentBuildNumber"))
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0)
        });
        build >= 22621
    }

    pub fn prepare(id: isize) {
        let pref: DWM_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND;
        // SAFETY: a live window of this process, and values of the sizes given.
        unsafe {
            let _ = DwmSetWindowAttribute(
                hwnd(id),
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &pref as *const _ as *const _,
                std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
            );
            if system_backdrop() {
                let margins = MARGINS {
                    cxLeftWidth: -1,
                    cxRightWidth: -1,
                    cyTopHeight: -1,
                    cyBottomHeight: -1,
                };
                let _ = DwmExtendFrameIntoClientArea(hwnd(id), &margins);
                let backdrop: DWM_SYSTEMBACKDROP_TYPE = DWMSBT_TRANSIENTWINDOW;
                let _ = DwmSetWindowAttribute(
                    hwnd(id),
                    DWMWA_SYSTEMBACKDROP_TYPE,
                    &backdrop as *const _ as *const _,
                    std::mem::size_of::<DWM_SYSTEMBACKDROP_TYPE>() as u32,
                );
            }
        }
    }

    fn area(r: RECT) -> Area {
        Area {
            x: f64::from(r.left),
            y: f64::from(r.top),
            w: f64::from(r.right - r.left),
            h: f64::from(r.bottom - r.top),
        }
    }

    pub fn place(id: isize, anchor: Option<Anchor>, size: (f64, f64)) {
        let point = anchor.map(|a| POINT {
            x: a.x.round() as i32,
            y: a.y.round() as i32,
        });
        // SAFETY: plain Win32 queries and a move of this process's window.
        unsafe {
            let monitor = match point {
                Some(p) => MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST),
                None => MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY),
            };
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if !GetMonitorInfoW(monitor, &mut info).as_bool() {
                return;
            }
            let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            let scale = f64::from(dpi_x) / 96.0;
            let screen = area(info.rcMonitor);
            let work = area(info.rcWork);
            let w = (size.0 * scale).round();
            let h = (size.1 * scale).round();
            // Without an anchor, the usual tray corner.
            let at = anchor.map_or((work.x + work.w, work.y + work.h), |a| (a.x, a.y));
            let (x, y) = panel_position(screen, work, at, (w, h), (MARGIN * scale).round());
            let _ = SetWindowPos(
                hwnd(id),
                Some(HWND_TOPMOST),
                x as i32,
                y as i32,
                w as i32,
                h as i32,
                SWP_NOACTIVATE,
            );
        }
    }

    pub fn show(id: isize) {
        // SAFETY: a live window of this process.
        unsafe {
            let _ = ShowWindow(hwnd(id), SW_SHOW);
            let _ = SetForegroundWindow(hwnd(id));
        }
    }

    pub fn cursor() -> Option<Anchor> {
        let mut p = POINT::default();
        // SAFETY: writes the position into `p`.
        unsafe { GetCursorPos(&mut p) }.ok()?;
        Some(Anchor {
            x: f64::from(p.x),
            y: f64::from(p.y),
        })
    }

    pub fn is_foreground(id: isize) -> bool {
        // SAFETY: a plain query.
        unsafe { GetForegroundWindow() }.0 as isize == id
    }

    /// Whether a mouse button is down with the pointer outside the window.
    pub fn click_outside(id: isize) -> bool {
        // SAFETY: plain queries on this process's window.
        unsafe {
            let pressed = [VK_LBUTTON, VK_RBUTTON]
                .iter()
                .any(|k| GetAsyncKeyState(i32::from(k.0)) as u16 & 0x8000 != 0);
            if !pressed {
                return false;
            }
            let mut r = RECT::default();
            let mut p = POINT::default();
            if GetWindowRect(hwnd(id), &mut r).is_err() || GetCursorPos(&mut p).is_err() {
                return false;
            }
            !(p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom)
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;

    use objc2::msg_send;
    use objc2::rc::{Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::{NSPoint, NSRect};

    use super::{panel_position, Anchor, Area, MARGIN};

    /// `NSViewWidthSizable | NSViewHeightSizable`.
    const RESIZE_WITH_PARENT: usize = 2 | 16;
    /// `NSWindowBelow`.
    const BELOW: isize = -1;
    /// `NSApplicationActivationPolicyAccessory`.
    const ACCESSORY: isize = 1;

    fn glass_class() -> Option<&'static AnyClass> {
        AnyClass::get(c"NSGlassEffectView")
    }

    /// Liquid Glass came with macOS 26.
    pub fn liquid_glass() -> bool {
        glass_class().is_some()
    }

    unsafe fn window_of(view: *mut c_void) -> Option<Retained<AnyObject>> {
        let view = &*(view as *const AnyObject);
        msg_send![view, window]
    }

    /// Hides the window buttons GPUI's titled panel carries, rounds the
    /// content, and lays Liquid Glass under it where the system has it.
    pub unsafe fn prepare(view: *mut c_void, radius: f64) {
        let Some(window) = window_of(view) else {
            return;
        };
        for button in 0isize..3 {
            let b: Option<Retained<AnyObject>> = msg_send![&*window, standardWindowButton: button];
            if let Some(b) = b {
                let _: () = msg_send![&*b, setHidden: true];
            }
        }
        let content: Option<Retained<AnyObject>> = msg_send![&*window, contentView];
        let Some(content) = content else {
            return;
        };
        let _: () = msg_send![&*content, setWantsLayer: true];
        let layer: Option<Retained<AnyObject>> = msg_send![&*content, layer];
        if let Some(layer) = layer {
            let _: () = msg_send![&*layer, setCornerRadius: radius];
            let _: () = msg_send![&*layer, setMasksToBounds: true];
        }
        if let Some(cls) = glass_class() {
            let bounds: NSRect = msg_send![&*content, bounds];
            let glass: Allocated<AnyObject> = msg_send![cls, alloc];
            let glass: Retained<AnyObject> = msg_send![glass, initWithFrame: bounds];
            let _: () = msg_send![&*glass, setCornerRadius: radius];
            let _: () = msg_send![&*glass, setAutoresizingMask: RESIZE_WITH_PARENT];
            let _: () = msg_send![
                &*content,
                addSubview: &*glass,
                positioned: BELOW,
                relativeTo: Option::<&AnyObject>::None
            ];
        }
        // The shadow follows the new, rounded shape.
        let _: () = msg_send![&*window, invalidateShadow];
    }

    pub unsafe fn cursor() -> Anchor {
        let cls = AnyClass::get(c"NSEvent").expect("NSEvent");
        let p: NSPoint = msg_send![cls, mouseLocation];
        Anchor { x: p.x, y: p.y }
    }

    /// Under the menu bar, centered on the anchor (the status item), in
    /// Cocoa's coordinates: points, y growing upwards from the bottom of the
    /// main screen.
    pub unsafe fn place(view: *mut c_void, anchor: Option<Anchor>, size: (f64, f64)) {
        let Some(window) = window_of(view) else {
            return;
        };
        let at = anchor.unwrap_or_else(|| cursor());
        let screens_cls = AnyClass::get(c"NSScreen").expect("NSScreen");
        let screens: Retained<AnyObject> = msg_send![screens_cls, screens];
        let count: usize = msg_send![&*screens, count];
        let mut chosen: Option<(NSRect, NSRect)> = None;
        for i in 0..count {
            let screen: Retained<AnyObject> = msg_send![&*screens, objectAtIndex: i];
            let frame: NSRect = msg_send![&*screen, frame];
            let visible: NSRect = msg_send![&*screen, visibleFrame];
            let inside = at.x >= frame.origin.x
                && at.x <= frame.origin.x + frame.size.width
                && at.y >= frame.origin.y
                && at.y <= frame.origin.y + frame.size.height;
            if inside || chosen.is_none() {
                chosen = Some((frame, visible));
                if inside {
                    break;
                }
            }
        }
        let Some((frame, visible)) = chosen else {
            return;
        };
        // Flip to y growing downwards for the shared placement rule.
        let top = frame.origin.y + frame.size.height;
        let flip = |r: NSRect| Area {
            x: r.origin.x,
            y: top - (r.origin.y + r.size.height),
            w: r.size.width,
            h: r.size.height,
        };
        let anchor = (at.x, top - at.y);
        let (x, y) = panel_position(flip(frame), flip(visible), anchor, size, MARGIN / 2.0);
        let origin = NSPoint::new(x, top - y - size.1);
        let _: () = msg_send![&*window, setFrameOrigin: origin];
    }

    pub unsafe fn show(view: *mut c_void) {
        let Some(window) = window_of(view) else {
            return;
        };
        let cls = AnyClass::get(c"NSApplication").expect("NSApplication");
        let app: Retained<AnyObject> = msg_send![cls, sharedApplication];
        let _: () = msg_send![&*app, activateIgnoringOtherApps: true];
        let _: () = msg_send![&*window, makeKeyAndOrderFront: Option::<&AnyObject>::None];
    }

    pub unsafe fn accessory_app() {
        let cls = AnyClass::get(c"NSApplication").expect("NSApplication");
        let app: Retained<AnyObject> = msg_send![cls, sharedApplication];
        let _: bool = msg_send![&*app, setActivationPolicy: ACCESSORY];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Area = Area {
        x: 0.0,
        y: 0.0,
        w: 1920.0,
        h: 1080.0,
    };
    const PANEL_SIZE: (f64, f64) = (380.0, 560.0);

    fn area(x: f64, y: f64, w: f64, h: f64) -> Area {
        Area { x, y, w, h }
    }

    #[test]
    fn bottom_taskbar_puts_panel_above_the_icon() {
        let work = area(0.0, 0.0, 1920.0, 1032.0);
        let pos = panel_position(SCREEN, work, (1700.0, 1056.0), PANEL_SIZE, 12.0);
        assert_eq!(pos, (1510.0, 1032.0 - 560.0 - 12.0));
    }

    #[test]
    fn panel_never_leaves_the_work_area() {
        let work = area(0.0, 0.0, 1920.0, 1032.0);
        let pos = panel_position(SCREEN, work, (1910.0, 1056.0), PANEL_SIZE, 12.0);
        assert_eq!(pos.0, 1920.0 - 380.0 - 12.0);
    }

    #[test]
    fn menu_bar_puts_panel_below() {
        let work = area(0.0, 25.0, 1920.0, 1055.0);
        let pos = panel_position(SCREEN, work, (1500.0, 12.0), PANEL_SIZE, 12.0);
        assert_eq!(pos, (1310.0, 37.0));
    }

    #[test]
    fn side_taskbars_are_supported() {
        let left = area(60.0, 0.0, 1860.0, 1080.0);
        assert_eq!(
            panel_position(SCREEN, left, (30.0, 900.0), PANEL_SIZE, 12.0),
            (72.0, 1080.0 - 560.0 - 12.0)
        );
        let right = area(0.0, 0.0, 1860.0, 1080.0);
        assert_eq!(
            panel_position(SCREEN, right, (1890.0, 300.0), PANEL_SIZE, 12.0),
            (1860.0 - 380.0 - 12.0, 20.0)
        );
    }

    #[test]
    fn hidden_taskbar_uses_the_icon_side() {
        let pos = panel_position(SCREEN, SCREEN, (1800.0, 1075.0), PANEL_SIZE, 12.0);
        assert_eq!(pos.1, 1080.0 - 560.0 - 12.0);
        let pos = panel_position(SCREEN, SCREEN, (1800.0, 5.0), PANEL_SIZE, 12.0);
        assert_eq!(pos.1, 12.0);
    }

    #[test]
    fn a_panel_taller_than_the_work_area_does_not_panic() {
        // 1366x768 at 125%: a 700 px panel in a 708 px work area.
        let screen = area(0.0, 0.0, 1366.0, 768.0);
        let work = area(0.0, 0.0, 1366.0, 708.0);
        let pos = panel_position(screen, work, (1300.0, 740.0), (475.0, 700.0), 15.0);
        assert_eq!(pos.1, 0.0);
        let left = area(60.0, 0.0, 1306.0, 768.0);
        let pos = panel_position(screen, left, (30.0, 400.0), (475.0, 760.0), 15.0);
        assert_eq!(pos, (75.0, 0.0));
    }
}
