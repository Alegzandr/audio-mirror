//! The panel's controls, drawn after Control Center: glass capsules that
//! answer to the press and never to the hover, platters, the switch, the
//! level meter, and SwiftUI's springs for motion.

use std::time::Duration;

use gpui::{
    div, point, prelude::*, px, svg, Animation, AnimationExt, BoxShadow, Div, ElementId,
    FontWeight, Hsla, SharedString, Stateful, Styled,
};

use super::theme::{Theme, RADIUS_PLATTER, WEIGHTS};

/// SwiftUI's spring with a 0.5 s duration and the given bounce, as an
/// easing over `total` seconds: `.smooth` is bounce 0, `.snappy` 0.15,
/// `.bouncy` 0.3.
pub fn spring(bounce: f32, total: f32) -> impl Fn(f32) -> f32 {
    let omega = 2.0 * std::f32::consts::PI / 0.5;
    let zeta = 1.0 - bounce;
    move |delta: f32| {
        let t = delta * total;
        if delta >= 1.0 {
            return 1.0;
        }
        if zeta >= 1.0 {
            1.0 - (-omega * t).exp() * (1.0 + omega * t)
        } else {
            let wd = omega * (1.0 - zeta * zeta).sqrt();
            1.0 - (-zeta * omega * t).exp()
                * ((wd * t).cos() + (zeta * omega / wd) * (wd * t).sin())
        }
    }
}

/// `.snappy`: the panel settling in, the menu popping up.
pub fn snappy() -> (Duration, impl Fn(f32) -> f32) {
    (Duration::from_millis(725), spring(0.15, 0.725))
}

/// `.bouncy`: the switch knob crossing.
pub fn bouncy() -> (Duration, impl Fn(f32) -> f32) {
    (Duration::from_millis(840), spring(0.3, 0.84))
}

/// `.smooth`: the other devices unfolding.
pub fn smooth() -> (Duration, impl Fn(f32) -> f32) {
    (Duration::from_millis(760), spring(0.0, 0.76))
}

/// A linear animation of `duration` plus `delay`, handing `animate` the
/// spring's progress (which a bouncing spring takes past 1). GPUI wants
/// its easings inside 0..1, so the spring is applied here instead.
pub fn animation(
    (duration, easing): (Duration, impl Fn(f32) -> f32 + 'static),
    delay: f32,
) -> (Animation, impl Fn(f32) -> f32 + 'static) {
    let total = duration.as_secs_f32();
    let progress = move |delta: f32| {
        let t = ((delta * (total + delay) - delay) / total).clamp(0.0, 1.0);
        easing(t)
    };
    (
        Animation::new(duration + Duration::from_secs_f32(delay)),
        progress,
    )
}

/// A platter: a glass card with a lit rim, a hairline edge and a soft shadow.
pub fn platter(t: &Theme) -> Div {
    div()
        .relative()
        .bg(t.platter)
        .rounded(px(RADIUS_PLATTER))
        .border_1()
        .border_color(t.edge)
        .shadow(vec![
            BoxShadow {
                color: t.shadow,
                offset: point(px(0.), px(1.)),
                blur_radius: px(1.5),
                spread_radius: px(0.),
            },
            BoxShadow {
                color: t.shadow,
                offset: point(px(0.), px(3.)),
                blur_radius: px(10.),
                spread_radius: px(0.),
            },
        ])
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// A capsule on `fill`.
    Normal,
    /// Solid accent.
    Primary,
    /// Text only, secondary ink.
    Plain,
    /// Text only, in the accent, as macOS draws "Edit" and "Done".
    Link,
    /// A capsule on `fill`, in the danger color (Remove).
    Danger,
    /// A capsule on `fill`, in the accent (Add).
    Accent,
}

/// A capsule button. `small` is the 24 px variant with 12 px text.
pub fn button(
    t: &Theme,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    kind: ButtonKind,
    small: bool,
) -> Stateful<Div> {
    let (bg, fg) = match kind {
        ButtonKind::Normal => (Some(t.fill), t.text),
        ButtonKind::Primary => (Some(t.accent), t.accent_text),
        ButtonKind::Plain => (None, t.text_2),
        ButtonKind::Link => (None, t.accent),
        ButtonKind::Danger => (Some(t.fill), t.danger),
        ButtonKind::Accent => (Some(t.fill), t.accent),
    };
    let fill_active = t.fill_active;
    let mut b = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded_full()
        .whitespace_nowrap()
        .font_weight(FontWeight(WEIGHTS.medium))
        .text_color(fg)
        .cursor_default()
        .child(label.into());
    b = if small {
        b.h(px(24.)).px(px(10.)).text_size(px(12.))
    } else {
        b.h(px(28.)).px(px(12.))
    };
    if let Some(bg) = bg {
        b = b.bg(bg);
    }
    match kind {
        ButtonKind::Primary => b.active(move |s| s.opacity(0.9)),
        _ => b.active(move |s| s.bg(fill_active)),
    }
}

/// The round mute button: a speaker glyph, filled with the accent while it
/// holds the sound.
pub fn round_button(
    t: &Theme,
    id: impl Into<ElementId>,
    glyph: &'static str,
    pressed: bool,
) -> Stateful<Div> {
    let (bg, fg) = if pressed {
        (t.accent, t.accent_text)
    } else {
        (t.fill, t.text)
    };
    let fill_active = t.fill_active;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(32.))
        .rounded_full()
        .bg(bg)
        .cursor_default()
        .when(!pressed, |d| d.active(move |s| s.bg(fill_active)))
        .when(pressed, |d| d.active(|s| s.opacity(0.9)))
        .child(glyph_svg(glyph, 14., fg))
}

