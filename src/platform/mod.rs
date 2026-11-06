mod host;
#[cfg(target_os = "linux")]
mod linux_host;
#[cfg(target_os = "linux")]
mod linux_monitor;
#[cfg(target_os = "linux")]
pub mod portal;
mod receiver;
#[cfg(target_os = "linux")]
pub use linux_monitor::{discover, probe};
#[cfg(target_os = "linux")]
pub use linux_monitor::{list_displays, read_monitor};

#[cfg(windows)]
mod win_input;
#[cfg(windows)]
mod win_monitor;
#[cfg(windows)]
mod win_receive;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use win_monitor::{discover, probe};
#[cfg(windows)]
pub use win_monitor::{list_displays, read_monitor};

use crate::{
    config::{Identity, Key, Role, Settings},
    video::VideoWorker,
};
use anyhow::Result;
use std::net::SocketAddr;

pub fn run(
    role: Role,
    address: SocketAddr,
    identity: Identity,
    peer: Key,
    settings: Settings,
) -> Result<()> {
    let video = settings
        .monitor
        .map(|(id, inputs)| {
            #[cfg(target_os = "linux")]
            let monitor = linux_monitor::Ddc(id);
            #[cfg(windows)]
            let monitor = win_monitor::Ddc(id);
            let worker = if crate::lifecycle::managed() {
                VideoWorker::watch(monitor)
            } else {
                VideoWorker::start(monitor)
            };
            worker.map(|worker| (inputs, worker))
        })
        .transpose()?;
    match role {
        Role::Host => {
            #[cfg(target_os = "linux")]
            return crate::runtime()?.block_on(linux_host::run(
                address,
                identity,
                peer,
                settings.edge,
                settings.sharing,
                settings.capture_devices,
                video,
            ));
            #[cfg(windows)]
            windows::run(
                address,
                identity,
                peer,
                settings.edge,
                settings.sharing,
                video,
            )
        }
        Role::Receiver => crate::runtime()?.block_on(async {
            #[cfg(target_os = "linux")]
            {
                let mut sink = portal::Portal::open(settings.edge).await?;
                let result = receiver::run(address, identity, peer, &mut sink, video).await;
                sink.close().await;
                result
            }
            #[cfg(windows)]
            {
                let mut sink = win_receive::NativeSink::new(settings.edge)?;
                receiver::run(address, identity, peer, &mut sink, video).await
            }
        }),
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct MonitorInfo {
    pub id: String,
    pub name: String,
    pub screen: String,
    pub current: Option<u16>,
    pub inputs: Vec<u16>,
    pub error: Option<String>,
    #[serde(default)]
    pub fingerprint: String,
}
