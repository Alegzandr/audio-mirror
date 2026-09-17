---
name: Audio Mirror
description: A tray panel that mirrors one audio source to several outputs, drawn as a plain system settings pane.
colors:
  accent: "#1f5fbf"
  accent-hover: "#194f9f"
  accent-text: "#ffffff"
  accent-soft: "rgba(31, 95, 191, 0.12)"
  danger: "#b3261e"
  window: "#f3f3f2"
  surface: "#ffffff"
  surface-sunken: "#e9e9e7"
  line: "#e2e2e0"
  line-strong: "#c9c9c6"
  ink: "#1c1c1e"
  ink-secondary: "#5f5f63"
  ink-tertiary: "#707075"
  meter: "#8a8a90"
  control: "#ffffff"
  control-hover: "#f5f5f4"
  control-active: "#ebebea"
  knob: "#ffffff"
  accent-dark: "#4d8fe8"
  accent-hover-dark: "#6aa2ee"
  accent-text-dark: "#0d1a2c"
  accent-soft-dark: "rgba(77, 143, 232, 0.2)"
  danger-dark: "#ff7b72"
  window-dark: "#1c1c1e"
  surface-dark: "#262628"
  surface-sunken-dark: "#1b1b1d"
  line-dark: "#333336"
  line-strong-dark: "#4a4a4e"
  ink-dark: "#f2f2f3"
  ink-secondary-dark: "#a6a6ab"
  ink-tertiary-dark: "#909096"
  meter-dark: "#9d9da3"
  control-dark: "#313134"
  control-hover-dark: "#3a3a3d"
  control-active-dark: "#444448"
  knob-dark: "#f2f2f3"
typography:
  title:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI Variable Text, Segoe UI, system-ui, Cantarell, Ubuntu, Noto Sans, sans-serif"
    fontSize: "17px"
    fontWeight: 600
    lineHeight: 1.4
    letterSpacing: "-0.01em"
  group-heading:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI Variable Text, Segoe UI, system-ui, Cantarell, Ubuntu, Noto Sans, sans-serif"
    fontSize: "13px"
    fontWeight: 600
    lineHeight: 1.4
  body:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI Variable Text, Segoe UI, system-ui, Cantarell, Ubuntu, Noto Sans, sans-serif"
    fontSize: "13px"
    fontWeight: 400
    lineHeight: 1.4
    fontFeature: "tnum"
  label:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI Variable Text, Segoe UI, system-ui, Cantarell, Ubuntu, Noto Sans, sans-serif"
    fontSize: "13px"
    fontWeight: 500
    lineHeight: 1.4
rounded:
  sm: "6px"
  md: "10px"
  pill: "20px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "12px"
  gutter: "16px"
  group-gap: "18px"
  row-x: "14px"
components:
  button:
    backgroundColor: "{colors.control}"
    textColor: "{colors.ink}"
    typography: "{typography.label}"
    rounded: "{rounded.sm}"
    padding: "4px 12px"
    height: "28px"
  button-hover:
    backgroundColor: "{colors.control-hover}"
  button-active:
    backgroundColor: "{colors.control-active}"
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.accent-text}"
    typography: "{typography.label}"
    rounded: "{rounded.sm}"
    padding: "4px 12px"
    height: "28px"
  button-primary-hover:
    backgroundColor: "{colors.accent-hover}"
  button-plain:
    textColor: "{colors.ink-secondary}"
    typography: "{typography.label}"
    rounded: "{rounded.sm}"
    padding: "4px 8px"
    height: "28px"
  button-plain-hover:
    backgroundColor: "{colors.control-active}"
    textColor: "{colors.ink}"
  button-toggle-pressed:
    backgroundColor: "{colors.accent-soft}"
    textColor: "{colors.accent}"
    rounded: "{rounded.sm}"
    width: "68px"
  select:
    backgroundColor: "{colors.control}"
    textColor: "{colors.ink}"
    typography: "{typography.body}"
    rounded: "{rounded.sm}"
    padding: "5px 8px"
    height: "30px"
  switch:
    backgroundColor: "{colors.line-strong}"
    rounded: "{rounded.pill}"
    width: "36px"
    height: "20px"
  switch-checked:
    backgroundColor: "{colors.accent}"
  grouped-list:
    backgroundColor: "{colors.surface}"
    rounded: "{rounded.md}"
  list-row:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    padding: "10px 14px"
    height: "44px"
  level-meter:
    backgroundColor: "{colors.surface-sunken}"
    height: "2px"
