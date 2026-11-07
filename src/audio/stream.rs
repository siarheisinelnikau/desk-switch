use super::link::{FRAME_BYTES, Link, Packet, Pcm};
use anyhow::{Context, Result, ensure};
use std::{collections::VecDeque, time::Duration};
use tokio::time::{Instant, timeout};

const WINDOW: usize = 12;
const FRESHNESS: Duration = Duration::from_millis(400);
const WARMUP: Duration = Duration::from_secs(3);
const READY_TIMEOUT: Duration = Duration::from_secs(7);
pub const SILENCE: Pcm = [0; FRAME_BYTES];

pub async fn request_start(link: &mut Link) -> Result<()> {
    let mut incoming = SILENCE;
    let packet = timeout(WARMUP, link.receive(&mut incoming))
        .await
        .context("receiver did not request audio")??;
    ensure!(
        packet == Packet::Start,
        "audio must start with an explicit request"
    );
    Ok(())
}

pub async fn transmit(
    link: &mut Link,
    source: &mut (impl tokio::io::AsyncRead + Unpin),
) -> Result<()> {
    let mut input = CaptureFrame {
        pcm: SILENCE,
        filled: 0,
    };
    let mut incoming = SILENCE;
    tokio::select! {
        result = timeout(WARMUP, input.read(source)) => {
            result.context("the audio source produced no samples")??;
        }
        result = link.receive(&mut incoming) => {
            result?;
            anyhow::bail!("unexpected packet before audio readiness");
        }
    }
    link.send(Packet::Ready, &SILENCE).await?;
    let mut credits = VecDeque::with_capacity(WINDOW);
    let mut next_ticket = 0;
    let mut sequence = 0;
    let mut last_credit = Instant::now();
    loop {
        tokio::select! {
            // Consume already queued credits before choosing to drop a capture frame.
            biased;
            _ = tokio::time::sleep_until(last_credit + WARMUP) => {
                anyhow::bail!("audio receiver stopped granting credits");
            }
            result = link.receive(&mut incoming) => {
                let Packet::Credit(ticket) = result? else { anyhow::bail!("unexpected audio sender command"); };
                ensure!(ticket == next_ticket && credits.len() < WINDOW, "invalid audio credit");
                next_ticket += 1;
                credits.push_back(ticket);
                last_credit = Instant::now();
            }
            result = input.read(source) => {
                result?;
                if let Some(ticket) = credits.pop_front() {
                    link.send(Packet::Audio { ticket, sequence }, &input.pcm).await?;
                }
                sequence += 1;
            }
        }
    }
}

struct CaptureFrame {
    pcm: Pcm,
    filled: usize,
}

impl CaptureFrame {
    async fn read(&mut self, source: &mut (impl tokio::io::AsyncRead + Unpin)) -> Result<()> {
        use tokio::io::AsyncReadExt;
        while self.filled < self.pcm.len() {
            let count = source.read(&mut self.pcm[self.filled..]).await?;
            ensure!(count != 0, "audio capture stopped");
            self.filled += count;
        }
        self.filled = 0;
        Ok(())
    }
}

pub async fn receive(
    link: &mut Link,
    queue: &std::sync::Mutex<super::playout::Playout>,
    mut on_frame: impl FnMut(u64),
) -> Result<()> {
    let _cleanup = super::playout::ClearOnDrop(queue);

    link.send(Packet::Start, &SILENCE).await?;
    let mut pcm = SILENCE;
    let packet = timeout(READY_TIMEOUT, link.receive(&mut pcm))
        .await
        .context("the sender audio source did not become ready")??;
    ensure!(
        packet == Packet::Ready,
        "sender did not confirm audio readiness"
    );
    let mut requests = VecDeque::with_capacity(WINDOW);
    for ticket in 0..WINDOW as u64 {
        requests.push_back((ticket, Instant::now() + FRESHNESS));
        link.send(Packet::Credit(ticket), &SILENCE).await?;
    }

    loop {
        let (expected, deadline) = requests.pop_front().unwrap();
        // The capture uses only a frame completed after the credit was sent.
        // Timing our own credit bounds the age without synchronizing the clocks.
        let packet = timeout(WARMUP, link.receive(&mut pcm))
            .await
            .context("audio sender stopped supplying frames")??;
        let Packet::Audio { ticket, sequence } = packet else {
            anyhow::bail!("expected an audio frame");
        };
        ensure!(ticket == expected, "unexpected audio frame ticket");
        // A delayed packet is discarded, but its credit is renewed. A brief
        // Wi-Fi stall should not tear down capture/playback for another second.
        if Instant::now() < deadline {
            queue
                .lock()
                .unwrap()
                .push(sequence, pcm, std::time::Instant::now());
            on_frame(sequence);
        }
        let ticket = ticket + WINDOW as u64;
        requests.push_back((ticket, Instant::now() + FRESHNESS));
        link.send(Packet::Credit(ticket), &SILENCE).await?;
    }
}
