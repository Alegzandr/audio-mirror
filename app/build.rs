//! Links FFmpeg's libswresample and libavutil statically, as OBS uses them.
//!
//! Lookup order:
//! 1. `FFMPEG_DIR` (a prefix with `lib/` inside)
//! 2. vcpkg on Windows (`VCPKG_ROOT`, or `../../vcpkg` next to the project)
//! 3. `third_party/ffmpeg/<target>` built by `scripts/build-ffmpeg.sh`
//!
//! It also compiles the interface (`src/ui/panel.slint`), and on Windows
//! gives the executable its icon, its manifest and its version details.

use std::env;
use std::path::{Path, PathBuf};

fn main() {
    slint_build::compile("src/ui/panel.slint").expect("panel.slint");
    windows_resources();

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

/// The icon Explorer and the taskbar show, the details in Properties, and
/// the manifest: per-monitor DPI awareness, so the panel is drawn sharp and
/// placed in physical pixels on every display.
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
</assembly>
"#;

fn windows_resources() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo:rerun-if-changed=icons/icon.ico");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("icons/icon.ico")
        .set("ProductName", "Audio Mirror")
        .set("FileDescription", "Audio Mirror")
        .set("CompanyName", "Alegzandr")
        .set("LegalCopyright", "MIT License")
        .set_manifest(MANIFEST);
    if let Err(e) = res.compile() {
        panic!("Windows resources: {e}");
    }
}
