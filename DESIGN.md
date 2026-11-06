---
name: Audio Mirror
description: A tray panel that mirrors one audio source to several outputs, drawn as a native macOS 27 Control Center module.
colors:
  tint: "rgba(246, 246, 248, 0.62)"
  tint-solid: "#ececee"
  platter: "rgba(255, 255, 255, 0.58)"
  platter-edge: "rgba(255, 255, 255, 0.9)"
  hairline: "rgba(0, 0, 0, 0.08)"
  fill: "rgba(0, 0, 0, 0.07)"
  fill-active: "rgba(0, 0, 0, 0.16)"
  track: "rgba(0, 0, 0, 0.1)"
  level: "#ffffff"
  meter: "rgba(0, 0, 0, 0.42)"
  ink: "rgba(0, 0, 0, 0.88)"
  ink-secondary: "rgba(0, 0, 0, 0.56)"
  ink-tertiary: "rgba(0, 0, 0, 0.42)"
  accent: "#007aff"
  accent-soft: "rgba(0, 122, 255, 0.16)"
  danger: "#e0352b"
  tint-dark: "rgba(30, 30, 32, 0.6)"
  tint-solid-dark: "#232325"
  platter-dark: "rgba(255, 255, 255, 0.08)"
  platter-edge-dark: "rgba(255, 255, 255, 0.16)"
  hairline-dark: "rgba(255, 255, 255, 0.08)"
  fill-dark: "rgba(255, 255, 255, 0.1)"
  track-dark: "rgba(255, 255, 255, 0.12)"
  level-dark: "rgba(255, 255, 255, 0.92)"
  meter-dark: "rgba(255, 255, 255, 0.5)"
  ink-dark: "rgba(255, 255, 255, 0.92)"
  ink-secondary-dark: "rgba(255, 255, 255, 0.6)"
  ink-tertiary-dark: "rgba(255, 255, 255, 0.44)"
  accent-dark: "#0a84ff"
  accent-soft-dark: "rgba(10, 132, 255, 0.24)"
  danger-dark: "#ff6961"
typography:
  title:
    fontFamily: "-apple-system, BlinkMacSystemFont, SF Pro Display, Segoe UI Variable Display, Segoe UI, system-ui, sans-serif"
    fontSize: "15px"
    fontWeight: 600
    letterSpacing: "-0.012em"
  group-heading:
    fontFamily: "-apple-system, BlinkMacSystemFont, SF Pro Text, Segoe UI Variable Text, Segoe UI, system-ui, Cantarell, Ubuntu, Noto Sans, sans-serif"
    fontSize: "12px"
    fontWeight: 600
  body:
    fontFamily: "-apple-system, BlinkMacSystemFont, SF Pro Text, Segoe UI Variable Text, Segoe UI, system-ui, Cantarell, Ubuntu, Noto Sans, sans-serif"
    fontSize: "13px"
    fontWeight: 400
    lineHeight: 1.35
    fontFeature: "tnum"
  device-name:
    fontFamily: "-apple-system, BlinkMacSystemFont, SF Pro Text, Segoe UI Variable Text, Segoe UI, system-ui, sans-serif"
    fontSize: "13px"
    fontWeight: 600
rounded:
  window: "18px"
  window-windows: "8px"
  platter: "16px"
  popup: "10px"
  field: "7px"
  capsule: "999px"
spacing:
  gutter: "12px"
  group-gap: "14px"
  platter-gap: "8px"
  platter-x: "14px"
components:
  platter:
    backgroundColor: "{colors.platter}"
    rounded: "{rounded.platter}"
    padding: "12px 12px 12px 14px"
  button:
    backgroundColor: "{colors.fill}"
    textColor: "{colors.ink}"
    rounded: "{rounded.capsule}"
    padding: "4px 12px"
    height: "28px"
  button-round:
    backgroundColor: "{colors.fill}"
    rounded: "{rounded.capsule}"
    size: "32px"
  button-round-pressed:
    backgroundColor: "{colors.accent}"
    textColor: "#ffffff"
  volume-capsule:
    backgroundColor: "{colors.track}"
    rounded: "{rounded.capsule}"
    height: "28px"
  level-meter:
    backgroundColor: "{colors.track}"
    height: "3px"
  switch:
    backgroundColor: "{colors.fill-active}"
    rounded: "{rounded.capsule}"
    width: "32px"
    height: "18px"
  switch-checked:
    backgroundColor: "{colors.accent}"
---

# Design System: Audio Mirror

## Overview

**North Star: "A Control Center module"**

Audio Mirror is drawn the way macOS 27 "Golden Gate" draws Control Center: Liquid Glass played straight, with the extra contrast Golden Gate added. The panel is a 380 by 560 window made of glass: the window itself is transparent and the system blurs what lies behind it (vibrancy on macOS, Acrylic on Windows, set up in `app/src/ui/native.rs`). A tint sits on that glass so text reads over any wallpaper; where no native glass exists (Linux), the tint becomes an opaque ground.

