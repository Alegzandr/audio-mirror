# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Audio Mirror is a Tauri 2 tray app that plays one audio source on N output devices, each with its own volume and mute. The audio engine is a faithful port of OBS Studio's desktop audio capture and audio monitoring; the only intended deviations are 1-to-N monitors, per-output mute, and monitors that retry every 3 s when their device cannot be opened. When changing engine behavior, follow the matching OBS code path (the file mapping is in `README.md` and in the module docs of `src-tauri/src/audio/mod.rs`).

`PRODUCT.md` (product constraints, brand rules) and `DESIGN.md` (design tokens) govern UI work. Notable rules: English only in code and comments, no em dash, no Inter font, no icon libraries or custom-drawn icons, no LED-style indicators, no hover effect on non-clickable elements, output state is shown as text.

## Commands

FFmpeg's `libswresample` and `libavutil` must be available as static libraries before anything compiles. `src-tauri/build.rs` looks in `FFMPEG_DIR`, then vcpkg on Windows (`VCPKG_ROOT` or `../vcpkg` next to the repo, triplet `x64-windows-static-md`), then `third_party/ffmpeg/<target>` (built by `scripts/build-ffmpeg.sh <rust-target>`; on Linux run `scripts/linux-deps.sh` first).

```sh
npm ci
npm run dev                                   # run the app (tauri dev)
npm run build                                 # tauri build
cargo test --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml <test_name>   # single test
```

CI (`.github/workflows/ci.yml`) also requires:

```sh
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets -- -D warnings
npm run check:ui                              # node --check ui/app.js and ui/splash.js
```

Platform modules are `cfg`-gated, so local clippy/tests only cover the host OS; CI runs tests on Windows, macOS and Linux.

The UI can be previewed without Tauri by opening `ui/index.html` in a browser: `ui/app.js` falls back to `demoInvoke` (fake devices and status) when `window.__TAURI__` is absent. Keep the demo in sync when adding or changing commands.

## Architecture

- `src-tauri/src/lib.rs`: Tauri setup and the `#[tauri::command]` handlers called by the UI. `AppState::update` is the single mutation path: mutate `AppConfig`, save it to JSON, then push `config.engine_config()` to the engine. Volume and mute changes also go straight to `Engine::set_gain`/`set_muted` for immediate effect. Closing the panel hides it; only Quit exits.
- `config.rs`: persisted settings (source id, per-output enabled/fader/mute and last known name). The fader position (0..1) is stored; `volume::fader_to_gain` (OBS log curve) converts it to gain.
- `audio/mod.rs`: `Engine` is a handle to an `audio-supervisor` thread driven by a `Cmd` channel (`Apply`, `Restart`, `System` events, `Shutdown`). The supervisor owns the session: one `Capture` for the source and one `Monitor` per enabled output, rebuilds them on config or default-device changes, retries failed monitors on a 250 ms tick, and publishes a `Status` the UI polls via the `status` command. No enabled output means the engine is stopped. Per-output gain/mute/peak live in lock-free `volume::OutputShared` shared between the engine handle and the audio threads.
- `audio/hub.rs`: `SourceHub`, the source side of libobs. Captures convert to 48 kHz stereo float and fan packets out to registered `AudioCallback`s (monitors) on the capture thread.
- `audio/swr.rs`: FFI wrapper over libswresample with OBS's settings and mono upmix matrix; each monitor resamples to its device format.
- Platform backends, selected at compile time as `platform` in `audio/mod.rs`, each providing enumeration, capture, monitoring and `watch_system`: `wasapi.rs` (Windows), `coreaudio.rs` (macOS, ScreenCaptureKit + AUHAL + AudioQueue), `pulse.rs` (Linux, libpulse). A monitor for the device the source captures returns `MonitorInit::Ignored` (OBS's `DO_NOT_SELF_MONITOR`).
- `tray.rs`: tray icon, panel positioning near the tray, menu (Open, Restart audio, Quit).
- `updater.rs`: Discord-style updates from the latest GitHub release. In release builds, setup opens the `splash` window (`ui/splash.html`, polls `update_progress`), installs a newer version and restarts, or closes the splash and calls `lib::start` (engine, tray, panel). While running it re-checks every 6 h and the panel offers a restart. Debug builds skip all of this.
- `install.rs` (Windows only): runs before Tauri. A release exe started outside `%LOCALAPPDATA%\AudioMirror` copies itself there, registers the Start menu shortcut and the Installed apps entry, relaunches and exits; `--uninstall` removes everything but the settings.
- `ui/`: build-free HTML/CSS/JS panel (no framework, no bundler), served directly by Tauri.

## Branches and releases

`develop` is the working branch; `main` receives merges from it. A `vX.Y.Z` tag on `main` triggers `.github/workflows/release.yml`, which checks the tag against the version in `src-tauri/Cargo.toml`. Bump the version in both `src-tauri/Cargo.toml` and `package.json`. Updater artifacts are signed with a minisign key whose public half is in `src-tauri/tauri.conf.json`; the private key only lives in repository secrets.
