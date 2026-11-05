//! Updates from the latest GitHub release, the way Discord does them.
//!
//! Every launch shows a small splash window that checks for a new version
//! before anything else starts. A new version is downloaded, its minisign
//! signature verified, installed in place, and the app restarts into it.
//! Without one (or without network) the splash closes and the app starts.
//! While the app runs, it checks again every few hours; a version found then
//! is installed right away and the panel offers a restart.
//!
//! The manifest, the signatures and the artifacts are the ones Tauri's
//! updater plugin reads (`latest.json` with a `platforms` map, minisign
//! signatures in base64, the key below), so a release reaches installs made
//! by any version, Tauri or not. On Windows the `.exe` is swapped in place,
//! on Linux the AppImage file is rewritten, on macOS the `.app` bundle is
//! replaced by the one in the `.app.tar.gz`.
//!
//! A version that downloads but fails to install (no write access to the
//! install folder, say) is remembered, also on disk: later launches start
//! without downloading it again, and the running app tries it again only
//! after the usual wait between checks, not after the short network retry.
//!
//! Everything here blocks: the shell runs it off the UI thread.

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use minisign_verify::{PublicKey, Signature};
use parking_lot::Mutex;
use serde::Deserialize;

pub const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);
/// No network at login must not hold the app for long.
const CHECK_TIMEOUT: Duration = Duration::from_secs(8);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Keeps the splash from flashing when the check is instant.
pub const SPLASH_MIN: Duration = Duration::from_millis(700);
/// Holds the version that last failed to install, next to the settings.
const FAILED_FILE: &str = "update-failed.txt";
/// Largest download accepted; the real artifacts are a few tens of MB.
const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;
/// Passed to the copy started by [`relaunch`], which waits for this one to
/// let go of the single-instance lock.
pub const RESTARTED_ARG: &str = "--restarted";

const ENDPOINT: &str =
    "https://github.com/Alegzandr/audio-mirror/releases/latest/download/latest.json";
/// The minisign public key, as Tauri stores it: the key file, in base64.
const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEJFNTBBOURGRTk4NjBERTcKUldUbkRZYnAzNmxRdnJZV1ZQTHZLRWRXem84eWdURU1pa1IzQVM2SmpQaTdTbFB3d1lGTzJ6b2MK";

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum Stage {
    #[default]
    Checking,
    Downloading,
    Installing,
    Restarting,
}

/// What the splash window shows.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Progress {
    pub stage: Stage,
    /// Download progress, when the size is known.
    pub percent: Option<u8>,
}

/// Why no update was installed.
pub enum Failure {
    /// Checking or downloading failed: worth trying again soon.
    Fetch(String),
    /// The download is fine but could not be installed: trying again soon
    /// would only download it again for nothing.
    Install(String),
}

impl Failure {
    /// How long to wait before the next check.
    pub fn wait(&self) -> Duration {
        match self {
            Failure::Fetch(e) => {
                log::warn!("update: {e}");
                RETRY_AFTER
            }
            Failure::Install(e) => {
                log::warn!("update install: {e}");
                CHECK_EVERY
            }
        }
    }
}

/// Development builds never replace themselves and start right away.
pub fn enabled() -> bool {
    !cfg!(debug_assertions)
}

#[derive(Default)]
pub struct Updater {
    progress: Mutex<Progress>,
    /// The version that failed to install in this process.
    failed: Mutex<Option<String>>,
}

#[derive(Deserialize)]
struct Manifest {
    version: String,
    platforms: std::collections::HashMap<String, Platform>,
}

#[derive(Deserialize)]
struct Platform {
    url: String,
    signature: String,
}

/// A newer release for this platform.
struct Release {
    version: String,
    url: String,
    signature: String,
}

impl Updater {
    pub fn progress(&self) -> Progress {
        *self.progress.lock()
    }

    pub fn set_stage(&self, stage: Stage) {
        *self.progress.lock() = Progress {
            stage,
            percent: None,
        };
    }

    fn failed_version(&self) -> Option<String> {
        if let Some(v) = self.failed.lock().clone() {
            return Some(v);
        }
        let text = std::fs::read_to_string(failed_path()?).ok()?;
        Some(text.trim().to_owned()).filter(|v| !v.is_empty())
    }

