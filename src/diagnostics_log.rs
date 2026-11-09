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
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const LOG_NAME: &str = "diagnostics.log";
const ROTATED_NAME: &str = "diagnostics.log.1";

static SINK: OnceLock<Mutex<Option<Sink>>> = OnceLock::new();

struct Sink {
    path: PathBuf,
    current: Option<LogFile>,
}

struct LogFile {
    file: File,
    written: u64,
    needs_separator: bool,
}

impl Sink {
    fn new(dir: &Path) -> Self {
        Self {
            path: dir.join(LOG_NAME),
            current: None,
        }
    }

    fn open(&self) -> io::Result<LogFile> {
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .create(true)
            .open(&self.path)?;
        let written = file.metadata()?.len();
        let mut needs_separator = false;
        if written > 0 {
            // Older versions left records without a terminating newline.
            let mut last = [0];
            file.seek(SeekFrom::End(-1))?;
            file.read_exact(&mut last)?;
            needs_separator = last[0] != b'\n';
        }
        Ok(LogFile {
            file,
            written,
            needs_separator,
        })
    }

    fn write(&mut self, line: &str) -> io::Result<()> {
        // Reopen after an I/O error to account for possible partial writes.
        let mut log = match self.current.take() {
            Some(log) => log,
            None => self.open()?,
        };
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_secs())
            .unwrap_or(0);
        let mut record = format!("{stamp} {line}");
        record.retain(|character| !character.is_control() || character == '\t');
        record.truncate(record.floor_char_boundary(MAX_BYTES as usize - 1));
        record.push('\n');

        let size = record.len() as u64 + u64::from(log.needs_separator);
        if log.written + size > MAX_BYTES {
            // Keep the old file intact and retry rotation on the next record if it fails.
            std::fs::rename(&self.path, self.path.with_file_name(ROTATED_NAME))?;
            log = self.open()?;
        }
        if log.needs_separator {
            record.insert(0, '\n');
        }
        log.file.write_all(record.as_bytes())?;
        log.written += record.len() as u64;
        log.needs_separator = false;
        self.current = Some(log);
        Ok(())
    }
}

fn sink() -> &'static Mutex<Option<Sink>> {
    SINK.get_or_init(|| Mutex::new(None))
}

/// Enables logging for the active profile when the environment asks for it.
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
    let mut guard = sink()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.is_none() {
        *guard = Some(Sink::new(dir));
    }
}

/// Appends an already formatted service message when logging is enabled.
pub fn append(line: &str) {
    let mut guard = sink()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(entry) = guard.as_mut() {
        let _ = entry.write(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_directory(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "desk-switch-diagnostics-{}-{name}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn separates_new_records_and_an_existing_unterminated_log() {
        let directory = test_directory("lines");
        let mut log = Sink::new(&directory);
        fs::write(&log.path, "old record").unwrap();
        log.write("Input: first\nmessage").unwrap();
        log.write("Audio: second\tmessage\r").unwrap();
        let text = fs::read_to_string(&log.path).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text:?}");
        assert_eq!(lines[0], "old record");
        assert!(lines[1].ends_with(" Input: firstmessage"));
        assert!(lines[2].ends_with(" Audio: second\tmessage"));
        assert!(text.ends_with('\n'));
        drop(log.current.take());
        fs::remove_file(log.path).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn stays_bounded_when_rotation_fails_and_recovers_afterward() {
        let directory = test_directory("rotation");
        let rotated = directory.join(ROTATED_NAME);
        fs::create_dir(&rotated).unwrap();
        let mut log = Sink::new(&directory);
        fs::write(&log.path, vec![b'x'; MAX_BYTES as usize]).unwrap();
        assert!(log.write("Input: blocked").is_err());
        assert!(log.write("Input: still blocked").is_err());
        assert_eq!(fs::metadata(&log.path).unwrap().len(), MAX_BYTES);

        fs::remove_dir(&rotated).unwrap();
        log.write("Input: recovered").unwrap();
        assert!(
            fs::read_to_string(&log.path)
                .unwrap()
                .ends_with(" Input: recovered\n")
        );
        assert_eq!(fs::metadata(&rotated).unwrap().len(), MAX_BYTES);

        log.write(&"ж".repeat(MAX_BYTES as usize)).unwrap();
        let text = fs::read_to_string(&log.path).unwrap();
        assert!(text.len() as u64 <= MAX_BYTES);
        assert!(text.ends_with('\n'));
        assert!(
            fs::read_to_string(&rotated)
                .unwrap()
                .ends_with(" Input: recovered\n")
        );
        drop(log.current.take());
        fs::remove_file(log.path).unwrap();
        fs::remove_file(rotated).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
