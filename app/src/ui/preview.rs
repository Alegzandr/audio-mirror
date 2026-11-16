//! Draws the panel and the splash into pictures with Slint's software
//! renderer, no window and no engine, to look at the layout:
//! `cargo test --manifest-path app/Cargo.toml preview -- --ignored`, then
//! open `target/preview-*.png`. The system glass is not there; a flat
//! backdrop stands in for it.

use std::cell::RefCell;
use std::rc::Rc;

use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Platform as SlintPlatform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, ModelRc, PhysicalSize, VecModel};

use super::{MenuEntry, OtherRow, OutputRow, Panel, Platform, Splash, Texts};

struct Headless(Rc<MinimalSoftwareWindow>);

impl SlintPlatform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

const SCALE: f32 = 2.0;
/// Stands in for the wallpaper seen through the glass.
const BACKDROP: [u8; 3] = [168, 176, 190];

fn render(window: &MinimalSoftwareWindow, size: (u32, u32), path: &str) {
    let (w, h) = (
        (size.0 as f32 * SCALE) as u32,
        (size.1 as f32 * SCALE) as u32,
    );
    window.set_size(PhysicalSize::new(w, h));
    let mut buffer = vec![PremultipliedRgbaColor::default(); (w * h) as usize];
    window.request_redraw();
    window.draw_if_needed(|renderer| {
        renderer.render(&mut buffer, w as usize);
    });
    let mut out = image::RgbaImage::new(w, h);
    for (px, c) in out.pixels_mut().zip(&buffer) {
        let a = u32::from(c.alpha);
        let mix = |v: u8, b: u8| (u32::from(v) + u32::from(b) * (255 - a) / 255).min(255) as u8;
        *px = image::Rgba([
            mix(c.red, BACKDROP[0]),
            mix(c.green, BACKDROP[1]),
            mix(c.blue, BACKDROP[2]),
            255,
        ]);
    }
    out.save(path).expect("preview png");
}

fn text(s: &str) -> slint::SharedString {
    s.into()
}

fn row(id: &str, name: &str, state: &str, fader: f32, muted: bool, db: &str) -> OutputRow {
    OutputRow {
        id: text(id),
        name: text(name),
        state: text(state),
        error: false,
        detail: text(""),
        fader,
        muted,
        db: text(db),
        can_rename: true,
    }
}

