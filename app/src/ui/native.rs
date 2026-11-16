//! What Slint does not do for a tray panel, done on the native window.
//!
//! The material behind the panel: on macOS 26 and later the real Liquid
//! Glass (`NSGlassEffectView`), vibrancy before; Acrylic on Windows; Linux
//! has none and the panel paints an opaque ground. Placement is done here
//! too, in the system's own units, which stays right across displays of
//! different scales.

use raw_window_handle::HasWindowHandle;
#[cfg(any(windows, target_os = "macos"))]
use raw_window_handle::RawWindowHandle;

/// What the panel sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glass {
    /// `NSGlassEffectView`, macOS 26 and later: it carries its own tint.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Liquid,
    /// Vibrancy (older macOS) or Acrylic (Windows).
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    Native,
    /// No system material (Linux): the tint becomes an opaque ground.
    #[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
    None,
}

impl Glass {
    /// The value `Platform.glass` takes in `panel.slint`.
    pub fn code(self) -> i32 {
        match self {
            Glass::Liquid => 0,
            Glass::Native => 1,
            Glass::None => 2,
        }
    }
}

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

/// Where the user clicked to open the panel, in the system's screen units
/// (physical pixels on Windows, points on macOS).
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
#[derive(Debug, Clone, Copy)]
pub struct Anchor {
    pub x: f64,
    pub y: f64,
}

/// A window's native handle (`HWND`, or the `NSView` on macOS), copied out
/// of Slint once the window exists. Main thread only.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeWindow(usize);

impl NativeWindow {
    pub fn of(window: &slint::Window) -> Option<Self> {
        let handle = window.window_handle();
        match HasWindowHandle::window_handle(&handle).ok()?.as_raw() {
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
pub use win::{click_outside, is_foreground, owner, system_backdrop};

/// Where a window of `size` (logical) is centered on the main display, in
/// physical pixels, where the system says so before the window exists.
pub fn centered(size: (f64, f64)) -> Option<(i32, i32)> {
    #[cfg(windows)]
    {
        win::centered(size)
    }
    #[cfg(not(windows))]
    {
        let _ = size;
        None
    }
}

/// Makes the system set up OLE on this thread, in the single-threaded
/// apartment winit needs for its windows. The audio engine asks for the
/// multithreaded one on every thread it touches; done first, this keeps
/// that request from taking the main thread.
pub fn init_ui_thread() {
    #[cfg(windows)]
    // SAFETY: called once, on the main thread, before any window.
    unsafe {
        let _ = windows::Win32::System::Ole::OleInitialize(None);
    }
}

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
    // SAFETY: NSApp exists once the event loop runs, and this runs on the main thread.
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

/// An auto-hidden taskbar leaves the work area as large as the screen, so
/// a panel placed in it ends under the taskbar once that slides back in.
/// When the work area covers the screen, this takes the taskbar's own
/// rectangle (`taskbar`, where it shows) out of it, on the side it sits.
#[cfg(any(windows, test))]
pub fn reserve_taskbar(screen: Area, work: Area, taskbar: Area) -> Area {
    if work != screen {
        return work;
    }
    let (sx2, sy2) = (screen.x + screen.w, screen.y + screen.h);
    let (tx2, ty2) = (taskbar.x + taskbar.w, taskbar.y + taskbar.h);
    // Not on this screen.
    if taskbar.x >= sx2 || tx2 <= screen.x || taskbar.y >= sy2 || ty2 <= screen.y {
        return work;
    }
    let horizontal = taskbar.w >= taskbar.h;
    let mut w = work;
    if horizontal && ty2 >= sy2 {
        w.h = (taskbar.y - screen.y).max(0.0);
    } else if horizontal {
        w.y = ty2.min(sy2);
        w.h = sy2 - w.y;
    } else if tx2 >= sx2 {
        w.w = (taskbar.x - screen.x).max(0.0);
    } else {
        w.x = tx2.min(sx2);
        w.w = sx2 - w.x;
    }
    w
}

#[cfg(windows)]
mod win {
    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        MONITOR_DEFAULTTOPRIMARY,
    };
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, GetCursorPos, GetForegroundWindow, GetWindowRect, SetForegroundWindow,
        SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE, SW_SHOW, WINDOW_EX_STYLE, WS_POPUP,
    };

    use super::{panel_position, reserve_taskbar, Anchor, Area, MARGIN};

    /// Where the taskbar shows, even while it is auto-hidden.
    fn taskbar() -> Option<Area> {
        use windows::Win32::UI::Shell::{SHAppBarMessage, ABM_GETTASKBARPOS, APPBARDATA};
        let mut data = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            ..Default::default()
        };
        // SAFETY: `data` is a sized APPBARDATA the call fills in.
        let found = unsafe { SHAppBarMessage(ABM_GETTASKBARPOS, &mut data) };
        (found != 0).then(|| area(data.rc))
    }

