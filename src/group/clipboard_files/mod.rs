//! Genuine native file-drop formats. Text containing a path is never an export.
use anyhow::Result;
use std::path::PathBuf;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Selection {
    pub sequence: u64,
    pub paths: Vec<PathBuf>,
}

pub(crate) fn read() -> Result<Option<Selection>> {
    #[cfg(windows)]
    return windows::read();
    #[cfg(target_os = "linux")]
    return linux::read();
    #[cfg(target_os = "macos")]
    return macos::read();
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    Ok(None)
}
pub(crate) fn sequence() -> Option<u64> {
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn GetClipboardSequenceNumber() -> u32;
        }
        let sequence = unsafe { GetClipboardSequenceNumber() };
        (sequence != 0).then_some(u64::from(sequence))
    }
    #[cfg(target_os = "linux")]
    return linux::sequence();
    #[cfg(target_os = "macos")]
    return macos::sequence();
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    None
}
pub(crate) fn write_if_current(paths: &[PathBuf], expected: u64) -> Result<bool> {
    #[cfg(windows)]
    return windows::write_if_current(paths, expected);
    #[cfg(target_os = "linux")]
    return linux::write_if_current(paths, expected);
    #[cfg(target_os = "macos")]
    return macos::write_if_current(paths, expected);
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    anyhow::bail!("File clipboard sharing is unavailable on this operating system")
}
