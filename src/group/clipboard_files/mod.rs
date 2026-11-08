//! Genuine native file-drop formats. Text containing a path is never an export.
use anyhow::Result;
use std::path::PathBuf;

pub(crate) mod provider;
use provider::{PublishedOffer, RemoteOffer};

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
    pub operation: crate::group::file_transfer::protocol::Operation,
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

pub(crate) fn publish_offer_if_current(
    offer: RemoteOffer, expected: u64,
) -> Result<Option<Box<dyn PublishedOffer>>> {
    #[cfg(windows)]
    return windows::publish_offer_if_current(offer, expected);
    #[cfg(target_os = "linux")]
    return linux::publish_offer_if_current(offer, expected);
    #[cfg(target_os = "macos")]
    return macos::publish_offer_if_current(offer, expected);
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    anyhow::bail!("File paste is unavailable. Use Send files.")
}

/// Recognizes our virtual names without opening or hydrating their content.
pub(crate) fn is_provider_path(path: &std::path::Path) -> bool {
    #[cfg(windows)]
    return windows::is_provider_path(path);
    #[cfg(target_os = "linux")]
    return linux::is_provider_path(path);
    #[cfg(not(any(windows, target_os = "linux")))]
    { let _ = path; false }
}

/// Optional native compare-and-clear. Never emulate this with a separate read
/// followed by an unconditional write: a newer local Copy must survive.
pub(crate) fn clear_cut_if_current(paths: &[PathBuf], expected: u64) -> Result<bool> {
    #[cfg(windows)]
    return windows::clear_cut_if_current(paths, expected);
    #[cfg(target_os = "linux")]
    return linux::clear_cut_if_current(paths, expected);
    #[cfg(not(any(windows, target_os = "linux")))]
    { let _ = (paths, expected); Ok(false) }
}