#[test]
#[ignore = "writes pictures to look at"]
fn preview() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Headless(window.clone()))).expect("platform");
    window.dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: SCALE,
    });
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/target");

    let ui = Panel::new().expect("panel");
    let platform = ui.global::<Platform>();
    platform.set_glass(1);
    platform.set_radius(8.0);
    platform.set_font_family(text("Inter Variable"));
    ui.set_texts(Texts {
        source: text("Source"),
        outputs: text("Outputs"),
        edit: text("Edit"),
        done: text("Done"),
        rename: text("Rename"),
        remove: text("Remove"),
        add: text("Add"),
        show: text("Show"),
        hide: text("Hide"),
        autostart: text("Start with system"),
        quit: text("Quit"),
        update: text("Update"),
        version: text("v0.6.0"),
    });
    ui.set_run_text(text("1 of 2 playing"));
    ui.set_source_label(text("Default output (Speakers) (default)"));
    ui.set_source_level(0.62);
    ui.set_source_db(text("\u{2212}22.8 dB"));
    let mut failing = row(
        "b",
        "Headphones (Arctis GameBuds)",
        "Unavailable",
        0.4,
        false,
        "\u{2212}31.2 dB",
    );
    failing.error = true;
    failing.detail = text("Device disconnected. Retrying.");
    ui.set_outputs(ModelRc::new(VecModel::from(vec![
        row(
            "a",
            "CABLE Input (VB-Audio Virtual Cable)",
            "Playing",
            0.8,
            false,
            "\u{2212}4.1 dB",
        ),
        failing,
    ])));
    ui.set_levels(ModelRc::new(VecModel::from(vec![0.55f32, 0.0])));
    ui.set_others(ModelRc::new(VecModel::from(vec![
        OtherRow {
            id: text("c"),
            name: text("Speakers (Realtek Audio)"),
        },
        OtherRow {
            id: text("d"),
            name: text("LG HDR 4K (NVIDIA High Definition Audio)"),
        },
    ])));
    ui.set_others_label(text("2 other devices"));
    ui.set_others_open(true);
    ui.set_autostart(true);
    ui.show().expect("show");
    render(&window, (380, 560), &format!("{dir}/preview-panel.png"));

    // Dragging the first volume capsule reports the position under the
    // pointer, and the release saves it.
    let faders = Rc::new(RefCell::new(Vec::new()));
    let seen = faders.clone();
    ui.on_fader(move |id, value, persist| seen.borrow_mut().push((id.to_string(), value, persist)));
    let at = |x: f32, y: f32| LogicalPosition::new(x, y);
    window.dispatch_event(WindowEvent::PointerPressed {
        position: at(40.0, 251.0),
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerMoved {
        position: at(135.0, 251.0),
    });
    window.dispatch_event(WindowEvent::PointerMoved {
        position: at(400.0, 251.0),
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position: at(400.0, 251.0),
        button: PointerEventButton::Left,
    });
    {
        let got = faders.borrow();
        assert!(got.len() >= 3, "fader events: {got:?}");
        let (id, first, persist) = &got[0];
        assert_eq!((id.as_str(), *persist), ("a", false));
        assert!(*first < 0.01, "pressed at the start: {first}");
        let (_, middle, _) = &got[1];
        assert!(*middle > 0.3 && *middle < 0.7, "dragged midway: {got:?}");
        let (_, last, persist) = got.last().unwrap();
        assert!(
            *persist && (*last - 1.0).abs() < 1e-6,
            "released past the end: {got:?}"
        );
    }

    // The mute button, then Escape on the focused panel.
    let mutes = Rc::new(RefCell::new(Vec::new()));
    let seen = mutes.clone();
    ui.on_toggle_mute(move |id| seen.borrow_mut().push(id.to_string()));
    let hides = Rc::new(RefCell::new(0));
    let seen = hides.clone();
    ui.on_hide_panel(move || *seen.borrow_mut() += 1);
    window.dispatch_event(WindowEvent::PointerPressed {
        position: at(340.0, 255.0),
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position: at(340.0, 255.0),
        button: PointerEventButton::Left,
    });
    assert_eq!(*mutes.borrow(), ["a"]);
    ui.invoke_take_focus();
    window.dispatch_event(WindowEvent::KeyPressed {
        text: slint::platform::Key::Escape.into(),
    });
    assert_eq!(*hides.borrow(), 1);

    // The source menu, open.
    ui.set_menu(ModelRc::new(VecModel::from(vec![
        MenuEntry {
            id: text("desktop"),
            label: text("Default output (Speakers) (default)"),
            header: false,
            checked: true,
        },
        MenuEntry {
            id: text(""),
            label: text("Output devices"),
            header: true,
            checked: false,
        },
        MenuEntry {
            id: text("output:a"),
            label: text("Speakers (Realtek Audio)"),
            header: false,
            checked: false,
        },
    ])));
    ui.set_menu_content(26.0 * 2.0 + 24.0);
    ui.set_menu_current(0.0);
    ui.set_menu_index(2);
    ui.set_menu_open(true);
    render(&window, (380, 560), &format!("{dir}/preview-menu.png"));
    ui.set_menu_open(false);

    // Edit mode, one output being renamed.
    ui.set_editing(true);
    ui.set_renaming_id(text("a"));
    ui.set_rename_text(text("CABLE Input"));
    ui.set_rename_suffix(text("(VB-Audio Virtual Cable)"));
    render(&window, (380, 560), &format!("{dir}/preview-edit.png"));
    ui.hide().expect("hide");

    let splash = Splash::new().expect("splash");
    splash.set_text(text("Downloading update, 42%"));
    splash.set_percent(42);
    splash.show().expect("show");
    render(&window, (300, 110), &format!("{dir}/preview-splash.png"));
}