    /// Remembers the version that failed to install, or forgets it with `None`.
    fn set_failed(&self, version: Option<&str>) {
        *self.failed.lock() = version.map(str::to_owned);
        let Some(path) = failed_path() else {
            return;
        };
        let result = match version {
            Some(v) => path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&path, v)),
            None => match std::fs::remove_file(&path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                r => r,
            },
        };
        if let Err(e) = result {
            log::warn!("update: {}: {e}", path.display());
        }
    }

    /// Installs a newer release if there is one and returns its version.
    /// `splash` reports each step in [`Updater::progress`], and skips a
    /// version that already failed to install: the launch must not wait on
    /// a download that cannot be used. The running app still tries it,
    /// every [`CHECK_EVERY`].
    pub fn check_and_install(&self, splash: bool) -> Result<Option<String>, Failure> {
        let Some(release) = check().map_err(Failure::Fetch)? else {
            return Ok(None);
        };
        if splash && self.failed_version().as_deref() == Some(release.version.as_str()) {
            log::info!("update: {} failed to install before", release.version);
            return Ok(None);
        }
        let bytes = download(&release, |percent| {
            if splash {
                *self.progress.lock() = Progress {
                    stage: Stage::Downloading,
                    percent,
                };
            }
        })
        .map_err(Failure::Fetch)?;
        if splash {
            self.set_stage(Stage::Installing);
        }
        if let Err(e) = install(&release, &bytes) {
            self.set_failed(Some(&release.version));
            return Err(Failure::Install(e));
        }
        if self.failed_version().is_some() {
            self.set_failed(None);
        }
        Ok(Some(release.version))
    }
}

fn failed_path() -> Option<PathBuf> {
    crate::paths::config_dir().map(|d| d.join(FAILED_FILE))
}

/// The key of this platform in the manifest's `platforms`.
fn target() -> Option<&'static str> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some("windows-x86_64")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("linux-x86_64")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("darwin-aarch64")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("darwin-x86_64")
    } else {
        None
    }
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .https_only(true)
        .build()
        .into()
}

fn check() -> Result<Option<Release>, String> {
    check_against(env!("CARGO_PKG_VERSION"))
}

fn check_against(current: &str) -> Result<Option<Release>, String> {
    let target = target().ok_or("no update for this platform")?;
    let manifest: Manifest = agent(CHECK_TIMEOUT)
        .get(ENDPOINT)
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_json()
        .map_err(|e| e.to_string())?;
    if !is_newer(&manifest.version, current)? {
        return Ok(None);
    }
    let platform = manifest
        .platforms
        .get(target)
        .ok_or_else(|| format!("no {target} in the release"))?;
    Ok(Some(Release {
        version: manifest.version.trim_start_matches('v').to_owned(),
        url: platform.url.clone(),
        signature: platform.signature.clone(),
    }))
}

fn is_newer(remote: &str, current: &str) -> Result<bool, String> {
    let parse = |v: &str| semver::Version::parse(v.trim_start_matches('v'));
    let remote = parse(remote).map_err(|e| format!("version {remote}: {e}"))?;
    let current = parse(current).map_err(|e| e.to_string())?;
    Ok(remote > current)
}

/// Downloads the release and checks its signature. `progress` gets the
/// percentage when the size is known.
fn download(release: &Release, mut progress: impl FnMut(Option<u8>)) -> Result<Vec<u8>, String> {
    let mut response = agent(DOWNLOAD_TIMEOUT)
        .get(&release.url)
        .call()
        .map_err(|e| e.to_string())?;
    let total = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&t| t > 0);
    let mut reader = response
        .body_mut()
        .with_config()
        .limit(MAX_DOWNLOAD)
        .reader();
    let mut bytes = Vec::with_capacity(total.unwrap_or(0).min(MAX_DOWNLOAD) as usize);
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        progress(total.map(|t| (bytes.len() as u64 * 100 / t).min(100) as u8));
    }
    verify(&bytes, &release.signature, PUBKEY)?;
    Ok(bytes)
}

/// Checks a minisign signature the way Tauri's updater does: the key and
/// the signature file are both handed over in base64.
fn verify(data: &[u8], signature: &str, pubkey: &str) -> Result<(), String> {
    use base64::Engine;
    let text = |b64: &str| -> Result<String, String> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| e.to_string())?;
        String::from_utf8(bytes).map_err(|e| e.to_string())
    };
    let key = PublicKey::decode(&text(pubkey)?).map_err(|e| e.to_string())?;
    let sig = Signature::decode(&text(signature)?).map_err(|e| e.to_string())?;
    key.verify(data, &sig, true)
        .map_err(|e| format!("signature: {e}"))
}