---

# Design System: Audio Mirror

## Overview

**Creative North Star: "The Quiet Settings Pane"**

Audio Mirror is drawn as a system settings panel played straight, finished to the level of macOS Settings and Windows 11 Settings. It is a 380 by 560 tray panel: a fixed header with the app name and a plain run-state sentence, a scrolling middle made of titled groups, and a fixed footer. Everything a user needs to know (what is the source, how loud is it, which outputs play, at what volume) is written as text or drawn as a thin neutral bar. The window should look like it shipped with the OS.

Density is that of a native pane: 13px system text, 44px minimum rows, hairline separators inside softly raised grouped lists. Neutrals follow the OS light or dark theme through `prefers-color-scheme`. One sober blue does all the work color is allowed to do: the primary action, checked switches, the pressed Mute toggle, slider fill, and focus. One red marks errors. The world refuses the dark LED mixer look, segmented or colored meters, decorative cards, and icons of any kind.

**Key Characteristics:**
- System font stack only, one size for almost everything, weight carries hierarchy.
- Grouped rows on a slightly raised white (or dark gray) surface, separated by 1px hairlines.
- Native control vocabulary: switches, range sliders, a native select, text buttons.
- State is text ("Playing", "Muted", "Not connected"), never a lamp.
- One blue accent, one error red, everything else neutral and theme-following.

## Colors

A neutral, OS-following gray palette with a single restrained blue and a single red. Every token has a light value and a `-dark` counterpart applied under `prefers-color-scheme: dark`.

### Primary
- **Settings Blue** (`accent`, dark `accent-dark`): primary button ("Restart to update"), checked switch track, volume slider fill, focus ring, pressed Mute text. Hover deepens in light mode and lightens in dark mode (`accent-hover`).
- **Blue Wash** (`accent-soft`): background of the pressed Mute toggle and text selection. The only tinted fill in the system.
- **Error Red** (`danger`): run-state and output-state text in error, source error messages. Text only, never a fill or a dot.

### Neutral
- **Window Gray** (`window`): panel background behind groups, header and footer.
- **Raised White** (`surface`): grouped list background, lifted by a faint shadow.
- **Sunken Track** (`surface-sunken`): empty part of the level meter.
- **Hairline** (`line`): row separators, footer rule, header rule once content scrolls.
- **Strong Hairline** (`line-strong`): control borders, off switch track, unfilled slider track, scrollbar thumb.
- **Ink** (`ink`), **Secondary Ink** (`ink-secondary`), **Tertiary Ink** (`ink-tertiary`): primary text; state text, dB values and notes; version string, row detail text and muted slider fill.
- **Meter Gray** (`meter`): the filled part of every level meter.
- **Control fills** (`control`, `control-hover`, `control-active`, `knob`): button, select, and thumb surfaces and their pressed states.

### Named Rules
**The One Blue Rule.** Blue appears only on something the user can act on or has turned on: primary action, checked state, slider fill, focus. Never on headings, meters, or decoration.

**The Gray Meter Rule.** Level meters are gray at every level. No green-to-red ramps, no segments, no peak lamps; loudness is read from bar length and the dB number beside it.

**The Words For State Rule.** Run and output state are sentences in ink, secondary ink, or error red. No colored dots, badges, or LED-style indicators.

## Typography

**Body Font:** the system UI stack (-apple-system, Segoe UI Variable Text, Segoe UI, system-ui, then Linux defaults)

**Character:** The OS's own voice. Tabular numerals are on globally so dB values never jitter.

### Hierarchy
- **Title** (600, 17px, -0.01em): the app name in the header, once.
- **Group heading** (600, 13px): "Source" and "Outputs" above each grouped list, inset 2px.
- **Body** (400, 13px, 1.4): device names, state text, notes, dB values, footer text.
- **Label** (500, 13px): button text; pressed toggles go to 600.

### Named Rules
**The One Size Rule.** Everything except the app title is 13px. Hierarchy comes from weight (400/500/600) and ink level, not size.

**The No Eyebrow Rule.** A group has a heading and nothing else above its list: no subtitle, no eyebrow, no description line.

## Layout

A fixed three-row grid fills the window: header (16px top, 12px bottom), scrolling main, footer (10px vertical). The 16px side gutter is shared by all three. Main stacks groups with an 18px gap; inside a group, the heading sits 6px above its list. Rows have a 44px minimum height and 10px by 14px padding; source and output rows use 12px by 14px. Output rows are a stack: a head line (switch, name, state text) and, below it, controls (slider with meter under it, dB value, Mute) indented 46px so they align with the device name. Detail text uses the same indent. Disabled outputs hide their controls. The dB column is a fixed 8ch, right-aligned. The header gains a hairline only after the list scrolls.

