//! Links FFmpeg's libswresample and libavutil statically, as OBS uses them.
//!
//! Lookup order:
//! 1. `FFMPEG_DIR` (a prefix with `lib/` inside)
//! 2. vcpkg on Windows (`VCPKG_ROOT`, or `../../vcpkg` next to the project)
//! 3. `third_party/ffmpeg/<target>` built by `scripts/build-ffmpeg.sh`

use std::env;
use std::path::{Path, PathBuf};

/// Commands from `lib.rs`. Listing them makes each one denied unless a file in
/// `capabilities/` grants it to a window, so the splash can only poll its
/// progress and the panel cannot reach that command.
const COMMANDS: &[&str] = &[
    "snapshot",
    "status",
    "set_source",
    "set_output_enabled",
    "set_output_volume",
    "set_output_muted",
    "rename_output",
    "source_menu",
    "set_autostart",
    "hide_panel",
    "restart",
    "quit",
    "update_progress",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("tauri build");

    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    println!("cargo:rerun-if-env-changed=VCPKG_ROOT");

    let target = env::var("TARGET").unwrap();
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let windows = target.contains("windows");

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = env::var("FFMPEG_DIR") {
        candidates.push(dir.into());
    }
    if windows {
        let triplet = "x64-windows-static-md";
        if let Ok(root) = env::var("VCPKG_ROOT") {
            candidates.push(Path::new(&root).join("installed").join(triplet));
        }
        candidates.push(manifest.join("../../vcpkg/installed").join(triplet));
    }
    candidates.push(manifest.join("../third_party/ffmpeg").join(&target));

    let prefix = candidates
        .into_iter()
        .find(|p| p.join("lib").exists())
        .unwrap_or_else(|| {
            panic!(
                "FFmpeg (libswresample, libavutil) not found for {target}. \
                 Run scripts/build-ffmpeg.sh, install it with vcpkg, or set FFMPEG_DIR."
            )
        });

    println!(
        "cargo:rustc-link-search=native={}",
        prefix.join("lib").display()
    );
    println!("cargo:rustc-link-lib=static=swresample");
    println!("cargo:rustc-link-lib=static=avutil");
    if target.ends_with("windows-msvc") {
        // DLLs imported at load time come from System32 only, never from the
        // folder the setup file was started in (LOAD_LIBRARY_SEARCH_SYSTEM32).
        println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
    }
    if windows {
        println!("cargo:rustc-link-lib=user32");
        println!("cargo:rustc-link-lib=bcrypt");
    } else if target.contains("linux") {
        println!("cargo:rustc-link-lib=m");
    }
}
