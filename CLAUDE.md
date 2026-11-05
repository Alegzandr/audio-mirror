# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Audio Mirror is a Rust tray app, drawn with GPUI, that plays one audio source on N output devices, each with its own volume and mute. The audio engine is a faithful port of OBS Studio's desktop audio capture and audio monitoring; the only intended deviations are 1-to-N monitors, per-output mute, monitors that notice a device that cannot be opened or that goes away while playing, and retry every 3 s (always from the supervisor thread: the WASAPI monitor no longer reopens its device from the audio callback, so one slow device cannot stall the capture and the other outputs), and monitors that follow their device's clock (`audio/drift.rs`). OBS has no such detection: `obs_reset_audio_monitoring` (`libobs/obs.c`) rebuilds monitors only when the user picks another monitoring device, or, on Windows, when the default render device changes; a tray app nobody is watching has to notice on its own. When changing engine behavior, follow the matching OBS code path (the file mapping is in `README.md` and in the module docs of `app/src/audio/mod.rs`).

`PRODUCT.md` (product constraints, brand rules) and `DESIGN.md` (design tokens) govern UI work. Notable rules: English only in code and comments, no em dash, the panel follows native macOS 27 (a Control Center module in Liquid Glass, over a transparent window with native vibrancy or Acrylic), no hover effect on non-clickable elements, output state is shown as text.

## Commands

FFmpeg's `libswresample` and `libavutil` must be available as static libraries before anything compiles. `app/build.rs` looks in `FFMPEG_DIR`, then vcpkg on Windows (`VCPKG_ROOT` or `../vcpkg` next to the repo, triplet `x64-windows-static-md`), then `third_party/ffmpeg/<target>` (built by `scripts/build-ffmpeg.sh <rust-target>`; on Linux run `scripts/linux-deps.sh` first).

```sh
cargo run --manifest-path app/Cargo.toml                 # run the app (debug: no update check, no self-install)
cargo build --release --manifest-path app/Cargo.toml
cargo test --manifest-path app/Cargo.toml
cargo test --manifest-path app/Cargo.toml <test_name>    # single test
```

CI (`.github/workflows/ci.yml`) also requires:

```sh
cargo fmt --manifest-path app/Cargo.toml --check
cargo clippy --manifest-path app/Cargo.toml --locked --all-targets -- -D warnings
```

Platform modules are `cfg`-gated, so local clippy/tests only cover the host OS; CI runs tests on Windows, macOS and Linux.

Release packaging: `scripts/appimage.sh` (Linux, pinned appimagetool and runtime) and `scripts/bundle-macos.sh` (universal `.app`, Info.plist from `app/macos/Info.plist`); the Windows artifact is the plain release exe.

## Architecture