## Elevation & Depth

Nearly flat. Depth is one step: grouped lists sit on the window gray with a faint ambient shadow, like a native settings card. Switch knobs and slider thumbs carry a small physical shadow. Nothing else lifts, and hover never adds a shadow.

### Shadow Vocabulary
- **Group lift** (`box-shadow: 0 1px 2px rgba(0,0,0,0.06), 0 1px 1px rgba(0,0,0,0.03)`; dark `0 1px 2px rgba(0,0,0,0.3)`): grouped lists only.
- **Knob** (`box-shadow: 0 1px 2px rgba(0,0,0,0.25)`): switch knob.
- **Thumb** (`box-shadow: 0 1px 3px rgba(0,0,0,0.2)`): slider thumb.

### Named Rules
**The Single Lift Rule.** Only grouped lists are raised. Rows, buttons, and headers stay on their surface.

## Shapes

Soft, native corners: 10px for grouped lists, 6px for buttons, the select and focus rings, full pills for switches, circles for knobs and thumbs. Tracks are 4px tall with 4px radius; meters are 2px tall with 2px radius. Borders are 1px hairlines only. Lists clip their rows.

## Components

### Buttons
Restrained text buttons, no icons.
- **Shape:** gently rounded (6px), 28px minimum height, 1px strong hairline border.
- **Default:** control fill, ink text, weight 500 (for example "Forget").
- **Primary:** Settings Blue fill and border with white text; one at a time ("Restart to update").
- **Plain:** transparent, secondary ink, 4px 8px padding ("Quit"); hover fills with the control-active gray and darkens text.
- **Toggle (Mute):** default button at 68px minimum width; pressed state uses the Blue Wash fill, blue text, weight 600, and relabels to "Muted".
- **Hover / Active / Focus:** 150ms color transitions; hover and active step through control-hover and control-active; focus is a 2px blue outline offset 2px. Disabled drops to 50% opacity with no hover.

### Inputs / Fields
- **Select:** native select, full width, control fill, strong hairline border, 6px radius, 30px minimum height; hover shifts to control-hover.
- **Switch:** 36 by 20px pill; strong-hairline track off, blue track on; 16px knob slides 16px over 200ms. Disabled switches go gray at 60% opacity.
- **Volume slider:** 4px track, blue fill up to the value and strong hairline after; 18px round thumb with a hairline border that scales slightly on hover and press. When the output is muted the fill turns tertiary ink.

### Cards / Containers
- **Grouped list:** Raised White, 10px radius, group lift shadow, no border, rows separated by 1px hairlines. The only container in the system.

### Level Meter (signature)
A single 2px bar on the Sunken Track, filled in Meter Gray by horizontal scale, with a right-aligned dB value in secondary ink ("−12.4 dB", "−∞ dB"). Under an output slider it is inset 9px to match the thumb's travel and dims to 35% when muted.

### Panel Header and Footer
Header: app title left, run-state sentence right (secondary ink idle, ink when running, red on error, ellipsized). Footer: "Start with system" switch left; version in tertiary ink, the primary restart button when an update is ready, and the plain Quit button right.

## Do's and Don'ts

### Do:
- **Do** use the system font stack at 13px for everything but the app title.
- **Do** put related settings in a grouped list with hairline separators and a plain group heading above it.
- **Do** write every state as text, and use Error Red only for error text.
- **Do** keep meters as 2px gray bars with a dB number beside them.
- **Do** follow the OS theme for all neutrals, using each token's `-dark` counterpart.
- **Do** write UI copy in English first, then translate it in every supported language (`ui/i18n.js`, tray menu in `src-tauri/src/i18n.rs`), and check that the French copy still fits the 380px panel.

### Don't:
- **Don't** use icons of any kind, including Lucide or custom-drawn ones.
- **Don't** use Inter or any web font.
- **Don't** build LED-style indicators, segmented or colored meters, or a dark mixer look.
- **Don't** add subtitles or eyebrows above or below headings.
- **Don't** add decorative color spots, heavy gradients, or neon purple styling.
- **Don't** add hover effects to anything that is not clickable.
- **Don't** use a second accent color, or blue on non-interactive elements.
- **Don't** add decorative cards; the grouped list is the only container.
- **Don't** write an em dash in UI copy.