    fn hwnd(id: isize) -> HWND {
        HWND(id as *mut _)
    }

    /// Windows 11 22H2 and later draw Acrylic themselves under a window
    /// (`DWMSBT_TRANSIENTWINDOW`), with a stronger blur than the older
    /// composition blur (`blur_behind`).
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

    /// The corners and the Acrylic of Windows 11 22H2 and later come with
    /// the window (`ui::window_attributes`); older systems get the
    /// composition blur here.
    pub fn prepare(id: isize) {
        if !system_backdrop() {
            blur_behind(id);
        }
    }

    /// A hidden window that owns the app's windows: an owned window is a
    /// pop-up with no taskbar button and no place in Alt+Tab, as a tray
    /// panel should be. Created once, on the main thread.
    pub fn owner() -> Option<isize> {
        thread_local! {
            static OWNER: Option<isize> = {
                // SAFETY: a plain hidden window of a system class.
                unsafe {
                    CreateWindowExW(
                        WINDOW_EX_STYLE::default(),
                        windows::core::w!("STATIC"),
                        windows::core::w!("Audio Mirror"),
                        WS_POPUP,
                        0,
                        0,
                        0,
                        0,
                        None,
                        None,
                        None,
                        None,
                    )
                }
                .ok()
                .map(|h| h.0 as isize)
            };
        }
        OWNER.with(|o| *o)
    }

    /// Before Windows 11 22H2: the composition blur behind the window, set
    /// through `SetWindowCompositionAttribute`, which user32 exports without
    /// declaring it.
    fn blur_behind(id: isize) {
        #[repr(C)]
        struct AccentPolicy {
            state: u32,
            flags: u32,
            gradient: u32,
            animation: u32,
        }
        #[repr(C)]
        struct CompositionData {
            attribute: u32,
            data: *mut std::ffi::c_void,
            size: usize,
        }
        type SetAttribute = unsafe extern "system" fn(HWND, *mut CompositionData) -> i32;
        /// `ACCENT_ENABLE_BLURBEHIND`.
        const BLUR_BEHIND: u32 = 3;
        /// `WCA_ACCENT_POLICY`.
        const ACCENT_POLICY: u32 = 19;
        // SAFETY: user32 is loaded in every GUI process; the function is
        // called with the layout it has had since Windows 10.
        unsafe {
            let Ok(user32) = GetModuleHandleW(windows::core::w!("user32.dll")) else {
                return;
            };
            let Some(f) =
                GetProcAddress(user32, windows::core::s!("SetWindowCompositionAttribute"))
            else {
                return;
            };
            let f: SetAttribute = std::mem::transmute(f);
            let mut policy = AccentPolicy {
                state: BLUR_BEHIND,
                flags: 0,
                gradient: 0,
                animation: 0,
            };
            let mut data = CompositionData {
                attribute: ACCENT_POLICY,
                data: &mut policy as *mut _ as *mut _,
                size: std::mem::size_of::<AccentPolicy>(),
            };
            f(hwnd(id), &mut data);
        }
    }

