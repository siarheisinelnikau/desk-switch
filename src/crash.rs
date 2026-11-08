use std::{
    backtrace::Backtrace,
    fs::{self, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    panic::{self, AssertUnwindSafe, PanicHookInfo},
    path::Path,
};

const REPORT_FILE: &str = "panic-report.txt";
const MAX_REPORT_BYTES: u64 = 256 * 1024;

#[cfg_attr(test, allow(dead_code))]
pub fn install(dir: &Path) {
    let dir = dir.to_path_buf();
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            let report = format_report(info);
            fs::create_dir_all(&dir)
                .and_then(|_| append_report(&dir.join(REPORT_FILE), &report))
                .ok();
        }));
        previous(info);
    }));
}

#[allow(dead_code)]
pub fn take_report(dir: &Path) -> Option<String> {
    let path = dir.join(REPORT_FILE);
    let bytes = fs::read(&path).ok()?;
    fs::remove_file(path).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The single-line summary of the last crash, for a short user-facing notice.
#[allow(dead_code)]
pub fn report_summary(report: &str) -> Option<&str> {
    report
        .lines()
        .find_map(|line| line.strip_prefix("Message: "))
        .map(str::trim)
        .filter(|message| !message.is_empty())
}

fn format_report(info: &PanicHookInfo<'_>) -> String {
    let message = info
        .payload()
        .downcast_ref::<&'static str>()
        .copied()
        .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
        .unwrap_or("non-string panic payload");
    let location = info
        .location()
        .map(ToString::to_string)
        .unwrap_or_else(|| "unknown".into());
    format!(
        "\n--- Desk Switch panic ---\nMessage: {message}\nLocation: {location}\nBacktrace:\n{}\n",
        Backtrace::force_capture()
    )
}

fn append_report(path: &Path, report: &str) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.seek(SeekFrom::End(0))?;
    file.write_all(report.as_bytes())?;
    let len = file.stream_position()?;
    if len > MAX_REPORT_BYTES {
        file.seek(SeekFrom::Start(len - MAX_REPORT_BYTES))?;
        let mut newest = Vec::new();
        (&mut file)
            .take(MAX_REPORT_BYTES)
            .read_to_end(&mut newest)?;
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&newest)?;
    }
    file.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_keep_the_newest_bounded_data_and_are_taken_once() {
        let dir = std::env::temp_dir().join(format!("desk-switch-crash-{}", std::process::id()));
        let path = dir.join(REPORT_FILE);
        fs::create_dir_all(&dir).unwrap();
        append_report(&path, &"x".repeat(MAX_REPORT_BYTES as usize + 128)).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().len(), MAX_REPORT_BYTES);
        append_report(&path, "newest report").unwrap();
        let report = take_report(&dir).unwrap();
        assert!(report.ends_with("newest report"));
        assert!(report.len() <= MAX_REPORT_BYTES as usize);
        assert!(take_report(&dir).is_none());
        fs::remove_dir_all(dir).unwrap();
    }
}
