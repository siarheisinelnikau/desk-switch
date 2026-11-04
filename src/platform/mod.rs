#[cfg(target_os = "linux")]
mod linux_monitor;
#[cfg(target_os = "linux")]
pub mod portal;
#[cfg(target_os = "linux")]
mod ubuntu;
#[cfg(target_os = "linux")]
pub use linux_monitor::probe;

#[cfg(windows)]
mod win_monitor;
#[cfg(windows)]
mod win_input;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use win_monitor::probe;

use crate::{config::{Identity, Key, Settings}, video::VideoWorker};
use anyhow::Result;
use std::net::SocketAddr;

pub fn run(address: SocketAddr, identity: Identity, peer: Key, settings: Settings) -> Result<()> {
    let video = settings.monitor.map(|(id, inputs)| {
        #[cfg(target_os = "linux")]
        let monitor = linux_monitor::Ddc(id);
        #[cfg(windows)]
        let monitor = win_monitor::Ddc(id);
        VideoWorker::start(monitor).map(|worker| (inputs, worker))
    }).transpose()?;

    #[cfg(target_os = "linux")]
    return crate::runtime()?.block_on(async {
        let portal = portal::Portal::open(settings.edge).await?;
        ubuntu::run(address, identity, peer, portal, video).await
    });

    #[cfg(windows)]
    windows::run(address, identity, peer, settings.edge, video)
}
