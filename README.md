# Audio Mirror

Plays one audio source on several output devices at once, each with its own volume. It lives in the tray: left click opens the panel, right click offers Open and Quit.

Typical use: hear your desktop audio in your headphones while also sending it to a virtual cable, a capture card or a second computer.

## Download

Grab the file for your system from the [latest release](https://github.com/Alegzandr/audio-mirror/releases/latest). Nothing to install.

| System | File | Notes |
| --- | --- | --- |
| Windows 10/11 (x64) | `Audio-Mirror_<version>_windows_x64_portable.exe` | Needs the WebView2 runtime, already present on Windows 11. The binary is not code-signed, so SmartScreen may ask for confirmation. |
| macOS 13+ (Apple Silicon and Intel) | `Audio-Mirror_<version>_macos_universal.zip` | Desktop audio asks for the Screen Recording permission, like OBS. The app is not notarized: after unzipping, run `xattr -cr "Audio Mirror.app"` once. |
| Linux (x86_64) | `Audio-Mirror_<version>_linux_x86_64.AppImage` | `chmod +x` it first. Desktop audio needs PulseAudio or PipeWire (pipewire-pulse). |

The app starts with the system after its first launch (it can be turned off in the panel), and updates itself from the latest release: the update is downloaded, its signature verified and installed in the background, and the panel offers a restart.

## How it works

The audio engine is a port of OBS Studio's desktop audio capture and audio monitoring, with one difference: a source feeds N monitors instead of one. Everything else follows the OBS code path by path.

| | Windows | macOS | Linux |
| --- | --- | --- | --- |
| Desktop audio | WASAPI loopback of the default output (`win-wasapi`) | ScreenCaptureKit, this app's own audio excluded (`mac-sck-audio-capture.m`) | Default sink's `.monitor` (`pulse-input.c`) |
| Other sources | Loopback of a given output, input devices | Input devices through AUHAL (`mac-audio.c`), loopback drivers as output captures | Other monitors, input sources |
| Monitoring | Written to WASAPI as each packet arrives, client reopened on failure (`wasapi-output.c`) | AudioQueue, three 30 ms buffers, 90 ms prefill (`coreaudio-output.c`) | Corked stream uncorked at 25 ms, buffer grown on backlog (`pulseaudio-output.c`) |
| Reconnection | Capture retries every 3 s and restarts when the default output changes; monitors are rebuilt on that change | Input capture retries every 2 s | Streams are reopened after 3 s |

Shared by all platforms, as in libobs:

- Each source is converted to 48 kHz stereo float (`process_audio`), then handed to the monitors on the capture thread (`source_signal_audio_data`).
- Each monitor converts to its device format with FFmpeg's libswresample, using OBS's settings and mono upmix matrix, then applies its volume. The slider uses OBS's logarithmic fader curve.
- An output recorded by the source is never a destination (`OBS_SOURCE_DO_NOT_SELF_MONITOR`). On macOS the capture leaves this app out, so any output can be used.

Two things OBS does not do, because a background service needs them: a monitor whose device cannot be opened is retried every 3 seconds, and each output has its own mute.

Code layout, in `src-tauri/src/audio`:

| File | Port of |
| --- | --- |
| `hub.rs` | Audio half of `obs-source.c` |
| `swr.rs` | `media-io/audio-resampler-ffmpeg.c` |
| `wasapi.rs` | `win-wasapi.cpp`, `audio-monitoring/win32` |
| `coreaudio.rs` | `mac-sck-audio-capture.m`, `mac-audio.c`, `audio-monitoring/osx` |
| `pulse.rs` | `pulse-input.c`, `audio-monitoring/pulse` |
| `mod.rs` | Session and monitor lifecycle (`audio_monitor_create`, `obs_reset_audio_monitoring`) |

## Development

Requirements: Rust (stable), Node.js 22, and FFmpeg's libavutil and libswresample as static libraries:

- Windows: `vcpkg install ffmpeg[core,swresample]:x64-windows-static-md` (with `VCPKG_ROOT` set, or vcpkg cloned next to this repository)
- macOS and Linux: `scripts/build-ffmpeg.sh <rust-target>` (on Linux, run `scripts/linux-deps.sh` first)

`FFMPEG_DIR` can point at any other prefix.

```sh
npm ci
npm run dev        # run the app
cargo test --manifest-path src-tauri/Cargo.toml
```

Opening `ui/index.html` directly in a browser shows the panel with demo data.

## Branches and releases

- `develop` is the working branch. `main` receives merges from `develop`.
- Every push or pull request to `main` or `develop` runs CI: rustfmt, clippy and the tests on Windows, macOS and Linux.
- Pushing a tag `vX.Y.Z` on a commit of `main` runs the release workflow: it checks the tag matches the version in `src-tauri/Cargo.toml`, runs the tests, builds the three portable executables, signs them for the updater, and publishes a GitHub release with `latest.json`.

To release:

```sh
# bump version in src-tauri/Cargo.toml and package.json, merge to main, then
git tag v0.2.0
git push origin v0.2.0
```

### Signing key

Updates are verified with a minisign key. The public key is in `src-tauri/tauri.conf.json`. The private key never enters the repository; the release workflow reads it from two repository secrets:

- `TAURI_SIGNING_PRIVATE_KEY`: content of the private key file
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: its password

Losing the private key means existing installs can no longer update.

## License

MIT
