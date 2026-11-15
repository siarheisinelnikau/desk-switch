//! USB/IP bridge to an externally installed, maintained USB/IP stack.
//! No driver installation, elevation, bind or firewall changes are implicit.
use super::{safe, transport};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{Shutdown, SocketAddr, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

const USBIP: u16 = 0x0111;
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Device {
    pub bus_id: String,
    pub name: String,
    pub fingerprint: String,
    pub descriptor: String,
    pub supported: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Grant {
    pub device: Device,
    pub peer: String,
}
pub fn bus_id(value: &str) -> Result<()> {
    let (bus, ports) = value.split_once('-').unwrap_or_default();
    let numeric = |part: &str| {
        !part.is_empty()
            && part.bytes().all(|b| b.is_ascii_digit())
            && part.parse::<u16>().is_ok_and(|v| v > 0)
    };
    ensure!(
        !value.is_empty()
            && value.len() < 32
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'-' || b == b'.')
            && numeric(bus)
            && ports.split('.').all(numeric),
        "Invalid USB bus ID"
    );
    Ok(())
}
fn socket(address: SocketAddr) -> Result<TcpStream> {
    ensure!(
        address.ip().is_loopback(),
        "USB backend must be on loopback"
    );
    let stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))
        .context("USB/IP backend is unavailable on localhost:3240")?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    Ok(stream)
}
fn header(op: u16) -> [u8; 8] {
    let mut bytes = [0; 8];
    bytes[..2].copy_from_slice(&USBIP.to_be_bytes());
    bytes[2..4].copy_from_slice(&op.to_be_bytes());
    bytes
}
fn response(stream: &mut impl Read, op: u16) -> Result<[u8; 8]> {
    let mut bytes = [0; 8];
    stream.read_exact(&mut bytes)?;
    ensure!(
        bytes[..2] == USBIP.to_be_bytes()
            && bytes[2..4] == op.to_be_bytes()
            && bytes[4..] == [0; 4],
        "USB/IP backend rejected the device or protocol"
    );
    Ok(bytes)
}
pub fn list(address: SocketAddr) -> Result<Vec<Device>> {
    let mut stream = socket(address)?;
    stream.write_all(&header(0x8005))?;
    parse_list(&mut stream)
}
fn parse_list(stream: &mut impl Read) -> Result<Vec<Device>> {
    response(stream, 0x0005)?;
    let mut count = [0; 4];
    stream.read_exact(&mut count)?;
    let count = u32::from_be_bytes(count);
    ensure!(count <= 256, "Too many exported USB devices");
    let mut devices = Vec::new();
    for _ in 0..count {
        let mut raw = [0; 312];
        stream.read_exact(&mut raw)?;
        let end = raw[256..288]
            .iter()
            .position(|b| *b == 0)
            .context("Invalid USB/IP bus ID")?;
        let id = std::str::from_utf8(&raw[256..256 + end])?.to_string();
        bus_id(&id)?;
        let mut interfaces = vec![0; usize::from(raw[311]) * 4];
        stream.read_exact(&mut interfaces)?;
        let mut classes = vec![raw[306]];
        classes.extend(interfaces.as_chunks::<4>().0.iter().map(|part| part[0]));
        classes.retain(|c| *c != 0);
        let supported =
            !classes.is_empty() && classes.iter().all(|c| matches!(c, 2 | 7 | 10 | 0xff));
        let vendor = u16::from_be_bytes(raw[300..302].try_into()?);
        let product = u16::from_be_bytes(raw[302..304].try_into()?);
        let mut fingerprint = raw.to_vec();
        fingerprint.extend(interfaces);
        devices.push(Device {
            bus_id: id,
            name: format!("{vendor:04x}:{product:04x}"),
            fingerprint: safe::hash(&fingerprint),
            descriptor: safe::hash(&raw),
            supported,
        });
    }
    Ok(devices)
}
pub fn backend_address() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 3240))
}
pub fn import_request(id: &str) -> Result<[u8; 40]> {
    bus_id(id)?;
    let mut request = [0; 40];
    request[..8].copy_from_slice(&header(0x8003));
    request[8..8 + id.len()].copy_from_slice(id.as_bytes());
    Ok(request)
}
/// Only a validated import for the approved bus ID may reach the raw daemon.
pub fn validate_import(request: &[u8; 40], id: &str) -> Result<()> {
    ensure!(
        *request == import_request(id)?,
        "USB tunnel requested another device or operation"
    );
    Ok(())
}

