//! GitHub release discovery, verified downloads, Windows EXE and Debian updates.
//! No UI runtime, settings store, renderer, or application-specific CLI is required.

mod config;
mod controller;
#[cfg(any(windows, target_os = "linux"))]
mod download;
mod github;
#[cfg(windows)]
mod install;
#[cfg(target_os = "linux")]
#[path = "linux_install.rs"]
mod install;
#[cfg(not(any(windows, target_os = "linux")))]
#[path = "unsupported_install.rs"]
mod install;
mod service;
mod verify;

pub use config::{Architecture, Config, InstallMode, InstallOptions};
pub use controller::{Controller, Poll, State};
pub use github::{Release, check_latest, parse_release};
#[cfg(feature = "smoke-test")]
pub use install::smoke_cli;
pub use install::{
    Prepared, apply_legacy_update, apply_update, cleanup_after_update, handoff, installation_target,
};
pub use prepare_download as prepare;
pub use service::{Event, Service};
pub use verify::{
    checksum, hash, single_asset_checksum, verify_executable, verify_pe, verify_pe_subsystem,
};

/// Blocking download and verification for a host's existing worker thread.
/// Never call on the UI thread; installation still requires explicit handoff.
pub fn prepare_download(
    release: &Release,
    config: &Config,
    progress: impl FnMut(u64, u64),
) -> Result<Prepared, String> {
    install::prepare(release, config, progress)
}

/// Cooperative cancellation is observed before requests, between reads and before
/// making the prepared update available. A blocked network read retains its timeout.
pub fn prepare_download_cancellable(
    release: &Release,
    config: &Config,
    cancel: &std::sync::atomic::AtomicBool,
    progress: impl FnMut(u64, u64),
) -> Result<Prepared, String> {
    install::prepare_cancellable(release, config, cancel, progress)
}

pub use prepare_download_cancellable as prepare_download_with_cancel;

/// Unix time in seconds, useful for a host application's persisted check timestamp.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(all(test, windows))]
mod tests;
