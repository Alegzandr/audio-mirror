//! Writes the engine's messages to a file in the app log folder, so a device
//! that refuses to open leaves a trace to report. Without this the `log`
//! calls spread over the audio backends go nowhere.
//!
//! One file, `audio-mirror.log`, in the folder Tauri's log plugin used. Past
//! [`MAX_SIZE`] it moves to `audio-mirror.old.log`, replacing the previous
//! one, so the folder never holds more than two files.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use log::{LevelFilter, Log, Metadata, Record};
use parking_lot::Mutex;

const MAX_SIZE: u64 = 512 * 1024;
const FILE: &str = "audio-mirror.log";
const OLD_FILE: &str = "audio-mirror.old.log";

struct Logger {
    file: Mutex<Option<(File, u64)>>,
    dir: Option<PathBuf>,
    stdout: bool,
}

impl Logger {
    fn open(&self) -> Option<(File, u64)> {
        let dir = self.dir.as_ref()?;
        fs::create_dir_all(dir).ok()?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(FILE))
            .ok()?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Some((file, size))
    }

    fn rotate(&self) -> Option<(File, u64)> {
        let dir = self.dir.as_ref()?;
        let _ = fs::rename(dir.join(FILE), dir.join(OLD_FILE));
        self.open()
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let line = format!(
            "[{}][{}][{}] {}\n",
            timestamp(secs),
            record.target(),
            record.level(),
            record.args()
        );
        if self.stdout {
            print!("{line}");
        }
        let mut slot = self.file.lock();
        if slot.as_ref().is_some_and(|(_, size)| *size >= MAX_SIZE) {
            *slot = self.rotate();
        } else if slot.is_none() {
            *slot = self.open();
        }
        if let Some((file, size)) = slot.as_mut() {
            if file.write_all(line.as_bytes()).is_ok() {
                *size += line.len() as u64;
            }
        }
    }

    fn flush(&self) {
        if let Some((file, _)) = self.file.lock().as_mut() {
            let _ = file.flush();
        }
    }
}

/// `YYYY-MM-DD HH:MM:SS` in UTC, from seconds since the epoch.
fn timestamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// Installs the logger. Debug builds also print to stdout.
pub fn init() {
    let logger = Logger {
        file: Mutex::new(None),
        dir: crate::paths::log_dir(),
        stdout: cfg!(debug_assertions),
    };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(LevelFilter::Info);
    }
}

#[cfg(test)]
mod tests {
    use super::timestamp;

    #[test]
    fn formats_utc_dates() {
        assert_eq!(timestamp(0), "1970-01-01 00:00:00");
        assert_eq!(timestamp(951_782_400), "2000-02-29 00:00:00");
        assert_eq!(timestamp(1_793_885_405), "2026-11-05 13:30:05");
    }
}
