//! One running copy per user. A second launch asks the first to show its
//! panel, then exits.
//!
//! The running copy holds an exclusive lock on `instance.lock` in the
//! settings folder; the system releases it when the process ends, however
//! it ends, so a crash never leaves a stale lock. The request to show the
//! panel goes through a named event on Windows and a Unix socket next to the
//! lock elsewhere.

use std::fs::{self, File, OpenOptions};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const LOCK_FILE: &str = "instance.lock";

/// Held for the life of the process: dropping it ends the single-instance
/// guarantee.
pub struct Guard {
    _lock: Option<File>,
}

pub enum Outcome {
    /// This process is the only one; `on_show` runs on a background thread
    /// each time another launch asks for the panel.
    Primary(Guard),
    /// Another copy runs and was asked to show its panel.
    Secondary,
}

fn dir() -> Option<PathBuf> {
    crate::paths::config_dir()
}

/// Takes the lock, or hands the launch to the copy that holds it.
///
/// `wait_for_lock` is for a restart: the process that spawned this one may
/// still be on its way out, so the lock is retried for a few seconds before
/// the launch counts as a second copy.
pub fn acquire(wait_for_lock: bool, on_show: impl Fn() + Send + 'static) -> Outcome {
    // Without a settings folder or a lock file, run unguarded rather than
    // not at all.
    let Some(dir) = dir() else {
        return Outcome::Primary(Guard { _lock: None });
    };
    let _ = fs::create_dir_all(&dir);
    let lock = match OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(LOCK_FILE))
    {
        Ok(f) => f,
        Err(e) => {
            log::warn!("single instance: {e}");
            return Outcome::Primary(Guard { _lock: None });
        }
    };

    let deadline = Instant::now() + Duration::from_secs(if wait_for_lock { 10 } else { 0 });
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(fs::TryLockError::WouldBlock) => {
                signal::send(&dir);
                return Outcome::Secondary;
            }
            Err(fs::TryLockError::Error(e)) => {
                // A file system without locks: better two copies than none.
                log::warn!("single instance: {e}");
                break;
            }
        }
    }
    signal::listen(&dir, on_show);
    Outcome::Primary(Guard { _lock: Some(lock) })
}

#[cfg(windows)]
mod signal {
    use std::path::Path;

    use windows::core::HSTRING;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        CreateEventW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE, INFINITE,
    };

    /// `Local\` keeps the event to the user's session.
    fn name() -> HSTRING {
        HSTRING::from(format!("Local\\{}.show", crate::paths::IDENTIFIER))
    }

    pub fn send(_dir: &Path) {
        // SAFETY: plain Win32 calls on a handle this function owns.
        unsafe {
            if let Ok(event) = OpenEventW(EVENT_MODIFY_STATE, false, &name()) {
                let _ = SetEvent(event);
                let _ = CloseHandle(event);
            }
        }
    }

    pub fn listen(_dir: &Path, on_show: impl Fn() + Send + 'static) {
        // An auto-reset event: each wait consumes one request.
        // SAFETY: the handle lives for the whole process, owned by the thread.
        let event = match unsafe { CreateEventW(None, false, false, &name()) } {
            Ok(e) => e,
            Err(e) => {
                log::warn!("single instance: {e}");
                return;
            }
        };
        let event = event.0 as usize;
        std::thread::Builder::new()
            .name("single-instance".into())
            .spawn(move || loop {
                let handle = windows::Win32::Foundation::HANDLE(event as *mut _);
                // SAFETY: the event handle is never closed.
                unsafe { WaitForSingleObject(handle, INFINITE) };
                on_show();
            })
            .ok();
    }
}

#[cfg(unix)]
mod signal {
    use std::io::Write;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;

    const SOCKET: &str = "instance.sock";

    pub fn send(dir: &Path) {
        if let Ok(mut stream) = UnixStream::connect(dir.join(SOCKET)) {
            let _ = stream.write_all(b"show");
        }
    }

    pub fn listen(dir: &Path, on_show: impl Fn() + Send + 'static) {
        // The lock is held, so a socket file left here is a dead copy's.
        let path = dir.join(SOCKET);
        let _ = std::fs::remove_file(&path);
        let listener = match UnixListener::bind(&path) {
            Ok(l) => l,
            Err(e) => {
                log::warn!("single instance: {e}");
                return;
            }
        };
        std::thread::Builder::new()
            .name("single-instance".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if stream.is_ok() {
                        on_show();
                    }
                }
            })
            .ok();
    }
}