The panel is for operating: it lists only the outputs that play, each in its own platter with a thick volume capsule, as Control Center's Sound module. The other devices wait folded in one platter; renaming and removing live behind the Edit button. It follows the OS light or dark theme.

## Colors

Neutrals are translucent inks and fills, so they take the color of the glass under them. One system blue (`accent`) marks what is on or actionable: checked switches, the pressed mute button, header actions, Add and Show, focus halos, the rename field. One red marks errors and Remove. The volume fill is white in both themes, like Control Center's sliders.

## Typography

SF Pro on macOS (`.SystemUIFont`), left to the system. Everywhere else the bundled Inter variable font (`app/assets/fonts`, SIL OFL), since SF Pro's license keeps it on Apple platforms; Inter gets heavier weights (430/530/620) to make up for ClearType drawing it thinner. Slint sets no letter spacing, so SF Pro's tracking table is not applied to Inter. 13px body, 12px group headings and output states, 15px semibold title. Device names are semibold. Slint cannot turn on OpenType features, so numerals are the font's default ones; dB values sit right-aligned in a fixed width so they hold their place.

## Layout

Header (title, run-state sentence), scrolling middle, footer (Start with system switch, version, Quit). Groups stack with a 14px gap, platters within a group with 8px. The Source platter holds a pop-up button and the source meter. Each output platter has a head line (name, state) and a controls line: the volume capsule with the live meter just under it, the dB value (fixed width), and the round mute button. In edit mode the controls line becomes Rename and Remove capsules.

## Elevation & Depth

Liquid Glass: platters carry a hairline edge and a soft shadow; the window has a 1px rim. Nothing else lifts. Switch knobs carry a small physical shadow.

## Shapes

Window 18px (8px on Windows, which rounds undecorated windows itself), platters 16px, pop-up button 10px, fields 7px, everything you press is a capsule or a circle.

## Components

- **Buttons:** glass capsules on `fill`. As in AppKit they answer to the press, never to the hover: pressing steps to `fill-active`. The pointer stays an arrow everywhere (a text field gets the I-beam). Primary is solid accent. Plain is text only. Header actions (Edit, Done) are accent text.
- **Mute:** a 32px round button with a speaker glyph; pressed, it fills with the accent and the glyph gains a cross.
- **Volume capsule:** 28px, track color, a white fill to the volume with a speaker glyph at its start. Pressing anywhere on it sets the volume and dragging moves it; with the row focused, the arrow keys step it and <kbd>M</kbd> mutes. Muted, the fill turns gray.
- **Level meter:** a 3px capsule under each volume capsule (4px for the source), filled in `meter` gray from the live peak.
- **Switch:** macOS 26/27 shape, 38 by 22px with a 24 by 18px pill knob, accent when on.
- **Source pop-up:** a capsule button with up and down chevrons. It opens a menu drawn after macOS's pop-up menus, on every system: current item laid over the button, group headers, a check, the focused item filled with the accent, and full keyboard control.
- **Scroll bars:** none drawn; the list scrolls with the wheel and the trackpad, and a hairline under the header shows it has.
- **Glyphs:** a small set of SVG glyphs drawn in the SF Symbols manner (speaker with waves, speaker crossed, check, chevrons; `app/assets/icons`), tinted by the text color. Text still carries every state.

## Motion

SwiftUI's springs, `.smooth`, `.snappy` and `.bouncy` (0.5 s; bounce 0, 0.15, 0.3), as Slint's `spring()` easing (`app/src/ui/panel.slint`). The panel's header and footer settle in on `.snappy` when it opens (position and opacity only, the window and its glass appear at once since the system shows them), the source menu fades in, the switch knob crosses on `.bouncy`, the other devices unfold in a short stagger only when the user unfolds them. Pressed buttons and a held capsule do not shrink or swell.

## Glass

The window's glass comes from the system (`app/src/ui/native.rs`): `NSGlassEffectView` (Liquid Glass) on macOS 26 and later, vibrancy before, Acrylic on Windows (the system backdrop from Windows 11 22H2, the older composition blur before), none on Linux; the `Theme` global in `panel.slint` tunes the tint to the material it sits on. Slint cannot blur what lies under an element inside the window, so the source menu is a near-opaque tint rather than glass.

## Do's and Don'ts

- **Do** keep the glass readable: the tint never drops below what Golden Gate's tinted setting gives.
- **Do** write every state as text, even when a glyph shows it too.
- **Do** keep one accent, the system blue.
- **Don't** add decorative color, gradients, or extra glass layers inside platters.
- **Don't** add hover effects to anything that is not clickable.
- **Don't** write an em dash in UI copy.
