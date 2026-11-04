use crate::{
    config::{Identity, Key, NOISE},
    wire::Message,
};
use anyhow::{Context, Result, ensure};
use bincode::Options;
use snow::{Builder, TransportState};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpStream, tcp::OwnedWriteHalf},
    sync::mpsc,
    task::JoinHandle,
    time::timeout,
};

const MAX_RECORD: usize = 128;
const IO_TIMEOUT: Duration = Duration::from_millis(500);
pub const LEASE: Duration = Duration::from_millis(1750);
#[cfg(windows)]
pub const PING_PERIOD: Duration = Duration::from_millis(250);

async fn read_record(
    stream: &mut (impl AsyncRead + Unpin),
    record: &mut [u8; MAX_RECORD],
) -> Result<usize> {
    let length = usize::from(stream.read_u8().await?);
    ensure!((1..=MAX_RECORD).contains(&length), "invalid record length");
    stream.read_exact(&mut record[..length]).await?;
    Ok(length)
}

async fn write_record(stream: &mut (impl AsyncWrite + Unpin), record: &[u8]) -> Result<()> {
    ensure!(
        (1..=MAX_RECORD).contains(&record.len()),
        "invalid outgoing record length"
    );
    stream.write_u8(record.len() as u8).await?;
    stream.write_all(record).await?;
    Ok(())
}

pub async fn authenticate(
    stream: &mut TcpStream,
    ours: &Identity,
    peer: &Key,
    initiator: bool,
    prologue: &[u8],
) -> Result<TransportState> {
    stream.set_nodelay(true)?;
    let builder = Builder::new(NOISE.parse()?)
        .prologue(prologue)?
        .local_private_key(&ours.private)?
        .remote_public_key(peer)?;
    let mut noise = if initiator {
        builder.build_initiator()?
    } else {
        builder.build_responder()?
    };

    timeout(Duration::from_secs(2), async {
        let mut record = [0; MAX_RECORD];
        for send in [initiator, !initiator] {
            if send {
                let len = noise.write_message(&[], &mut record)?;
                write_record(stream, &record[..len]).await?;
            } else {
                let len = read_record(stream, &mut record).await?;
                let mut payload = [0; MAX_RECORD];
                ensure!(
                    noise.read_message(&record[..len], &mut payload)? == 0,
                    "invalid handshake payload"
                );
            }
        }
        Ok(noise.into_transport_mode()?)
    })
    .await
    .context("peer authentication timed out")?
    .context("peer authentication failed")
}

pub struct Pipe {
    state: Arc<Mutex<TransportState>>,
    writer: OwnedWriteHalf,
    pub messages: mpsc::Receiver<Result<Message>>,
    reader: JoinHandle<()>,
}

fn codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_limit(MAX_RECORD as u64)
        .reject_trailing_bytes()
}

impl Pipe {
    pub async fn connect(
        mut stream: TcpStream,
        ours: &Identity,
        peer: &Key,
        initiator: bool,
    ) -> Result<Self> {
        let state = Arc::new(Mutex::new(
            authenticate(
                &mut stream,
                ours,
                peer,
                initiator,
                b"Desk Switch protocol 1",
            )
            .await?,
        ));
        let (mut stream, writer) = stream.into_split();
        let reader_state = Arc::clone(&state);
        let (send, messages) = mpsc::channel(64);
        let reader = tokio::spawn(async move {
            let mut record = [0; MAX_RECORD];
            loop {
                let result = async {
                    let length = read_record(&mut stream, &mut record).await?;
                    let mut plain = [0; MAX_RECORD];
                    let length = reader_state
                        .lock()
                        .unwrap()
                        .read_message(&record[..length], &mut plain)?;
                    Ok(codec().deserialize::<Message>(&plain[..length])?)
                }
                .await;
                let stop = result.is_err();
                if send.send(result).await.is_err() || stop {
                    break;
                }
            }
        });
        Ok(Self {
            state,
            writer,
            messages,
            reader,
        })
    }

    pub async fn send(&mut self, message: Message) -> Result<()> {
        let plain = codec().serialize(&message)?;
        let mut record = [0; MAX_RECORD];
        let len = self
            .state
            .lock()
            .unwrap()
            .write_message(&plain, &mut record)?;
        timeout(IO_TIMEOUT, write_record(&mut self.writer, &record[..len]))
            .await
            .context("peer write timed out")??;
        Ok(())
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    fn identity() -> Identity {
        let pair = Builder::new(NOISE.parse().unwrap())
            .generate_keypair()
            .unwrap();
        Identity {
            private: pair.private.try_into().unwrap(),
            public: pair.public.try_into().unwrap(),
        }
    }

    #[tokio::test]
    async fn only_the_pinned_pair_can_exchange_messages() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let windows = identity();
        let ubuntu = identity();
        let outsider = identity();
        let client = async {
            let bad = Pipe::connect(
                TcpStream::connect(address).await.unwrap(),
                &outsider,
                &ubuntu.public,
                true,
            )
            .await;
            assert!(bad.is_err());
            let mut pipe = Pipe::connect(
                TcpStream::connect(address).await.unwrap(),
                &windows,
                &ubuntu.public,
                true,
            )
            .await
            .unwrap();
            pipe.send(Message::Ping(42)).await.unwrap();
            assert_eq!(
                pipe.messages.recv().await.unwrap().unwrap(),
                Message::Pong(42)
            );
        };
        let server = async {
            let bad = Pipe::connect(
                listener.accept().await.unwrap().0,
                &ubuntu,
                &windows.public,
                false,
            )
            .await;
            assert!(bad.is_err());
            let mut pipe = Pipe::connect(
                listener.accept().await.unwrap().0,
                &ubuntu,
                &windows.public,
                false,
            )
            .await
            .unwrap();
            assert_eq!(
                pipe.messages.recv().await.unwrap().unwrap(),
                Message::Ping(42)
            );
            pipe.send(Message::Pong(42)).await.unwrap();
        };
        tokio::join!(client, server);
    }
}