- `app/src/lib.rs`: startup. Windows self-install, logging, single instance, then `ui::run`. The `AppState` is built inside GPUI's start callback: on Windows the engine registers for device changes on the main thread, which must find COM already initialized by GPUI (OLE, STA).
- `state.rs`: `AppState`, the app state and every change the panel makes, free of the UI toolkit. `AppState::update` is the single mutation path: mutate `AppConfig`, save it to JSON, then push `config.engine_config()` to the engine. Volume and mute changes also go straight to `Engine::set_gain`/`set_muted` for immediate effect. The panel sends changes through one `settings` worker thread, so they reach the file in order without blocking the UI. Closing the panel hides it; only Quit exits.
- `paths.rs`: settings and log folders, the ones Tauri resolved for the bundle identifier, so installs that predate GPUI keep their files. `logging.rs`, `single_instance.rs` (a file lock, plus a named event on Windows or a Unix socket to show the first copy's panel) and `autostart.rs` (same login item name and form as Tauri's plugin) replace the Tauri plugins.
- `config.rs`: persisted settings (source id, per-output enabled/fader/mute and last known name). The fader position (0..1) is stored; `volume::fader_to_gain` (OBS log curve) converts it to gain.
- `audio/mod.rs`: `Engine` is a handle to an `audio-supervisor` thread driven by a `Cmd` channel (`Apply`, `Restart`, `System` events, `Shutdown`). The supervisor owns the session: one `Capture` for the source and one `Monitor` per enabled output, rebuilds them on config or default-device changes, retries failed monitors on a 250 ms tick, and publishes a `Status` the panel polls every 100 ms while it is open. No enabled output means the engine is stopped. Per-output gain/mute/peak live in lock-free `volume::OutputShared` shared between the engine handle and the audio threads.
- `audio/message.rs`: every status message the engine reports. A `Kind` pairs a stable `code` with OBS's English wording; `Message` adds the untranslatable detail (an `HRESULT`, a system error). Nothing matches on message text, and a test checks that every code has a French wording in `i18n.rs`.
- `audio/hub.rs`: `SourceHub`, the source side of libobs. Captures convert to 48 kHz stereo float and fan packets out to registered `AudioCallback`s (monitors) on the capture thread.
- `audio/swr.rs`: FFI wrapper over libswresample with OBS's settings and mono upmix matrix; each monitor resamples to its device format with `Resampler::adjustable`, whose ratio `set_drift` moves.
- `audio/drift.rs`: `DriftControl`, a PI loop per monitor that holds the level queued ahead of the device (read from the device clock, never from buffer fullness at packet time) by moving the resampler ratio by up to 1000 ppm. Tuned in a simulation in its tests; `wasapi::tests::drift_on_real_devices` (ignored, silent) checks it on the machine's outputs in 150 s.
- Platform backends, selected at compile time as `platform` in `audio/mod.rs`, each providing enumeration, capture, monitoring and `watch_system`: `wasapi.rs` (Windows), `coreaudio.rs` (macOS, ScreenCaptureKit + AUHAL + AudioQueue), `pulse.rs` (Linux, libpulse). A monitor for the device the source captures returns `MonitorInit::Ignored` (OBS's `DO_NOT_SELF_MONITOR`).
- `i18n.rs`: interface language (English, French), detected once from the system locale with `sys-locale`, and every user-visible string, keyed like `outputs.edit` with the same keys in each language (a test checks it). `Lang::engine` translates the engine's status messages by the code each one carries (`audio/message.rs`) and falls back to the English wording sent with it; engine code keeps its OBS-matching English strings.
- `updater.rs`: Discord-style updates from the latest GitHub release, toolkit-free and blocking (the UI runs it off its thread). It reads the `latest.json` format and minisign signatures of Tauri's updater plugin, so a release reaches installs made by any version; keep that format. On Windows the exe is swapped in place, on Linux the AppImage rewritten, on macOS the `.app` replaced from the `.app.tar.gz`. `verifies_the_published_release` (ignored, network) checks the live release's signature. Debug builds skip all of this.
- `install.rs` (Windows only): runs first. A release exe started outside `%LOCALAPPDATA%\AudioMirror` copies itself there, registers the Start menu shortcut and the Installed apps entry, relaunches and exits; `--uninstall` removes everything but the settings.
- `ui/` (GPUI): `mod.rs` owns the shell (`Shell` global): the tray events, the panel's lifecycle (hidden on focus loss after a 400 ms grace, a 250 ms reopen guard, and on Windows a click-outside watch), the update splash and the 6 h update watch. GPUI cannot hide a window, so hiding removes the panel window and showing opens a new one around the same `Panel` entity; on Windows and Linux GPUI quits when its last window closes, so `keep_alive` opens one that is never shown. `native.rs` does what GPUI does not, on the raw handle: physical-pixel placement near the tray (`panel_position`), topmost, rounding, Acrylic or Liquid Glass. Those calls make the system call back into GPUI synchronously, so `NativeWindow` methods only run from a task between awaits, never inside an update. `panel.rs` is the panel, `view.rs` its pure display logic (tested), `widgets.rs` the controls and SwiftUI springs (GPUI easings must stay in 0..1, so springs are applied inside the animator), `text_field.rs` the rename field (from GPUI's input example, IME-aware), `splash.rs`, `tray.rs` (`tray-icon` on Windows and macOS, `ksni` on Linux), `theme.rs` (design tokens), `assets.rs` (glyphs, icon, Inter TTF; DirectWrite registers Inter as "Inter Variable Text", so the registered name is looked up at startup).

## Branches and releases

`develop` is the working branch; `main` receives merges from it. A `vX.Y.Z` tag on `main` triggers `.github/workflows/release.yml`, which checks the tag against the version in `app/Cargo.toml`. Bump the version there. Updater artifacts are signed by the Tauri CLI's signer, pinned in `scripts/sign`, with a minisign key whose public half is `PUBKEY` in `app/src/updater.rs`; the private key only lives in repository secrets.
