use anyhow::{Result, ensure};
use mdns_sd::{ResolvedService, ScopedIp, ServiceDaemon, ServiceEvent, ServiceInfo};
use std::{
    net::{SocketAddr, SocketAddrV6},
    sync::mpsc::{self, Receiver},
    time::{SystemTime, UNIX_EPOCH},
};

const SERVICE: &str = "_desk-switch._tcp.local.";

#[derive(Clone, Debug)]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub version: String,
    pub protocol: u16,
    pub role: String,
    pub addresses: Vec<SocketAddr>,
    pub audio_port: u16,
}
impl Peer {
    pub fn compatible(&self) -> bool {
        self.version == crate::compatibility::VERSION
            && self.protocol == crate::compatibility::PROTOCOL
    }
}

pub enum Event {
    Found(Peer),
    Removed(String),
    Error(String),
}

pub struct Discovery {
    daemon: ServiceDaemon,
    fullname: String,
    events: Receiver<Event>,
}

impl Discovery {
    pub fn start(
        name: &str,
        role: &str,
        port: u16,
        audio_port: u16,
        ctx: eframe::egui::Context,
    ) -> Result<Self> {
        ensure!(port != 0 && audio_port != 0, "invalid discovery ports");
        let daemon = ServiceDaemon::new()?;
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let id = format!("ds-{}-{stamp:x}", std::process::id());
        let hostname = format!("{id}.local.");
        let name = clean(name, 64);
        let properties = [
            ("app", "desk-switch".to_owned()),
            ("name", name),
            ("role", role.to_owned()),
            ("version", crate::compatibility::VERSION.to_owned()),
            ("protocol", crate::compatibility::PROTOCOL.to_string()),
            ("audio-port", audio_port.to_string()),
        ];
        let properties = properties
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect::<std::collections::HashMap<_, _>>();
        let service =
            ServiceInfo::new(SERVICE, &id, &hostname, "", port, properties)?.enable_addr_auto();
        let fullname = service.get_fullname().to_owned();
        daemon.register(service)?;
        let incoming = daemon.browse(SERVICE)?;
        let monitor = daemon.monitor()?;
        let (send, events) = mpsc::sync_channel(64);
        let own = fullname.clone();
        std::thread::Builder::new()
            .name("Desk Switch discovery".into())
            .spawn(move || {
                enum Incoming {
                    Browse(Result<ServiceEvent, flume::RecvError>),
                    Monitor(Result<mdns_sd::DaemonEvent, flume::RecvError>),
                }
                loop {
                    let event = flume::Selector::new()
                        .recv(&incoming, Incoming::Browse)
                        .recv(&monitor, Incoming::Monitor)
                        .wait();
                    let update = match event {
                        Incoming::Browse(Ok(ServiceEvent::ServiceResolved(info)))
                            if info.get_fullname() != own =>
                        {
                            parse(&info).map(Event::Found)
                        }
                        Incoming::Browse(Ok(ServiceEvent::ServiceRemoved(_, id))) => {
                            Some(Event::Removed(id))
                        }
                        Incoming::Monitor(Ok(mdns_sd::DaemonEvent::Error(error))) => {
                            Some(Event::Error(error.to_string()))
                        }
                        Incoming::Browse(Err(_)) | Incoming::Monitor(Err(_)) => break,
                        _ => None,
                    };
                    if let Some(update) = update {
                        match send.try_send(update) {
                            Ok(()) => ctx.request_repaint(),
                            Err(mpsc::TrySendError::Disconnected(_)) => break,
                            Err(mpsc::TrySendError::Full(_)) => {}
                        }
                    }
                }
            })?;
        Ok(Self {
            daemon,
            fullname,
            events,
        })
    }
    pub fn poll(&self) -> impl Iterator<Item = Event> + '_ {
        self.events.try_iter().take(64)
    }
}
impl Drop for Discovery {
    fn drop(&mut self) {
        let _ = self.daemon.stop_browse(SERVICE);
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

pub fn computer_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|value| clean(&value, 64))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "Windows PC"
            } else {
                "Ubuntu PC"
            }
            .into()
        })
}

fn clean(value: &str, limit: usize) -> String {
    value
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .take(limit)
        .collect()
}

