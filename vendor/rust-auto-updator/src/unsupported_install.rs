use crate::{Config, InstallOptions, Release};
use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

const UNSUPPORTED: &str =
    "In-place installation currently supports portable Windows executables only.";

pub struct Prepared;

impl Prepared {
    pub fn set_restart_args(&mut self, _: Vec<String>) -> Result<(), String> {
        Err(UNSUPPORTED.into())
    }
}

pub(crate) fn prepare(
    _: &Release,
    _: &Config,
    _: impl FnMut(u64, u64),
) -> Result<Prepared, String> {
    Err(UNSUPPORTED.into())
}
pub(crate) fn prepare_cancellable(
    _: &Release,
    _: &Config,
    _: &AtomicBool,
    _: impl FnMut(u64, u64),
) -> Result<Prepared, String> {
    Err(UNSUPPORTED.into())
}
pub fn handoff(_: &mut Prepared) -> Result<(), String> {
    Err(UNSUPPORTED.into())
}
pub fn apply_update(_: &Config) -> Result<(), String> {
    Err(UNSUPPORTED.into())
}
pub fn installation_target(_: &Config) -> Result<PathBuf, String> {
    Err(UNSUPPORTED.into())
}
pub fn apply_legacy_update(
    _: &Config,
    _: &Path,
    _: &Path,
    _: &str,
    _: u32,
    _: Option<&Path>,
) -> Result<(), String> {
    Err(UNSUPPORTED.into())
}
pub fn cleanup_after_update(_: PathBuf, _: InstallOptions) {}
#[cfg(feature = "smoke-test")]
pub fn smoke_cli(_: &[std::ffi::OsString], _: &Config) -> Result<bool, String> {
    Err(UNSUPPORTED.into())
}
