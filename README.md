# Audio Mirror

Plays one audio source on several output devices at once, each with its own volume. It lives in the tray: left click opens the panel, right click offers Open and Quit.

Typical use: hear your desktop audio in your headphones while also sending it to a virtual cable, a capture card or a second computer.

## Download

Grab the file for your system from the [latest release](https://github.com/Alegzandr/audio-mirror/releases/latest). Nothing to install.

| System | File | Notes |
| --- | --- | --- |
| Windows 10/11 (x64) | `Audio-Mirror_<version>_windows_x64_portable.exe` | Needs the WebView2 runtime, already present on Windows 11. The binary is not code-signed, so SmartScreen may ask for confirmation. |
| macOS 10.15+ (Apple Silicon and Intel) | `Audio-Mirror_<version>_macos_universal.zip` | Desktop audio capture needs macOS 14.6+. The app is not notarized: after unzipping, run `xattr -cr "Audio Mirror.app"` once. |
| Linux (x86_64) | `Audio-Mirror_<version>_linux_x86_64.AppImage` | `chmod +x` it first. Desktop audio needs PulseAudio or PipeWire (pipewire-pulse). |

The app starts with the system after its first launch (it can be turned off in the panel), and updates itself from the latest release: the update is downloaded, its signature verified and installed in the background, and the panel offers a restart.

## How it works

The audio path follows OBS Studio's audio monitoring (`libobs/audio-monitoring`):

- **Desktop audio** is a loopback capture of the default output: WASAPI loopback on Windows, a CoreAudio process tap on macOS, the default sink's `.monitor` source on PulseAudio. When the default output changes, the capture follows it.
- **Each output** gets its own lock-free queue, a resampler to the device's native rate and channel layout, a 40 ms prefill before playback starts, and the volume applied just before the samples are written, with a short ramp to avoid clicks. The volume slider uses OBS's logarithmic fader curve.
- **Clock drift** between the source and each output is absorbed by steering the resampling ratio (within 0.5%) to keep the queue level steady. When a queue still runs dry, the output plays silence and prefills again; when it fills up, the excess is dropped, as OBS does.
- **Feedback protection**: the output being captured can never be a destination (the equivalent of `OBS_SOURCE_DO_NOT_SELF_MONITOR`).
- **Reconnection**: a supervisor thread rebuilds any stream that fails every 3 seconds, like OBS's WASAPI plugin.

Code layout, in `src-tauri/src`:

| File | Role |
| --- | --- |
| `dsp.rs` | Real-time path: queues, resampling, channel remapping, volume, drift control |
| `engine.rs` | cpal streams, supervisor, reconnection, status |
| `devices.rs` | Device enumeration, source resolution, loopback handling |
| `config.rs` | Persisted choices |
| `tray.rs` | Tray icon and panel placement |
| `updater.rs` | Silent updates |

## Development

Requirements: Rust (stable), Node.js 22. On Linux, run `scripts/linux-deps.sh` for the system libraries.

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
