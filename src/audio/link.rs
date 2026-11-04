use crate::{
    config::{Identity, Key},
    transport::authenticate,
};
use anyhow::{Context, Result, bail, ensure};
use snow::TransportState;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

pub const RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
pub const SAMPLE_BYTES: usize = 2;
pub const BLOCK_ALIGN: usize = CHANNELS * SAMPLE_BYTES;
pub const FRAME_BYTES: usize = RATE as usize / 100 * BLOCK_ALIGN;
pub type Pcm = [u8; FRAME_BYTES];

const HEADER: usize = 2;
const DATA_HEADER: usize = 17;
const MAX_PLAIN: usize = DATA_HEADER + FRAME_BYTES;
const TAG: usize = 16;
const MAX_RECORD: usize = MAX_PLAIN + TAG;
const WRITE_TIMEOUT: Duration = Duration::from_millis(125);

#[derive(Debug, PartialEq, Eq)]
pub enum Packet {
    Start,
    Ready,
    Credit(u64),
    Audio { ticket: u64, sequence: u64 },
}

impl Packet {
    fn encode(&self, pcm: &Pcm, output: &mut [u8; MAX_PLAIN]) -> usize {
        match self {
            Self::Start => {
                output[0] = 0;
                1
            }
            Self::Ready => {
                output[0] = 1;
                1
            }
            Self::Credit(ticket) => {
                output[0] = 2;
                output[1..9].copy_from_slice(&ticket.to_be_bytes());
                9
            }
            Self::Audio { ticket, sequence } => {
                output[0] = 3;
                output[1..9].copy_from_slice(&ticket.to_be_bytes());
                output[9..17].copy_from_slice(&sequence.to_be_bytes());
                output[DATA_HEADER..].copy_from_slice(pcm);
                MAX_PLAIN
            }
        }
    }

    fn decode(input: &[u8], pcm: &mut Pcm) -> Result<Self> {
        match (input.first(), input.len()) {
            (Some(0), 1) => Ok(Self::Start),
            (Some(1), 1) => Ok(Self::Ready),
            (Some(2), 9) => Ok(Self::Credit(u64::from_be_bytes(input[1..9].try_into()?))),
            (Some(3), MAX_PLAIN) => {
                pcm.copy_from_slice(&input[DATA_HEADER..]);
                Ok(Self::Audio {
                    ticket: u64::from_be_bytes(input[1..9].try_into()?),
                    sequence: u64::from_be_bytes(input[9..17].try_into()?),
                })
            }
            _ => bail!("invalid audio packet"),
        }
    }
}

pub struct Link {
    stream: TcpStream,
    state: TransportState,
    record: [u8; HEADER + MAX_RECORD],
    filled: usize,
}

impl Link {
    pub async fn open(
        mut stream: TcpStream,
        identity: &Identity,
        peer: &Key,
        initiator: bool,
    ) -> Result<Self> {
        let state = authenticate(
            &mut stream,
            identity,
            peer,
            initiator,
            b"Desk Switch audio 1",
        )
        .await?;
        Ok(Self {
            stream,
            state,
            record: [0; HEADER + MAX_RECORD],
            filled: 0,
        })
    }

    pub async fn send(&mut self, packet: Packet, pcm: &Pcm) -> Result<()> {
        let mut plain = [0; MAX_PLAIN];
        let len = packet.encode(pcm, &mut plain);
        let mut record = [0; HEADER + MAX_RECORD];
        let len = self
            .state
            .write_message(&plain[..len], &mut record[HEADER..])?;
        record[..HEADER].copy_from_slice(&(len as u16).to_be_bytes());
        timeout(
            WRITE_TIMEOUT,
            self.stream.write_all(&record[..HEADER + len]),
        )
        .await
        .context("audio network write stalled")??;
        Ok(())
    }

    // A competing select branch can interrupt a read between TCP fragments.
    // Keep both the partial header and payload across polls.
    pub async fn receive(&mut self, pcm: &mut Pcm) -> Result<Packet> {
        self.fill(HEADER).await?;
        let len = usize::from(u16::from_be_bytes(self.record[..HEADER].try_into()?));
        ensure!(
            (TAG + 1..=MAX_RECORD).contains(&len),
            "invalid audio record length"
        );
        self.fill(HEADER + len).await?;
        let mut plain = [0; MAX_PLAIN];
        let len = self
            .state
            .read_message(&self.record[HEADER..HEADER + len], &mut plain)?;
        self.filled = 0;
        Packet::decode(&plain[..len], pcm)
    }

    async fn fill(&mut self, end: usize) -> Result<()> {
        while self.filled < end {
            let count = self.stream.read(&mut self.record[self.filled..end]).await?;
            ensure!(count != 0, "audio peer disconnected");
            self.filled += count;
        }
        Ok(())
    }
}
