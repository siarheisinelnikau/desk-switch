use anyhow::{Result, ensure};
use mdns_sd::{ResolvedService, ScopedIp, ServiceDaemon, ServiceEvent, ServiceInfo};
use std::{
    net::{SocketAddr, SocketAddrV6},
    sync::mpsc::{self, Receiver},
};

const SERVICE: &str = "_desk-switch._tcp.local.";

#[derive(Clone, Debug)]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub version: String,
    pub protocol: u16,
    pub addresses: Vec<SocketAddr>,
    pub public_key: Option<String>,
    pub control_port: Option<u16>,
}
impl Peer {
    pub fn compatible(&self) -> bool {
        self.version == crate::compatibility::VERSION
            && self.protocol == crate::compatibility::PROTOCOL
            && self.public_key.is_some()
            && self.control_port.is_some()
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
        public_key: &str,
        control_port: u16,
        ctx: eframe::egui::Context,
    ) -> Result<Self> {
        ensure!(
            control_port != 0 && public_key.len() == 64,
            "invalid discovery endpoint"
        );
        let daemon = ServiceDaemon::new()?;
        let id = format!("ds-{}", &public_key[..24]);
        let hostname = format!("{id}.local.");
        let name = clean(name, 64);
        let properties = [
            ("app", "desk-switch".to_owned()),
            ("name", name),
            ("role", "receiver".to_owned()),
            ("version", crate::compatibility::VERSION.to_owned()),
            ("protocol", crate::compatibility::PROTOCOL.to_string()),
            ("audio-port", crate::workspace::AUDIO_PORT.to_string()),
            ("control-port", control_port.to_string()),
            ("key", public_key.to_owned()),
        ];
        let properties = properties
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect::<std::collections::HashMap<_, _>>();
        let service = ServiceInfo::new(
            SERVICE,
            &id,
            &hostname,
            "",
            crate::workspace::INPUT_PORT,
            properties,
        )?
        .enable_addr_auto();
        let fullname = service.get_fullname().to_owned();
        daemon.register(service)?;
        let incoming = daemon.browse(SERVICE)?;
        let monitor = daemon.monitor()?;
        let (send, events) = mpsc::sync_channel(64);
        let own = fullname.clone();
        let own_key = public_key.to_owned();
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
                            if info.get_fullname() != own
                                && info.get_property_val_str("key") != Some(own_key.as_str()) =>
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

fn parse(info: &ResolvedService) -> Option<Peer> {
    if info.get_property_val_str("app")? != "desk-switch" || info.get_port() == 0 {
        return None;
    }
    let version = info.get_property_val_str("version")?;
    if version.is_empty()
        || version.len() > 48
        || !version.is_ascii()
        || version.bytes().any(|b| b.is_ascii_control())
    {
        return None;
    }
    let protocol = info.get_property_val_str("protocol")?.parse().ok()?;
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
        addresses,
        public_key: info
            .get_property_val_str("key")
            .filter(|key| {
                key.len() == 64
                    && key.bytes().all(|b| b.is_ascii_hexdigit())
                    && *key != "0".repeat(64)
            })
            .map(str::to_ascii_lowercase),
        control_port: info
            .get_property_val_str("control-port")
            .and_then(|s| s.parse().ok())
            .filter(|p| *p != 0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn untrusted_records_are_bounded_and_never_pair_or_connect() {
        let protocol = crate::compatibility::PROTOCOL.to_string();
        let properties = [
            ("app", "desk-switch"),
            ("name", "Fixture PC"),
            ("version", crate::compatibility::VERSION),
            ("protocol", protocol.as_str()),
            (
                "key",
                "1111111111111111111111111111111111111111111111111111111111111111",
            ),
            ("control-port", "45878"),
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
    fn other_protocols_and_unknown_roles_remain_visible() {
        for protocol in ["1", "3", "99"] {
            let properties = [
                ("app", "desk-switch"),
                ("name", "Future PC"),
                ("version", "99.1.0"),
                ("protocol", protocol),
                ("role", "future-role"),
            ];
            let record = ServiceInfo::new(
                SERVICE,
                "other",
                "other.local.",
                "192.168.1.7",
                45876,
                &properties[..],
            )
            .unwrap()
            .as_resolved_service();
            let peer = parse(&record).unwrap();
            assert!(!peer.compatible());
            assert_eq!(peer.protocol, protocol.parse::<u16>().unwrap());
        }
    }
}
