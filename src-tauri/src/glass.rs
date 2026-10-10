//! The material behind the panel.
//!
//! The panel window is transparent (`tauri.conf.json`) and the system draws
//! the glass under the page: on macOS 26 and later the real Liquid Glass
//! (`NSGlassEffectView`), on older macOS the `popover` vibrancy, on Windows
//! Acrylic, both set by `windowEffects`. Linux has none, so the page paints
//! an opaque ground. The page learns which one it sits on from
//! `window.__AUDIO_MIRROR_GLASS__`, set before its scripts run, and tunes
//! its tint to it.

use tauri::plugin::TauriPlugin;
use tauri::{Runtime, WebviewWindow};

/// Corner radius of the panel on macOS, matched by `--radius-window` in
/// `ui/styles.css`.
#[cfg(target_os = "macos")]
const RADIUS: f64 = 18.0;

/// What the page sits on: `liquid`, `native` (vibrancy or Acrylic) or `none`.
pub fn kind() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        if mac::available() {
            "liquid"
        } else {
            "native"
        }
    }
    #[cfg(windows)]
    {
        "native"
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        "none"
    }
}

/// Hands the material to every webview before its scripts run.
pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    tauri::plugin::Builder::new("glass")
        .js_init_script(format!("window.__AUDIO_MIRROR_GLASS__ = \"{}\";", kind()))
        .build()
}

/// Puts Liquid Glass under the panel where the system has it; elsewhere the
/// window keeps the effect `tauri.conf.json` gave it.
pub fn apply<R: Runtime>(panel: &WebviewWindow<R>) {
    #[cfg(target_os = "macos")]
    {
        if !mac::available() {
            return;
        }
        // The vibrancy view would sit over the glass: drop it first.
        let _ = panel.set_effects(None);
        let window = panel.clone();
        let _ = panel.run_on_main_thread(move || {
            if let Ok(ns_window) = window.ns_window() {
                // SAFETY: the pointer is the panel's live NSWindow, used on
                // the main thread.
                unsafe { mac::insert_glass(ns_window, RADIUS) };
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = panel;
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;

    use objc2::msg_send;
    use objc2::rc::{Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::NSRect;

    /// `NSViewWidthSizable | NSViewHeightSizable`.
    const RESIZE_WITH_PARENT: usize = 2 | 16;
    /// `NSWindowBelow`.
    const BELOW: isize = -1;

    fn class() -> Option<&'static AnyClass> {
        AnyClass::get(c"NSGlassEffectView")
    }

    /// Liquid Glass came with macOS 26.
    pub fn available() -> bool {
        class().is_some()
    }

    /// Adds an `NSGlassEffectView` filling the window's content view, under
    /// the webview.
    ///
    /// # Safety
    /// `ns_window` must be a live `NSWindow`, and this must run on the main
    /// thread.
    pub unsafe fn insert_glass(ns_window: *mut c_void, radius: f64) {
        let Some(cls) = class() else {
            return;
        };
        let window = &*(ns_window as *const AnyObject);
        let content: Option<Retained<AnyObject>> = msg_send![window, contentView];
        let Some(content) = content else {
            return;
        };
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
        // The shadow follows the new, rounded shape.
        let _: () = msg_send![window, invalidateShadow];
    }
}
