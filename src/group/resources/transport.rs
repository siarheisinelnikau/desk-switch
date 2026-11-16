//! Pinned Noise KK, workspace binding, bounded records and domain separation.
//! This channel adds capabilities without changing legacy input/file protocols.
use crate::config::{self, Identity};
use anyhow::{Context, Result, ensure};
use bincode::Options;
use serde::{Serialize, de::DeserializeOwned};
use snow::{Builder, TransportState};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const MAGIC: &[u8; 8] = b"DESKRES1";
const MAX: usize = 8 * 1024 * 1024;
const RECORD: usize = 48 * 1024;
pub const VERSION: u16 = 1;
pub struct Connection {
    stream: TcpStream,
    noise: TransportState,
}
pub struct Reader {
    stream: TcpStream,
    noise: Arc<Mutex<TransportState>>,
}
pub struct Writer {
    stream: TcpStream,
    noise: Arc<Mutex<TransportState>>,
}
/// A deadline covers an entire packet, including slow partial socket reads.
struct Deadline<'a> {
    stream: &'a mut TcpStream,
    until: Instant,
}
impl Deadline<'_> {
    fn remaining(&self) -> std::io::Result<Duration> {
        self.until
            .checked_duration_since(Instant::now())
            .filter(|v| !v.is_zero())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Resource I/O deadline elapsed",
                )
            })
    }
}
impl Read for Deadline<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        self.stream.read(bytes)
    }
}
impl Write for Deadline<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        self.stream.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

fn codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_limit(MAX as u64)
        .reject_trailing_bytes()
}
fn record_write(stream: &mut impl Write, bytes: &[u8]) -> Result<()> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= RECORD + 16,
        "Invalid resource record"
    );
    let mut frame = Vec::with_capacity(bytes.len() + 2);
    frame.extend((bytes.len() as u16).to_be_bytes());
    frame.extend(bytes);
    stream.write_all(&frame)?;
    Ok(())
}
fn record_read(stream: &mut impl Read) -> Result<Vec<u8>> {
    let mut size = [0; 2];
    stream.read_exact(&mut size)?;
    let size = u16::from_be_bytes(size) as usize;
    ensure!(
        (16..=RECORD + 16).contains(&size),
        "Invalid resource record"
    );
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes)?;
    Ok(bytes)
}
fn authenticate(
    mut stream: TcpStream,
    identity: &Identity,
    peer: &str,
    workspace: &str,
    initiator: bool,
) -> Result<Connection> {
    ensure!(
        (16..=128).contains(&workspace.len())
            && workspace
                .bytes()
                .all(|v| v.is_ascii_hexdigit() || v == b'-'),
        "Invalid resource workspace"
    );
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let noise = handshake(
        &mut Deadline {
            stream: &mut stream,
            until: Instant::now() + Duration::from_secs(5),
        },
        identity,
        peer,
        workspace,
        initiator,
    )?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    Ok(Connection { stream, noise })
}
fn handshake(
    stream: &mut (impl Read + Write),
    identity: &Identity,
    peer: &str,
    workspace: &str,
    initiator: bool,
) -> Result<TransportState> {
    let prologue = format!("Desk Switch resources {VERSION}\0{workspace}");
    let peer = config::key(peer)?;
    let builder = Builder::new(config::NOISE.parse()?)
        .prologue(prologue.as_bytes())?
        .local_private_key(&identity.private)?
        .remote_public_key(&peer)?;
    let mut noise = if initiator {
        builder.build_initiator()?
    } else {
        builder.build_responder()?
    };
    for send in [initiator, !initiator] {
        let mut bytes = [0; 256];
        if send {
            let len = noise.write_message(&VERSION.to_be_bytes(), &mut bytes)?;
            record_write(stream, &bytes[..len])?;
        } else {
            let input = record_read(stream)?;
            let len = noise.read_message(&input, &mut bytes)?;
            ensure!(
                bytes[..len] == VERSION.to_be_bytes(),
                "Unsupported folder/USB protocol; update both computers"
            );
        }
    }
    Ok(noise.into_transport_mode()?)
}
pub fn connect(
    address: SocketAddr,
    identity: &Identity,
    peer: &str,
    workspace: &str,
) -> Result<Connection> {
    ensure!(workspace.len() <= 128, "Invalid resource workspace");
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(4)).context(
        "Folder/USB sharing is unavailable; update both computers and check the connection",
    )?;
    let mut greeting = Deadline {
        stream: &mut stream,
        until: Instant::now() + Duration::from_secs(5),
    };
    greeting.write_all(MAGIC)?;
    greeting.write_all(&identity.public)?;
    greeting.write_all(&(workspace.len() as u16).to_be_bytes())?;
    greeting.write_all(workspace.as_bytes())?;
    authenticate(stream, identity, peer, workspace, true)
}
pub fn accept(
    mut stream: TcpStream,
    identity: &Identity,
    workspace: &str,
    peers: &BTreeSet<String>,
) -> Result<(String, Connection)> {
    // Windows accepts can inherit a nonblocking listener's mode. Our packet
    // deadlines require blocking reads; an incomplete greeting must wait for
    // its remaining bytes instead of aborting an otherwise valid connection.
    stream.set_nonblocking(false)?;
    let mut greeting = Deadline {
        stream: &mut stream,
        until: Instant::now() + Duration::from_secs(5),
    };
    let mut magic = [0; 8];
    greeting.read_exact(&mut magic)?;
    ensure!(&magic == MAGIC, "Invalid resource greeting");
    let mut key = [0; 32];
    greeting.read_exact(&mut key)?;
    let peer = config::hex(&key);
    ensure!(peers.contains(&peer), "Untrusted resource peer");
    let mut length = [0; 2];
    greeting.read_exact(&mut length)?;
    let length = u16::from_be_bytes(length) as usize;
    ensure!((16..=128).contains(&length), "Invalid resource workspace");
    let mut bytes = vec![0; length];
    greeting.read_exact(&mut bytes)?;
    ensure!(bytes == workspace.as_bytes(), "Resource workspace mismatch");
    Ok((
        peer.clone(),
        authenticate(stream, identity, &peer, workspace, false)?,
    ))
}