#[derive(Default)]
pub struct Owners {
    entries: Mutex<BTreeMap<String, Owner>>,
    sequence: AtomicU64,
}
struct Owner {
    peer: String,
    token: u64,
    disconnect: Box<dyn Fn() + Send + Sync>,
}
pub struct Lease {
    registry: Arc<Owners>,
    id: String,
    token: u64,
}
impl Owners {
    pub fn reserve(self: &Arc<Self>, id: &str, peer: &str, socket: TcpStream) -> Result<Lease> {
        self.claim(id, peer, move || {
            let _ = socket.shutdown(Shutdown::Both);
        })
    }
    fn claim(
        self: &Arc<Self>,
        id: &str,
        peer: &str,
        disconnect: impl Fn() + Send + Sync + 'static,
    ) -> Result<Lease> {
        bus_id(id)?;
        let mut entries = self.entries.lock().unwrap();
        ensure!(!entries.contains_key(id), "USB device already has an owner");
        let token = self.sequence.fetch_add(1, Ordering::Relaxed);
        entries.insert(
            id.into(),
            Owner {
                peer: peer.into(),
                token,
                disconnect: Box::new(disconnect),
            },
        );
        Ok(Lease {
            registry: self.clone(),
            id: id.into(),
            token,
        })
    }
    pub fn snapshot(&self) -> BTreeMap<String, String> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .map(|(id, owner)| (id.clone(), owner.peer.clone()))
            .collect()
    }
    pub fn detach(&self, id: &str) {
        if let Some(owner) = self.entries.lock().unwrap().get(id) {
            (owner.disconnect)();
        }
        // Keep the lease reserved until both forwarding directions have stopped.
    }
    pub fn close_untrusted(&self, permitted: impl Fn(&str) -> bool) {
        for owner in self.entries.lock().unwrap().values() {
            if !permitted(&owner.peer) {
                (owner.disconnect)();
            }
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut entries = self.registry.entries.lock().unwrap();
        if entries.get(&self.id).is_some_and(|v| v.token == self.token) {
            entries.remove(&self.id);
        }
    }
}