#[cfg(windows)]
fn install(release: &Release, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;

    // The verified bytes are staged next to the executable, in a file this
    // process creates itself, so nothing can swap them before the replace.
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("no install folder")?;
    let tmp = dir.join(format!("audio-mirror.update-{}.exe", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut f| {
            f.write_all(bytes)?;
            f.sync_all()
        });
    let result = written
        .map_err(|e| e.to_string())
        .and_then(|()| self_replace::self_replace(&tmp).map_err(|e| e.to_string()));
    let _ = std::fs::remove_file(&tmp);
    if result.is_ok() {
        crate::install::updated(&release.version);
    }
    result
}

/// Rewrites the AppImage the app runs from: the new file is written next to
/// it and renamed over it, so a failure leaves the old one whole.
#[cfg(target_os = "linux")]
fn install(_release: &Release, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let target = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .ok_or("not running from an AppImage")?;
    let dir = target.parent().ok_or("no install folder")?;
    let tmp = dir.join(format!(".audio-mirror.update-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mode = std::fs::metadata(&target)
        .map(|m| m.permissions().mode())
        .unwrap_or(0o755);
    let result = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut f| {
            f.write_all(bytes)?;
            f.sync_all()
        })
        .and_then(|()| std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode)))
        .and_then(|()| std::fs::rename(&tmp, &target))
        .map_err(|e| e.to_string());
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Replaces the `.app` bundle the app runs from with the one in the archive.
/// The new bundle is unpacked next to the old one, on the same volume, then
/// the two are swapped by renames; the old one is removed last.
#[cfg(target_os = "macos")]
fn install(_release: &Release, bytes: &[u8]) -> Result<(), String> {
    use std::path::{Component, Path};

    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| e.to_string())?;
    // .../Audio Mirror.app/Contents/MacOS/audio-mirror
    let bundle = exe
        .ancestors()
        .nth(3)
        .filter(|p| p.extension().is_some_and(|e| e == "app"))
        .ok_or("not running from an app bundle")?
        .to_path_buf();
    let dir = bundle.parent().ok_or("no install folder")?;
    let pid = std::process::id();
    let staged = dir.join(format!(".audio-mirror.update-{pid}"));
    let old = dir.join(format!(".audio-mirror.old-{pid}"));
    let _ = std::fs::remove_dir_all(&staged);

    let unpack = || -> Result<(), String> {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
        for entry in archive.entries().map_err(|e| e.to_string())? {
            let mut entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path().map_err(|e| e.to_string())?.into_owned();
            // The archive holds `Audio Mirror.app/...`: drop that first
            // component, and refuse anything that would leave the folder.
            let rest: PathBuf = path.components().skip(1).collect();
            if rest.as_os_str().is_empty() {
                continue;
            }
            if !rest.components().all(|c| matches!(c, Component::Normal(_))) {
                return Err(format!("bad path in update: {}", path.display()));
            }
            let dest = staged.join(&rest);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            entry.unpack(&dest).map_err(|e| e.to_string())?;
        }
        let exe_name = exe.file_name().ok_or("no executable name")?;
        if !staged.join("Contents/MacOS").join(exe_name).is_file() {
            return Err("no executable in the update".into());
        }
        Ok(())
    };
    if let Err(e) = unpack() {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(e);
    }
    let swap = |from: &Path, to: &Path| std::fs::rename(from, to).map_err(|e| e.to_string());
    if let Err(e) = swap(&bundle, &old) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(e);
    }
    if let Err(e) = swap(&staged, &bundle) {
        let _ = swap(&old, &bundle);
        let _ = std::fs::remove_dir_all(&staged);
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(&old);
    // Lets Launch Services notice the new bundle.
    let _ = std::process::Command::new("touch").arg(&bundle).status();
    Ok(())
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn install(_release: &Release, _bytes: &[u8]) -> Result<(), String> {
    Err("updates are not supported on this system".into())
}

/// Starts a new copy of the app with the same arguments, which waits for
/// this one to exit. The caller exits right after.
pub fn relaunch() {
    let exe = std::env::current_exe().ok();
    // A running AppImage's executable lives in a temporary mount: start the
    // AppImage file itself.
    #[cfg(target_os = "linux")]
    let exe = std::env::var_os("APPIMAGE").map(PathBuf::from).or(exe);
    let Some(exe) = exe else {
        return;
    };
    let mut args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| a != RESTARTED_ARG)
        .collect();
    args.push(RESTARTED_ARG.into());
    if let Err(e) = std::process::Command::new(exe).args(args).spawn() {
        log::error!("restart: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert_eq!(is_newer("0.5.0", "0.4.3"), Ok(true));
        assert_eq!(is_newer("v0.4.10", "0.4.9"), Ok(true));
        assert_eq!(is_newer("0.4.3", "0.4.3"), Ok(false));
        assert_eq!(is_newer("0.4.2", "0.4.3"), Ok(false));
        assert!(is_newer("latest", "0.4.3").is_err());
    }

    #[test]
    fn reads_the_tauri_key_and_rejects_a_bad_signature() {
        use base64::Engine;
        let key = base64::engine::general_purpose::STANDARD
            .decode(PUBKEY)
            .unwrap();
        assert!(PublicKey::decode(std::str::from_utf8(&key).unwrap()).is_ok());
        assert!(verify(b"data", "bm90IGEgc2lnbmF0dXJl", PUBKEY).is_err());
    }

    /// Downloads this platform's artifact of the published release and
    /// checks its signature, as an update from 0.0.0 would. Network.
    #[test]
    #[ignore]
    fn verifies_the_published_release() {
        let release = check_against("0.0.0").unwrap().expect("a release");
        let bytes = download(&release, |_| {}).unwrap();
        assert!(bytes.len() > 1024 * 1024);
        let mut tampered = bytes;
        tampered[1000] ^= 1;
        assert!(verify(&tampered, &release.signature, PUBKEY).is_err());
    }

    #[test]
    fn has_a_target_on_supported_systems() {
        if cfg!(any(windows, target_os = "linux", target_os = "macos")) {
            assert!(target().is_some());
        }
    }
}