    /// Where a window of `size` (logical) sits centered on the main display,
    /// in physical pixels.
    pub fn centered(size: (f64, f64)) -> Option<(i32, i32)> {
        // SAFETY: plain Win32 queries.
        unsafe {
            let monitor = MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if !GetMonitorInfoW(monitor, &mut info).as_bool() {
                return None;
            }
            let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            let scale = f64::from(dpi_x) / 96.0;
            let work = area(info.rcWork);
            Some((
                (work.x + (work.w - size.0 * scale) / 2.0).round() as i32,
                (work.y + (work.h - size.1 * scale) / 2.0).round() as i32,
            ))
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
            let mut work = area(info.rcWork);
            if let Some(bar) = taskbar() {
                work = reserve_taskbar(screen, work, bar);
            }
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
    /// `NSVisualEffectMaterialPopover`.
    const POPOVER: isize = 6;
    /// `NSVisualEffectBlendingModeBehindWindow`.
    const BEHIND_WINDOW: isize = 0;
    /// `NSVisualEffectStateActive`.
    const ACTIVE: isize = 1;

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

    /// Rounds the window and lays the system material under the panel:
    /// Liquid Glass where the system has it, vibrancy before. winit needs
    /// its own view to stay the window's content view, so the material goes
    /// into the frame view, right under it.
    pub unsafe fn prepare(view: *mut c_void, radius: f64) {
        let Some(window) = window_of(view) else {
            return;
        };
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
        let frame_view: Option<Retained<AnyObject>> = msg_send![&*content, superview];
        let Some(frame_view) = frame_view else {
            return;
        };
        let bounds: NSRect = msg_send![&*content, frame];
        let material: Retained<AnyObject> = if let Some(cls) = glass_class() {
            let glass: Allocated<AnyObject> = msg_send![cls, alloc];
            let glass: Retained<AnyObject> = msg_send![glass, initWithFrame: bounds];
            let _: () = msg_send![&*glass, setCornerRadius: radius];
            glass
        } else {
            let Some(cls) = AnyClass::get(c"NSVisualEffectView") else {
                return;
            };
            let effect: Allocated<AnyObject> = msg_send![cls, alloc];
            let effect: Retained<AnyObject> = msg_send![effect, initWithFrame: bounds];
            let _: () = msg_send![&*effect, setMaterial: POPOVER];
            let _: () = msg_send![&*effect, setBlendingMode: BEHIND_WINDOW];
            let _: () = msg_send![&*effect, setState: ACTIVE];
            let _: () = msg_send![&*effect, setWantsLayer: true];
            let layer: Option<Retained<AnyObject>> = msg_send![&*effect, layer];
            if let Some(layer) = layer {
                let _: () = msg_send![&*layer, setCornerRadius: radius];
                let _: () = msg_send![&*layer, setMasksToBounds: true];
            }
            effect
        };
        let _: () = msg_send![&*material, setAutoresizingMask: RESIZE_WITH_PARENT];
        let _: () = msg_send![
            &*frame_view,
            addSubview: &*material,
            positioned: BELOW,
            relativeTo: &*content
        ];
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
    fn an_auto_hidden_taskbar_keeps_its_room() {
        // Bottom, 48 px: the panel ends above it, not under it.
        let bar = area(0.0, 1392.0, 2560.0, 48.0);
        let screen = area(0.0, 0.0, 2560.0, 1440.0);
        let work = reserve_taskbar(screen, screen, bar);
        assert_eq!(work, area(0.0, 0.0, 2560.0, 1392.0));
        let pos = panel_position(screen, work, (2350.0, 1416.0), PANEL_SIZE, 12.0);
        assert_eq!(pos.1, 1392.0 - 560.0 - 12.0);
        // Left and top.
        let left = reserve_taskbar(screen, screen, area(0.0, 0.0, 60.0, 1440.0));
        assert_eq!(left, area(60.0, 0.0, 2500.0, 1440.0));
        let top = reserve_taskbar(screen, screen, area(0.0, 0.0, 2560.0, 40.0));
        assert_eq!(top, area(0.0, 40.0, 2560.0, 1400.0));
        // A visible taskbar is already out of the work area; another
        // screen's taskbar is not this one's business.
        let shown = area(0.0, 0.0, 2560.0, 1392.0);
        assert_eq!(reserve_taskbar(screen, shown, bar), shown);
        let other = area(2560.0, 1392.0, 1920.0, 48.0);
        assert_eq!(reserve_taskbar(screen, screen, other), screen);
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