fn send(stream: &mut impl Write, noise: &mut TransportState, value: &impl Serialize) -> Result<()> {
    let bytes = codec().serialize(value)?;
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX,
        "Resource packet too large"
    );
    // The total length is encrypted as part of the first record.
    let mut input = Vec::with_capacity(bytes.len() + 4);
    input.extend((bytes.len() as u32).to_be_bytes());
    input.extend(bytes);
    for part in input.chunks(RECORD) {
        let mut output = vec![0; part.len() + 16];
        let len = noise.write_message(part, &mut output)?;
        record_write(stream, &output[..len])?;
    }
    Ok(())
}
fn receive<T: DeserializeOwned>(stream: &mut impl Read, noise: &mut TransportState) -> Result<T> {
    let input = record_read(stream)?;
    let mut plain = vec![0; RECORD];
    let len = noise.read_message(&input, &mut plain)?;
    ensure!(len > 4, "Invalid resource packet");
    let size = u32::from_be_bytes(plain[..4].try_into()?) as usize;
    ensure!((1..=MAX).contains(&size), "Resource packet too large");
    let mut bytes = plain[4..len].to_vec();
    while bytes.len() < size {
        let input = record_read(stream)?;
        let len = noise.read_message(&input, &mut plain)?;
        ensure!(
            len > 0 && bytes.len() + len <= size,
            "Invalid resource packet length"
        );
        bytes.extend(&plain[..len]);
    }
    ensure!(bytes.len() == size, "Invalid resource packet length");
    Ok(codec().deserialize(&bytes)?)
}
impl Connection {
    pub fn send(&mut self, value: &impl Serialize) -> Result<()> {
        send(
            &mut Deadline {
                stream: &mut self.stream,
                until: Instant::now() + Duration::from_secs(30),
            },
            &mut self.noise,
            value,
        )
    }
    pub fn receive<T: DeserializeOwned>(&mut self) -> Result<T> {
        receive(
            &mut Deadline {
                stream: &mut self.stream,
                until: Instant::now() + Duration::from_secs(30),
            },
            &mut self.noise,
        )
    }
    pub fn socket(&self) -> Result<TcpStream> {
        Ok(self.stream.try_clone()?)
    }
    pub fn duplex(self) -> Result<(Reader, Writer)> {
        let noise = Arc::new(Mutex::new(self.noise));
        Ok((
            Reader {
                stream: self.stream.try_clone()?,
                noise: noise.clone(),
            },
            Writer {
                stream: self.stream,
                noise,
            },
        ))
    }
}
// USB relays use one-record packets, releasing Noise locks before blocking I/O.
impl Writer {
    pub fn bytes(&mut self, bytes: &[u8]) -> Result<()> {
        ensure!(bytes.len() < RECORD, "USB tunnel chunk too large");
        let mut input = Vec::with_capacity(bytes.len() + 1);
        input.push(1);
        input.extend(bytes);
        let mut encrypted = vec![0; input.len() + 16];
        let len = self
            .noise
            .lock()
            .unwrap()
            .write_message(&input, &mut encrypted)?;
        record_write(&mut self.stream, &encrypted[..len])
    }
}
impl Reader {
    pub fn bytes(&mut self) -> Result<Vec<u8>> {
        let encrypted = record_read(&mut self.stream)?;
        let mut bytes = vec![0; RECORD];
        let len = self
            .noise
            .lock()
            .unwrap()
            .read_message(&encrypted, &mut bytes)?;
        ensure!(len > 0 && bytes[0] == 1, "Invalid USB tunnel packet");
        Ok(bytes[1..len].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Cursor,
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
    };
    struct Memory {
        send: mpsc::Sender<Vec<u8>>,
        receive: mpsc::Receiver<Vec<u8>>,
        pending: Cursor<Vec<u8>>,
    }
    impl Read for Memory {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            if bytes.is_empty() {
                return Ok(0);
            }
            if self.pending.position() == self.pending.get_ref().len() as u64 {
                self.pending = Cursor::new(
                    self.receive
                        .recv_timeout(Duration::from_secs(1))
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, e))?,
                );
            }
            self.pending.read(bytes)
        }
    }
    impl Write for Memory {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.send
                .send(bytes.to_vec())
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    fn pair() -> (Memory, Memory) {
        let (a, b) = mpsc::channel();
        let (c, d) = mpsc::channel();
        (
            Memory {
                send: a,
                receive: d,
                pending: Cursor::new(Vec::new()),
            },
            Memory {
                send: c,
                receive: b,
                pending: Cursor::new(Vec::new()),
            },
        )
    }
    fn identity() -> Identity {
        let keys = Builder::new(config::NOISE.parse().unwrap())
            .generate_keypair()
            .unwrap();
        Identity {
            private: keys.private.try_into().unwrap(),
            public: keys.public.try_into().unwrap(),
        }
    }
    fn states(wrong_peer: bool, wrong_workspace: bool) -> Result<(TransportState, TransportState)> {
        let alice = identity();
        let bob = identity();
        let other = identity();
        let (mut a, mut b) = pair();
        let a_key = config::hex(&alice.public);
        let b_key = config::hex(&bob.public);
        let server = std::thread::spawn(move || {
            handshake(
                &mut b,
                &bob,
                &a_key,
                if wrong_workspace {
                    "bb11223344556677"
                } else {
                    "aa11223344556677"
                },
                false,
            )
        });
        let other_key = config::hex(&other.public);
        let client = handshake(
            &mut a,
            &alice,
            if wrong_peer { &other_key } else { &b_key },
            "aa11223344556677",
            true,
        );
        drop(a);
        let server = server.join().unwrap();
        Ok((client?, server?))
    }
    #[test]
    fn pinned_authentication_rejects_another_identity_and_workspace_without_network() {
        states(false, false).unwrap();
        assert!(states(true, false).is_err());
        assert!(states(false, true).is_err());
    }
    #[test]
    fn multi_record_frames_round_trip_and_replays_and_reordering_fail_authentication() {
        let (mut a, mut b) = states(false, false).unwrap();
        let value = vec![42_u8; 200_000];
        let mut bytes = Vec::new();
        send(&mut bytes, &mut a, &value).unwrap();
        assert_eq!(
            receive::<Vec<u8>>(&mut Cursor::new(bytes.clone()), &mut b).unwrap(),
            value
        );
        assert!(receive::<Vec<u8>>(&mut Cursor::new(bytes.clone()), &mut b).is_err());
        let (_, mut fresh) = states(false, false).unwrap();
        // Wrong session keys must fail even when packet bytes and sizes match.
        assert!(receive::<Vec<u8>>(&mut Cursor::new(bytes.clone()), &mut fresh).is_err());
        let (mut a, mut b) = states(false, false).unwrap();
        let mut bytes = Vec::new();
        send(&mut bytes, &mut a, &value).unwrap();
        let first = 2 + u16::from_be_bytes(bytes[..2].try_into().unwrap()) as usize;
        let second =
            first + 2 + u16::from_be_bytes(bytes[first..first + 2].try_into().unwrap()) as usize;
        let reordered = [&bytes[first..second], &bytes[..first], &bytes[second..]].concat();
        assert!(receive::<Vec<u8>>(&mut Cursor::new(reordered), &mut b).is_err());
    }
    #[test]
    fn invalid_record_and_packet_lengths_and_truncation_are_bounded() {
        assert!(record_read(&mut Cursor::new(vec![255, 255])).is_err());
        let (mut a, mut b) = states(false, false).unwrap();
        let mut plain = ((MAX + 1) as u32).to_be_bytes().to_vec();
        plain.push(1);
        let mut encrypted = vec![0; plain.len() + 16];
        let count = a.write_message(&plain, &mut encrypted).unwrap();
        let mut bytes = Vec::new();
        record_write(&mut bytes, &encrypted[..count]).unwrap();
        assert!(receive::<Vec<u8>>(&mut Cursor::new(bytes), &mut b).is_err());
        let (mut a, mut b) = states(false, false).unwrap();
        let mut bytes = Vec::new();
        send(&mut bytes, &mut a, &vec![1_u8; 90_000]).unwrap();
        bytes.truncate(bytes.len() - 1);
        assert!(receive::<Vec<u8>>(&mut Cursor::new(bytes), &mut b).is_err());
    }
    fn loopback(allow_peer: bool, workspace: &str) -> (Result<()>, Result<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let alice = identity();
        let bob = identity();
        let a = config::hex(&alice.public);
        let b = config::hex(&bob.public);
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = stop.clone();
        let server = std::thread::spawn(move || -> Result<()> {
            let deadline = Instant::now() + Duration::from_secs(5);
            let socket = loop {
                ensure!(
                    !cancel.load(Ordering::Acquire) && Instant::now() < deadline,
                    "Loopback fixture stopped before accepting a client"
                );
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => return Err(error.into()),
                }
            };
            let peers = if allow_peer {
                BTreeSet::from([a])
            } else {
                BTreeSet::new()
            };
            let (_, mut connection) = accept(socket, &bob, "aa11223344556677", &peers)?;
            ensure!(
                connection.receive::<String>()? == "fixture",
                "Unexpected fixture request"
            );
            connection.send(&"ack")
        });
        let client = (|| -> Result<()> {
            let mut connection = connect(address, &alice, &b, workspace)?;
            connection.send(&"fixture")?;
            ensure!(
                connection.receive::<String>()? == "ack",
                "Unexpected fixture response"
            );
            Ok(())
        })();
        // Join on both success and failure. A failed connect must not leave an
        // accept thread blocked forever or skip its authentication result.
        stop.store(true, Ordering::Release);
        (client, server.join().unwrap())
    }
    #[test]
    fn loopback_channel_authenticates_only_pinned_members() {
        let (client, server) = loopback(true, "aa11223344556677");
        client.unwrap();
        server.unwrap();
    }
    #[test]
    fn loopback_greeting_rejects_an_untrusted_peer_and_another_workspace() {
        for (allow_peer, workspace, expected) in [
            (false, "aa11223344556677", "Untrusted resource peer"),
            (true, "bb11223344556677", "Resource workspace mismatch"),
        ] {
            let (client, server) = loopback(allow_peer, workspace);
            assert!(client.is_err());
            assert_eq!(server.unwrap_err().to_string(), expected);
        }
    }
}
