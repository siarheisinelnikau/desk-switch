//! Explicit folder grants and USB ownership on a separate authenticated channel.
mod protocol;
mod service;
mod settings;
mod storage;
pub mod sync;
mod transport;
pub mod usb;
pub(crate) use super::file_transfer::protocol as safe;
pub use service::{Command, Service, Snapshot};