pub fn selected_addresses(
    role: &str,
    audio_mode: &str,
    port: u16,
    audio_port: u16,
    peer: &Peer,
    address: SocketAddr,
) -> (SocketAddr, SocketAddr) {
    let unspecified = if address.is_ipv4() {
        std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)
    } else {
        std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED)
    };
    let control = if role == "host" {
        address
    } else {
        SocketAddr::new(
            unspecified,
            if peer.role == "host" {
                address.port()
            } else {
                port
            },
        )
    };
    let audio = if audio_mode == "receive" {
        let mut target = address;
        target.set_port(peer.audio_port);
        target
    } else {
        SocketAddr::new(unspecified, audio_port)
    };
    (control, audio)
}

fn parse(info: &ResolvedService) -> Option<Peer> {
    if info.get_property_val_str("app")? != "desk-switch" || info.get_port() == 0 {
        return None;
    }
    let version = info.get_property_val_str("version")?;
    if version.len() > 48 || semver::Version::parse(version).is_err() {
        return None;
    }
    let role = info.get_property_val_str("role")?;
    if !["host", "receiver"].contains(&role) {
        return None;
    }
    let protocol = info.get_property_val_str("protocol")?.parse().ok()?;
    let audio_port = info
        .get_property_val_str("audio-port")?
        .parse()
        .ok()
        .filter(|port| *port != 0)?;
    let mut addresses = info
        .get_addresses()
        .iter()
        .filter_map(|address| {
            let ip = address.to_ip_addr();
            if ip.is_unspecified() || ip.is_multicast() || ip.is_loopback() {
                return None;
            }
            match address {
                ScopedIp::V6(value) => Some(SocketAddr::V6(SocketAddrV6::new(
                    *value.addr(),
                    info.get_port(),
                    0,
                    value.scope_id().index,
                ))),
                _ => Some(SocketAddr::new(ip, info.get_port())),
            }
        })
        .collect::<Vec<_>>();
    addresses.sort_by_key(|address| (address.is_ipv6(), address.to_string()));
    addresses.dedup();
    addresses.truncate(8);
    if addresses.is_empty() {
        return None;
    }
    Some(Peer {
        id: info.get_fullname().to_owned(),
        name: clean(info.get_property_val_str("name")?, 64),
        version: version.into(),
        protocol,
        role: role.into(),
        addresses,
        audio_port,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn untrusted_records_are_bounded_and_never_pair_or_connect() {
        let properties = [
            ("app", "desk-switch"),
            ("name", "Fixture PC"),
            ("version", crate::compatibility::VERSION),
            ("protocol", "2"),
            ("role", "receiver"),
            ("audio-port", "45877"),
        ];
        let info = ServiceInfo::new(
            SERVICE,
            "fixture",
            "fixture.local.",
            "192.168.1.5,127.0.0.1,::1",
            45876,
            &properties[..],
        )
        .unwrap()
        .as_resolved_service();
        let peer = parse(&info).unwrap();
        assert!(peer.compatible());
        assert_eq!(
            peer.addresses,
            vec!["192.168.1.5:45876".parse::<SocketAddr>().unwrap()]
        );
        assert_eq!(peer.name, "Fixture PC");
        let invalid = ServiceInfo::new(
            SERVICE,
            "bad",
            "bad.local.",
            "192.168.1.7",
            0,
            &properties[..],
        )
        .unwrap()
        .as_resolved_service();
        assert!(parse(&invalid).is_none());
        assert_eq!(clean("\nA\u{0}BCDE", 3), "ABC");
    }

    #[test]
    fn choosing_an_endpoint_keeps_server_and_client_addresses_separate() {
        let address = SocketAddr::V6(SocketAddrV6::new("fe80::5".parse().unwrap(), 47000, 0, 7));
        assert_eq!(address.to_string().parse::<SocketAddr>().unwrap(), address);
        let peer = Peer {
            id: "fixture".into(),
            name: "PC".into(),
            version: crate::compatibility::VERSION.into(),
            protocol: 2,
            role: "host".into(),
            addresses: vec![address],
            audio_port: 47001,
        };
        let (control, audio) = selected_addresses("host", "receive", 45876, 45877, &peer, address);
        assert_eq!(control, address);
        assert_eq!(audio.port(), 47001);
        assert_eq!(audio.ip(), address.ip());
        let (control, audio) = selected_addresses("receiver", "send", 45876, 45877, &peer, address);
        assert!(control.ip().is_unspecified());
        assert_eq!(control.port(), 47000);
        assert!(audio.ip().is_unspecified());
        assert_eq!(audio.port(), 45877);
    }
}