/// Closing either socket ends both directions. Authenticated empty frames keep
/// an idle device alive; no ownership is restored automatically after reconnect.
pub fn relay(
    connection: transport::Connection,
    mut local: TcpStream,
    cancel: Arc<AtomicBool>,
) -> Result<()> {
    let network = connection.socket()?;
    let shutdown = local.try_clone()?;
    let mut outgoing = local.try_clone()?;
    local.set_read_timeout(Some(Duration::from_secs(1)))?;
    local.set_write_timeout(Some(Duration::from_secs(5)))?;
    let (mut reader, mut writer) = connection.duplex()?;
    let stop = cancel.clone();
    let network_stop = network.try_clone()?;
    let local_stop = shutdown.try_clone()?;
    let send = std::thread::spawn(move || -> Result<()> {
        let result = (|| {
            let mut bytes = [0; 32 * 1024];
            while !stop.load(Ordering::Acquire) {
                match local.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(count) => writer.bytes(&bytes[..count])?,
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                        ) =>
                    {
                        writer.bytes(&[])?
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Ok(())
        })();
        let _ = network_stop.shutdown(Shutdown::Both);
        let _ = local_stop.shutdown(Shutdown::Both);
        result
    });
    let result = (|| {
        while !cancel.load(Ordering::Acquire) {
            let bytes = reader.bytes()?;
            if !bytes.is_empty() {
                outgoing.write_all(&bytes)?;
            }
        }
        Ok(())
    })();
    let _ = network.shutdown(Shutdown::Both);
    let _ = shutdown.shutdown(Shutdown::Both);
    let sent = send
        .join()
        .map_err(|_| anyhow::anyhow!("USB tunnel worker failed"))?;
    result.and(sent)
}
pub fn export_socket(id: &str) -> Result<TcpStream> {
    bus_id(id)?;
    socket(backend_address())
}
pub fn start_export(
    backend: &mut (impl Read + Write),
    device: &Device,
    request: &[u8; 40],
) -> Result<Vec<u8>> {
    validate_import(request, &device.bus_id)?;
    backend.write_all(request)?;
    let mut reply = response(backend, 0x0003)?.to_vec();
    let mut descriptor = [0; 312];
    backend.read_exact(&mut descriptor)?;
    ensure!(
        safe::hash(&descriptor) == device.descriptor,
        "USB device changed during attachment; share it again"
    );
    reply.extend(descriptor);
    Ok(reply)
}

#[cfg(target_os = "linux")]
fn command(args: &[String]) -> Result<()> {
    use std::{
        process::{Command, Stdio},
        time::Instant,
    };
    let executable = ["/usr/sbin/usbip", "/usr/bin/usbip", "/sbin/usbip"]
        .into_iter()
        .find(|p| std::path::Path::new(p).is_file())
        .context("Linux usbip client is not installed")?;
    let mut child = Command::new(executable)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stderr = child
        .stderr
        .take()
        .context("USB backend error stream unavailable")?;
    let error = std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut bytes = [0; 1024];
        while let Ok(count) = stderr.read(&mut bytes) {
            if count == 0 {
                break;
            }
            let take = count.min(8192_usize.saturating_sub(kept.len()));
            kept.extend(&bytes[..take]);
        }
        String::from_utf8_lossy(&kept).into_owned()
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() > Duration::from_secs(15) {
            let _ = child.kill();
            break child.wait()?;
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let error = error.join().unwrap_or_default();
    ensure!(
        status.success(),
        "USB/IP command failed. Check the installed tools, vhci_hcd and existing permissions: {}",
        error.trim()
    );
    Ok(())
}
#[cfg(target_os = "linux")]
pub fn attach(port: u16, id: &str) -> Result<()> {
    bus_id(id)?;
    command(&[
        "--tcp-port".into(),
        port.to_string(),
        "attach".into(),
        "--remote=127.0.0.1".into(),
        format!("--busid={id}"),
    ])
}
#[cfg(not(target_os = "linux"))]
pub fn attach(_port: u16, _id: &str) -> Result<()> {
    bail!(
        "USB import currently requires Ubuntu and the Linux usbip client; Windows can export through usbipd-win"
    )
}
#[cfg(target_os = "linux")]
pub fn detach_import(port: u16, id: &str) -> Result<()> {
    bus_id(id)?;
    let expected = format!("127.0.0.1 {port} {id}");
    for entry in std::fs::read_dir("/run/vhci_hcd").context(
        "USB/IP port record is unavailable; the closed tunnel has disconnected the device",
    )? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(number) = name
            .strip_prefix("port")
            .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        else {
            continue;
        };
        let record = std::fs::read_to_string(entry.path())?;
        if record.trim() == expected {
            return command(&["detach".into(), format!("--port={number}")]);
        }
    }
    bail!("USB/IP port record no longer belongs to this tunnel; no other device was detached")
}
#[cfg(not(target_os = "linux"))]
pub fn detach_import(_port: u16, _id: &str) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ownership_is_exclusive_and_released_only_after_cleanup() {
        let owners = Arc::new(Owners::default());
        let disconnected = Arc::new(AtomicBool::new(false));
        let signal = disconnected.clone();
        let lease = owners
            .claim("1-2", "alice", move || {
                signal.store(true, Ordering::Release);
            })
            .unwrap();
        assert!(owners.claim("1-2", "bob", || {}).is_err());
        owners.detach("1-2");
        assert!(disconnected.load(Ordering::Acquire));
        assert!(owners.claim("1-2", "bob", || {}).is_err());
        drop(lease);
        assert!(owners.claim("1-2", "bob", || {}).is_ok());
    }
    #[test]
    fn racing_owners_and_trust_revocation_keep_one_lease_until_cleanup() {
        let owners = Arc::new(Owners::default());
        let barrier = Arc::new(std::sync::Barrier::new(12));
        let disconnected = Arc::new(AtomicBool::new(false));
        let jobs: Vec<_> = (0..12)
            .map(|i| {
                let owners = owners.clone();
                let barrier = barrier.clone();
                let disconnected = disconnected.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    owners
                        .claim("1-2", &i.to_string(), move || {
                            disconnected.store(true, Ordering::Release);
                        })
                        .ok()
                })
            })
            .collect();
        let leases: Vec<_> = jobs
            .into_iter()
            .filter_map(|job| job.join().unwrap())
            .collect();
        assert_eq!(leases.len(), 1);
        owners.close_untrusted(|_| false);
        assert!(disconnected.load(Ordering::Acquire));
        assert_eq!(owners.snapshot().len(), 1);
        drop(leases);
        assert!(owners.snapshot().is_empty());
    }
    #[test]
    fn proxy_accepts_only_an_exact_import_and_never_a_device_list() {
        let mut request = import_request("1-2.3").unwrap();
        validate_import(&request, "1-2.3").unwrap();
        assert!(validate_import(&request, "1-2.4").is_err());
        request[3] = 5;
        assert!(validate_import(&request, "1-2.3").is_err());
        for id in ["--help", "1-2;shutdown", "1/2", "", "-x"] {
            assert!(bus_id(id).is_err());
        }
    }
    #[test]
    fn real_wire_device_list_filters_unsupported_device_classes() {
        let mut socket = Vec::new();
        socket.write_all(&header(0x0005)).unwrap();
        socket.write_all(&2_u32.to_be_bytes()).unwrap();
        for (id, class) in [(b"1-2", 7), (b"1-3", 8)] {
            let mut raw = [0; 312];
            raw[256..259].copy_from_slice(id);
            raw[306] = class;
            socket.write_all(&raw).unwrap();
        }
        let devices = parse_list(&mut std::io::Cursor::new(socket.clone())).unwrap();
        assert!(devices[0].supported);
        assert!(!devices[1].supported);
        assert_ne!(devices[0].fingerprint, devices[1].fingerprint);
        socket.truncate(20);
        assert!(parse_list(&mut std::io::Cursor::new(socket)).is_err());
    }
    #[test]
    fn import_verifies_backend_identity_and_never_forwards_another_device() {
        struct Backend {
            input: std::io::Cursor<Vec<u8>>,
            sent: Vec<u8>,
        }
        impl Read for Backend {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                self.input.read(bytes)
            }
        }
        impl Write for Backend {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.sent.extend(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut raw = [0; 312];
        raw[256..259].copy_from_slice(b"1-2");
        raw[306] = 7;
        let device = Device {
            bus_id: "1-2".into(),
            name: "Fixture printer".into(),
            descriptor: safe::hash(&raw),
            fingerprint: safe::hash(b"fixture interfaces"),
            supported: true,
        };
        let mut reply = header(3).to_vec();
        reply.extend(raw);
        let mut backend = Backend {
            input: std::io::Cursor::new(reply.clone()),
            sent: Vec::new(),
        };
        assert_eq!(
            start_export(&mut backend, &device, &import_request("1-2").unwrap()).unwrap(),
            reply
        );
        assert_eq!(backend.sent, import_request("1-2").unwrap());
        let mut wrong = device.clone();
        wrong.descriptor = safe::hash(b"another device");
        backend.input.set_position(0);
        assert!(start_export(&mut backend, &wrong, &import_request("1-2").unwrap()).is_err());
        backend.sent.clear();
        assert!(start_export(&mut backend, &device, &import_request("1-3").unwrap()).is_err());
        assert!(backend.sent.is_empty());
    }
}
