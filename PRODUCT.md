# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Stack

Tauri 2 (Rust) with a build-free HTML/CSS/JS interface served from `ui/`. Delegated choice: no framework, the interface is a single tray panel.

## Users

Streamers and content creators on Windows, macOS or Linux. During a live stream or a recording session they want to hear the computer's sound in their headphones while also sending it to a virtual cable, a capture card or a second PC.

## Product Purpose

Audio Mirror takes one audio source (the default output, a loopback capture of a specific output, or an input device) and plays it on N output devices at once, each with its own volume. Success: set it once, it lives in the tray, can start with the system, and reconnects by itself when a device comes back.

## Positioning

Behaves exactly like OBS Studio's audio monitoring, which streamers already trust during live sessions, without OBS's interface and for several outputs at once.

## Operating Context

A tray icon. Left click unfolds a small panel above the taskbar (like OneDrive), right click offers Open, Restart audio and Quit. Opened briefly next to OBS, a game or a DAW: check the sound flows, adjust a volume, mute an output.

## Capabilities and Constraints

- One source at a time, N outputs, per-output volume (OBS logarithmic fader curve) and mute.
- The audio engine reproduces OBS Studio's desktop capture and monitoring behavior on each platform, including its buffering; the only additions are 1 to N outputs, per-output mute and retrying monitors.
- Minimum settings, maximum decisions made for the user: mirroring runs whenever an output is on; every launch checks for an update in a small splash window and applies it before starting, updates found while running install silently and the panel only offers a restart; start with system is off by default and turned on by the user in the panel.
- An output captured by the source can never be a destination.
- One file per system: a self-installing executable on Windows (per user, no installer window, like Discord), an AppImage on Linux, an app bundle on macOS; updates from the latest GitHub release.
- Desktop capture: macOS 13 or later (ScreenCaptureKit, Screen Recording permission), PulseAudio or PipeWire on Linux.
- English only for now; more languages later. All code and comments in English.

## Brand Commitments

Name: Audio Mirror. Owner constraints: no subtitles, no Inter font, no Lucide or custom-drawn icons, no decorative color spots, no LED-style indicators, no em dash, no hover effect on anything that is not clickable, no heavy gradients, no neon purple style.

## Evidence on Hand

No testimonials, numbers or screenshots of real use. Do not invent any.

## Product Principles

- Sound first: each output's state reads at a glance, as text.
- Decide for the user: fewer settings, sensible defaults, everything remembered.
- Safe by default: no feedback loop possible, automatic reconnection.