pub fn glyph_svg(path: &'static str, size: f32, color: Hsla) -> impl IntoElement {
    svg()
        .path(path)
        .size(px(size))
        .flex_none()
        .text_color(color)
}

/// The switch, macOS 26/27: a pill knob that crosses on `.bouncy`. `id`
/// changes with the state, which starts the knob's move.
pub fn switch(t: &Theme, id: impl Into<ElementId>, on: bool, animate: bool) -> Div {
    let (from, to) = if on { (2.0, 12.0) } else { (12.0, 2.0) };
    let knob = div()
        .absolute()
        .top(px(2.))
        .w(px(24.))
        .h(px(18.))
        .rounded_full()
        .bg(t.knob)
        .shadow(vec![BoxShadow {
            color: gpui::black().opacity(0.2),
            offset: point(px(0.), px(1.)),
            blur_radius: px(2.5),
            spread_radius: px(0.),
        }]);
    let knob = if animate {
        let (anim, progress) = animation(bouncy(), 0.0);
        knob.with_animation(id, anim, move |k, delta| {
            k.left(px(from + (to - from) * progress(delta)))
        })
        .into_any_element()
    } else {
        knob.left(px(to)).into_any_element()
    };
    div()
        .relative()
        .flex_none()
        .w(px(38.))
        .h(px(22.))
        .rounded_full()
        .bg(if on { t.accent } else { t.fill_active })
        .child(knob)
}

/// A level meter: a thin capsule filled from the left to `level` (0 to 1).
pub fn meter(t: &Theme, level: f32, height: f32, dimmed: bool) -> Div {
    div()
        .relative()
        .flex_1()
        .h(px(height))
        .rounded_full()
        .bg(t.track)
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .w(gpui::relative(level.clamp(0.0, 1.0)))
                .rounded_full()
                .bg(t.meter)
                .when(dimmed, |d| d.opacity(0.3)),
        )
}

#[cfg(test)]
mod tests {
    use super::spring;

    #[test]
    fn springs_start_at_zero_and_settle_at_one() {
        for bounce in [0.0, 0.15, 0.3] {
            let f = spring(bounce, 0.8);
            assert!(f(0.0).abs() < 1e-6);
            assert_eq!(f(1.0), 1.0);
            assert!((f(0.99) - 1.0).abs() < 0.01, "bounce {bounce} not settled");
        }
        // Only a bouncing spring overshoots.
        let smooth = spring(0.0, 0.8);
        assert!((0..100).all(|i| smooth(i as f32 / 100.0) <= 1.0 + 1e-4));
        let bouncy = spring(0.3, 0.8);
        assert!((0..100).any(|i| bouncy(i as f32 / 100.0) > 1.01));
    }
}
