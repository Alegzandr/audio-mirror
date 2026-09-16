---
version: 1
slug: "ui-index-html"
primary_target: "ui/index.html"
related_targets: []
---

Scope: the tray panel of the desktop app (ui/index.html), 380 by 560, unfolded above the taskbar. Mode: Operate.
Audience: streamers, panel opened briefly next to OBS. Task: pick a source, turn outputs on, adjust their volumes, check that everything plays.

## Direction contract

THESIS: a system settings panel played straight, at the finish level of macOS Settings and Windows 11 Settings. Refuses the dark LED mixer and decorative cards.

OWN-WORLD: system font only, grouped rows on a slightly raised surface, hairline separators, native controls (switches, sliders, select, text buttons). Neutrals follow the OS theme, one sober blue accent for the primary action, checked state and slider fill. One error color. No icons.

STORY: the user sees the source and its level, sees each output and its state as text, adjusts a volume, and knows at a glance everything plays.

FIRST VIEWPORT: header with the app name left and the run state as text right; Source group (select, thin level meter with dB); Outputs group (switch, name, state, volume slider with a thin meter under it, dB value in a fixed-width cell, Mute toggle); footer with Start with system, version or Restart to update, and Quit.

FORM: category canon, references macOS Settings and Windows 11 Settings, seed bea59542.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
