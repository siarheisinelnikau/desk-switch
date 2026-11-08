//! A dedicated, pinned Noise connection keeps bulk data off input/control links.
use super::protocol::{MAX_MANIFEST, Packet, VERSION};
use crate::config::{self, Identity};
use anyhow::{Context, Result, ensure};
use bincode::Options;
use snow::{Builder, TransportState};
use std::{collections::BTreeSet, net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

const MAGIC: &[u8; 8] = b"DESKFILE";
const RECORD: usize = 60 * 1024;
const AUTH: Duration = Duration::from_secs(5);
const IO: Duration = Duration::from_secs(30);
const MAX_PACKET: usize = MAX_MANIFEST + 128 * 1024;

pub(super) struct Connection {
    stream: TcpStream,
    noise: TransportState,
}
fn codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_limit(MAX_PACKET as u64)
        .reject_trailing_bytes()
}
async fn record_write(stream: &mut TcpStream, bytes: &[u8]) -> Result<()> {
    ensure!(
        (1..=u16::MAX as usize).contains(&bytes.len()),
        "Invalid transfer record size"
    );
    let mut framed = Vec::with_capacity(bytes.len() + 2);
    framed.extend((bytes.len() as u16).to_be_bytes());
    framed.extend(bytes);
    stream.write_all(&framed).await?;
    Ok(())
}
async fn record_read(stream: &mut TcpStream, limit: usize) -> Result<Vec<u8>> {
    let length = usize::from(stream.read_u16().await?);
    ensure!(
        length > 0 && length <= limit,
        "Invalid transfer record size"
    );
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).await?;
    Ok(bytes)
}
fn valid_workspace(id: &str) -> Result<()> {
    ensure!(
        (16..=128).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'),
        "Invalid file transfer workspace"
    );
    Ok(())
}
async fn authenticate(
    mut stream: TcpStream,
    identity: &Identity,
    peer: &str,
    workspace: &str,
    initiator: bool,
) -> Result<Connection> {
    valid_workspace(workspace)?;
    let prologue = format!("Desk Switch file transfer {VERSION}\0{workspace}");
    let peer_key = config::key(peer)?;
    let builder = Builder::new(config::NOISE.parse()?)
        .prologue(prologue.as_bytes())?
        .local_private_key(&identity.private)?
        .remote_public_key(&peer_key)?;
    let mut noise = if initiator {
        builder.build_initiator()?
    } else {
        builder.build_responder()?
    };
    stream.set_nodelay(true)?;
    timeout(AUTH, async {
        for send in [initiator, !initiator] {
            if send {
                let mut bytes = [0; 256];
                let count = noise.write_message(&VERSION.to_be_bytes(), &mut bytes)?;
                record_write(&mut stream, &bytes[..count]).await?;
            } else {
                let bytes = record_read(&mut stream, 256).await?;
                let mut plain = [0; 256];
                let count = noise.read_message(&bytes, &mut plain)?;
                ensure!(
                    plain[..count] == VERSION.to_be_bytes(),
                    "Unsupported file transfer protocol"
                );
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("File transfer authentication timed out")??;
    Ok(Connection {
        stream,
        noise: noise.into_transport_mode()?,
    })
}
pub(super) async fn connect(
    address: SocketAddr,
    identity: &Identity,
    peer: &str,
    workspace: &str,
) -> Result<Connection> {
    valid_workspace(workspace)?;
    let mut stream = timeout(AUTH, TcpStream::connect(address))
        .await
        .context("The other computer did not answer the file transfer request")??;
    timeout(AUTH, async {
        stream.write_all(MAGIC).await?;
        stream.write_all(&identity.public).await?;
        stream.write_u16(workspace.len() as u16).await?;
        stream.write_all(workspace.as_bytes()).await?;
        Ok::<_, std::io::Error>(())
    })
    .await??;
    authenticate(stream, identity, peer, workspace, true).await
}
pub(super) async fn accept(
    mut stream: TcpStream,
    identity: &Identity,
    workspace: &str,
    peers: &BTreeSet<String>,
) -> Result<(String, Connection)> {
    let peer = timeout(AUTH, async {
        let mut magic = [0; 8];
        stream.read_exact(&mut magic).await?;
        ensure!(&magic == MAGIC, "Invalid file transfer greeting");
        let mut key = [0; 32];
        stream.read_exact(&mut key).await?;
        let peer = config::hex(&key);
        ensure!(
            peers.contains(&peer),
            "This computer is not a trusted workspace member"
        );
        let length = usize::from(stream.read_u16().await?);
        ensure!(
            (16..=128).contains(&length),
            "Invalid file transfer workspace"
        );
        let mut bytes = vec![0; length];
        stream.read_exact(&mut bytes).await?;
        ensure!(
            bytes == workspace.as_bytes(),
            "The file transfer belongs to another workspace"
        );
        Ok::<_, anyhow::Error>(peer)
    })
    .await
    .context("File transfer authentication timed out")??;
    let connection = authenticate(stream, identity, &peer, workspace, false).await?;
    Ok((peer, connection))
}
impl Connection {
    pub(super) async fn send(&mut self, packet: &Packet) -> Result<()> {
        let bytes = codec().serialize(packet)?;
        ensure!(
            !bytes.is_empty() && bytes.len() <= MAX_PACKET,
            "File transfer packet is too large"
        );
        timeout(IO, async {
            let mut offset = 0;
            let mut first = true;
            while offset < bytes.len() {
                let end = (offset + RECORD - 4).min(bytes.len());
                let mut plain = Vec::with_capacity(RECORD);
                if first {
                    plain.extend((bytes.len() as u32).to_be_bytes());
                }
                plain.extend(&bytes[offset..end]);
                let mut encrypted = vec![0; plain.len() + 16];
                let count = self.noise.write_message(&plain, &mut encrypted)?;
                record_write(&mut self.stream, &encrypted[..count]).await?;
                offset = end;
                first = false;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("The file transfer connection stopped responding")??;
        Ok(())
    }
    pub(super) async fn receive(&mut self) -> Result<Packet> {
        timeout(IO, async {
            let first = record_read(&mut self.stream, RECORD + 16).await?;
            let mut plain = vec![0; RECORD];
            let count = self.noise.read_message(&first, &mut plain)?;
            ensure!(count >= 5, "Invalid file transfer packet header");
            let length = u32::from_be_bytes(plain[..4].try_into()?) as usize;
            ensure!(
                (1..=MAX_PACKET).contains(&length),
                "File transfer packet is too large"
            );
            let mut bytes = Vec::with_capacity(length.min(RECORD));
            bytes.extend(&plain[4..count]);
            ensure!(bytes.len() <= length, "Invalid file transfer packet length");
            while bytes.len() < length {
                let encrypted = record_read(&mut self.stream, RECORD + 16).await?;
                let count = self.noise.read_message(&encrypted, &mut plain)?;
                ensure!(
                    count > 0 && bytes.len() + count <= length,
                    "Invalid file transfer packet length"
                );
                bytes.extend(&plain[..count]);
            }
            Ok(codec().deserialize(&bytes)?)
        })
        .await
        .context("The file transfer connection stopped responding")?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity() -> Identity {
        let pair = Builder::new(config::NOISE.parse().unwrap())
            .generate_keypair()
            .unwrap();
        Identity {
            private: pair.private.try_into().unwrap(),
            public: pair.public.try_into().unwrap(),
        }
    }
    #[tokio::test]
    async fn pinned_peers_exchange_chunks_but_another_workspace_or_identity_cannot() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let alice = identity();
        let bob = identity();
        let outsider = identity();
        let a = config::hex(&alice.public);
        let b = config::hex(&bob.public);
        let workspace = "ab".repeat(32);
        let trusted = BTreeSet::from([a]);
        let server = async {
            assert!(
                accept(
                    listener.accept().await.unwrap().0,
                    &bob,
                    &workspace,
                    &trusted
                )
                .await
                .is_err()
            );
            assert!(
                accept(
                    listener.accept().await.unwrap().0,
                    &bob,
                    &workspace,
                    &trusted
                )
                .await
                .is_err()
            );
            let (_, mut pipe) = accept(
                listener.accept().await.unwrap().0,
                &bob,
                &workspace,
                &trusted,
            )
            .await
            .unwrap();
            match pipe.receive().await.unwrap() {
                Packet::Chunk { bytes, digest, .. } => {
                    assert_eq!(bytes, vec![7; 48 * 1024]);
                    assert_eq!(digest, super::super::protocol::hash(&bytes));
                }
                _ => panic!("Expected a synthetic chunk"),
            }
            pipe.send(&Packet::Complete).await.unwrap();
        };
        let client = async {
            assert!(connect(address, &outsider, &b, &workspace).await.is_err());
            assert!(
                connect(address, &alice, &b, &"cd".repeat(32))
                    .await
                    .is_err()
            );
            let mut pipe = connect(address, &alice, &b, &workspace).await.unwrap();
            let bytes = vec![7; 48 * 1024];
            let digest = super::super::protocol::hash(&bytes);
            pipe.send(&Packet::Chunk {
                index: 0,
                offset: 0,
                bytes,
                digest,
            })
            .await
            .unwrap();
            assert!(matches!(pipe.receive().await.unwrap(), Packet::Complete));
        };
        tokio::join!(server, client);
    }
    #[tokio::test]
    async fn large_offers_span_authenticated_records_without_losing_the_following_packet() {
        use super::super::protocol::{Entry, Kind, Manifest, Origin, hash};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let alice = identity();
        let bob = identity();
        let a = config::hex(&alice.public);
        let b = config::hex(&bob.public);
        let workspace = "ab".repeat(32);
        let mut entries = vec![Entry {
            path: vec!["Folder".into()],
            kind: Kind::Directory,
        }];
        entries.extend((0..1500).map(|n| Entry {
            path: vec![
                "Folder".into(),
                format!("file-{n:04}-{}.bin", "x".repeat(72)),
            ],
            kind: Kind::File {
                size: 0,
                sha256: hash(&[]),
            },
        }));
        let manifest = Manifest {
            version: VERSION,
            id: "cd".repeat(16),
            name: "Folder".into(),
            roots: vec!["Folder".into()],
            entries,
            total: 0,
        };
        manifest.validate().unwrap();
        let offer = Packet::Offer {
            manifest: manifest.clone(),
            origin: Origin::Send,
            stamp: None,
        };
        assert!(codec().serialize(&offer).unwrap().len() > 64 * 1024);
        let server = async {
            let (_, mut pipe) = accept(
                listener.accept().await.unwrap().0,
                &bob,
                &workspace,
                &BTreeSet::from([a]),
            )
            .await
            .unwrap();
            let Packet::Offer {
                manifest: received, ..
            } = pipe.receive().await.unwrap()
            else {
                panic!("Expected a multi-record manifest")
            };
            assert_eq!(received, manifest);
            assert!(matches!(pipe.receive().await.unwrap(), Packet::Finish));
            pipe.send(&Packet::Complete).await.unwrap();
        };
        let client = async {
            let mut pipe = connect(address, &alice, &b, &workspace).await.unwrap();
            pipe.send(&offer).await.unwrap();
            pipe.send(&Packet::Finish).await.unwrap();
            assert!(matches!(pipe.receive().await.unwrap(), Packet::Complete));
        };
        tokio::join!(server, client);
    }
}
