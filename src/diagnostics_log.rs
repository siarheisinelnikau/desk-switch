//! Optional rotating diagnostics log.
//!
//! The in-memory report that **Copy diagnostics** produces only exists while
//! the application runs. A crash therefore loses the last service messages,
//! which is exactly when they are needed. This sink writes the same lines to
//! `<state-dir>/diagnostics.log` next to `panic-report.txt`.
//!
//! The log can contain computer names, file paths and network addresses, so
//! it stays off unless it is requested explicitly with
//! `DESK_SWITCH_DIAGNOSTICS_LOG=1`. The file is rotated at a fixed size and
//! never grows without bound.

use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const LOG_NAME: &str = "diagnostics.log";
const ROTATED_NAME: &str = "diagnostics.log.1";

static SINK: OnceLock<Mutex<Option<Sink>>> = OnceLock::new();

struct Sink {
    path: PathBuf,
    file: File,
    written: u64,
}

fn sink() -> &'static Mutex<Option<Sink>> {
    SINK.get_or_init(|| Mutex::new(None))
}

/// Opens the log when the environment asks for it. Calling this twice keeps
/// the first sink, so a second GUI start in the same process does not reopen
/// or truncate the file.
#[cfg_attr(test, allow(dead_code))]
pub fn enable_if_requested(dir: &Path) {
    let requested = std::env::var("DESK_SWITCH_DIAGNOSTICS_LOG")
        .map(|value| {
            let value = value.trim().to_ascii_lowercase();
            value == "1" || value == "true" || value == "yes"
        })
        .unwrap_or(false);
    if !requested {
        return;
    }
    let path = dir.join(LOG_NAME);
    let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let written = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let mut guard = sink()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.is_none() {
        *guard = Some(Sink {
            path,
            file,
            written,
        });
    }
}

/// Appends one line. The call never fails and never blocks on a slow disk for
/// longer than the write itself takes.
pub fn append(service: &str, line: &str) {
    let mut guard = sink()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(entry) = guard.as_mut() else {
        return;
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    let mut record = format!("{stamp} {service}: {line}\n");
    record.retain(|character| !character.is_control() || character == '\t');
    if entry.written + record.len() as u64 > MAX_BYTES {
        // Keep the previous file once, then start a fresh log.
        let _ = std::fs::rename(&entry.path, entry.path.with_file_name(ROTATED_NAME));
        if let Ok(file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&entry.path)
        {
            entry.file = file;
            entry.written = 0;
        } else {
            *guard = None;
            return;
        }
    }
    if entry.file.write_all(record.as_bytes()).is_ok() {
        entry.written += record.len() as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_requested_log_is_written_and_bounded() {
        let directory =
            std::env::temp_dir().join(format!("desk-switch-diagnostics-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        // The sink is process wide, so the test uses the private path directly.
        let path = directory.join(LOG_NAME);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        {
            let mut guard = sink().lock().unwrap();
            *guard = Some(Sink {
                path: path.clone(),
                file,
                written: 0,
            });
        }
        append("Input", "a message");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("Input: a message"), "{text}");
        sink().lock().unwrap().take();
        let _ = std::fs::remove_dir_all(&directory);
    }
}
