//! Per-user install on Windows, the way Discord does it.
//!
//! The release is a single executable. Started from anywhere else, it copies
//! itself to `%LOCALAPPDATA%\AudioMirror`, registers itself (Start menu
//! shortcut, entry in Installed apps) and relaunches from there, without a
//! window or a question. Each launch from the install folder refreshes that
//! registration, so the version Windows shows follows the updates, which
//! replace the installed executable in place. `--uninstall`, run by Windows
//! from Installed apps, undoes it all and keeps the settings.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs, io};

use windows::core::{Interface, HSTRING};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::{
    SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_SYSTEM32,
};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

const DIR_NAME: &str = "AudioMirror";
const EXE_NAME: &str = "audio-mirror.exe";
const OLD_EXE_NAME: &str = "audio-mirror.old.exe";
const PRODUCT: &str = "Audio Mirror";
const UNINSTALL_ARG: &str = "--uninstall";
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\AudioMirror";
/// Values written by the autostart plugin, named after the product.
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const STARTUP_APPROVED_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
/// WebView2 data folder, named after the app identifier.
const WEBVIEW_DIR: &str = "io.github.alegzandr.audiomirror";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Runs before Tauri starts. Returns false when this process must exit
/// (the installed copy was launched, or the app was uninstalled).
pub fn prepare() -> bool {
    restrict_dll_search();
    // Development builds run from the target folder.
    if cfg!(debug_assertions) {
        return true;
    }
    let (Some(dir), Ok(current)) = (install_dir(), env::current_exe()) else {
        return true;
    };
    let target = dir.join(EXE_NAME);
    let args: Vec<OsString> = env::args_os().skip(1).collect();

    if args.iter().any(|a| a == UNINSTALL_ARG) {
        if let Err(e) = uninstall(&dir) {
            log::warn!("uninstall: {e}");
        }
        return false;
    }

    if same_file(&current, &target) {
        let _ = fs::remove_file(dir.join(OLD_EXE_NAME));
        if let Err(e) = register(&dir, &target) {
            log::warn!("install: {e}");
        }
        return true;
    }

    // An older setup file never downgrades a newer install.
    let keep_installed = target.exists()
        && installed_version().is_some_and(|v| parse(&v) > parse(env!("CARGO_PKG_VERSION")));
    if !keep_installed {
        if let Err(e) = copy_to(&current, &dir, &target) {
            // Better to run from where it is than not at all.
            log::warn!("install: {e}");
            return true;
        }
    }
    // If the installed app is already running, single-instance shows its panel.
    Command::new(&target).args(&args).spawn().is_err()
}

fn install_dir() -> Option<PathBuf> {
    env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join(DIR_NAME))
}

fn shortcut_path() -> Option<PathBuf> {
    env::var_os("APPDATA").map(|d| {
        PathBuf::from(d)
            .join(r"Microsoft\Windows\Start Menu\Programs")
            .join(format!("{PRODUCT}.lnk"))
    })
}

fn system32() -> PathBuf {
    let mut buf = [0u16; 260];
    // SAFETY: the buffer outlives the call and its length is passed with it.
    let len = unsafe { GetSystemDirectoryW(Some(&mut buf)) } as usize;
    if len == 0 || len > buf.len() {
        return PathBuf::from(r"C:\Windows\System32");
    }
    PathBuf::from(OsString::from_wide(&buf[..len]))
}

/// Keeps DLLs loaded by name from being picked up next to the executable,
/// which for a setup file is usually the Downloads folder.
fn restrict_dll_search() {
    // SAFETY: plain flag setter, called before any other thread exists.
    if let Err(e) = unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) } {
        log::warn!("dll search: {e}");
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn parse(version: &str) -> Vec<u64> {
    version.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

fn installed_version() -> Option<String> {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(UNINSTALL_KEY)
        .and_then(|k| k.get_value("DisplayVersion"))
        .ok()
}

fn copy_to(current: &Path, dir: &Path, target: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let staged = dir.join(format!("{EXE_NAME}.new"));
    fs::copy(current, &staged)?;
    if target.exists() {
        // A running executable cannot be overwritten, but it can be renamed.
        let old = dir.join(OLD_EXE_NAME);
        let _ = fs::remove_file(&old);
        fs::rename(target, &old)?;
    }
    fs::rename(&staged, target)
}

fn register(dir: &Path, target: &Path) -> io::Result<()> {
    let exe = target.display().to_string();
    let size_kb = fs::metadata(target).map(|m| m.len() / 1024).unwrap_or(0);
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(UNINSTALL_KEY)?;
    key.set_value("DisplayName", &PRODUCT)?;
    key.set_value("DisplayVersion", &env!("CARGO_PKG_VERSION"))?;
    key.set_value("DisplayIcon", &format!("{exe},0"))?;
    key.set_value("Publisher", &"Alegzandr")?;
    key.set_value("URLInfoAbout", &"https://github.com/Alegzandr/audio-mirror")?;
    key.set_value("InstallLocation", &dir.display().to_string())?;
    // Uninstalling asks nothing, so both commands are the same.
    let uninstall = format!("\"{exe}\" {UNINSTALL_ARG}");
    key.set_value("UninstallString", &uninstall)?;
    key.set_value("QuietUninstallString", &uninstall)?;
    key.set_value("NoModify", &1u32)?;
    key.set_value("NoRepair", &1u32)?;
    key.set_value("EstimatedSize", &(size_kb as u32))?;

    if let Some(link) = shortcut_path().filter(|p| !p.exists()) {
        create_shortcut(target, &link).map_err(io::Error::other)?;
    }
    Ok(())
}

fn create_shortcut(target: &Path, link: &Path) -> windows::core::Result<()> {
    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let result = (|| {
            let shell: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            shell.SetPath(&HSTRING::from(target.as_os_str()))?;
            if let Some(dir) = target.parent() {
                shell.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
            }
            shell.SetIconLocation(&HSTRING::from(target.as_os_str()), 0)?;
            shell
                .cast::<IPersistFile>()?
                .Save(&HSTRING::from(link.as_os_str()), true)
        })();
        if init.is_ok() {
            CoUninitialize();
        }
        result
    }
}

fn uninstall(dir: &Path) -> io::Result<()> {
    // Other copies keep their executable and WebView2 files locked.
    // Full path: a bare name is also looked up next to this executable.
    let _ = Command::new(system32().join("taskkill.exe"))
        .args(["/F", "/T", "/IM", EXE_NAME, "/FI"])
        .arg(format!("PID ne {}", std::process::id()))
        .creation_flags(CREATE_NO_WINDOW)
        .status();

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    for key in [RUN_KEY, STARTUP_APPROVED_KEY] {
        if let Ok(k) = hkcu.open_subkey_with_flags(key, winreg::enums::KEY_SET_VALUE) {
            let _ = k.delete_value(PRODUCT);
        }
    }
    let _ = hkcu.delete_subkey_all(UNINSTALL_KEY);
    if let Some(link) = shortcut_path() {
        let _ = fs::remove_file(link);
    }
    if let Some(local) = env::var_os("LOCALAPPDATA") {
        let _ = fs::remove_dir_all(PathBuf::from(local).join(WEBVIEW_DIR));
    }

    // Moves this executable out of the folder, deleted once the process ends.
    self_replace::self_delete_outside_path(dir)?;
    fs::remove_dir_all(dir)
}
