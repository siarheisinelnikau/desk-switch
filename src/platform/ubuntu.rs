use super::portal::Portal;
use crate::{
    config::{Identity, Key},
    transport::{LEASE, Pipe},
    video::{Inputs, Job, SETTLE, VideoResult, VideoWorker},
    wire::{Message, Mode},
};
use anyhow::{Context, Result, bail, ensure};
use futures_util::StreamExt;
use std::{net::SocketAddr, time::Duration};
use tokio::{
    net::TcpListener,
    sync::oneshot,
    time::{Instant, MissedTickBehavior, interval, timeout},
};

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Armed,
    Reading,
    Verified,
    Active,
    Exiting,
    Returning,
    Returned,
}

struct Transfer {
    id: u64,
    mode: Mode,
    edge: Option<u16>,
    stage: Stage,
}

pub async fn run(
    address: SocketAddr,
    identity: Identity,
    peer: Key,
    mut portal: Portal,
    video: Option<(Inputs, VideoWorker)>,
) -> Result<()> {
    let listener = TcpListener::bind(address).await.context("binding Ubuntu listen address")?;
    println!("Listening for the pinned Windows peer on {address}.");
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    loop {
        let (stream, _) = tokio::select! {
            result = &mut shutdown => { result?; break; }
            event = portal.stream.next() => {
                if let Err(error) = event.context("EIS device stream closed").and_then(|event| Ok(event?)).and_then(|event| portal.handle(event)) {
                    portal.close().await;
                    return Err(error);
                }
                continue;
            }
            result = listener.accept() => result?,
        };
        let mut pipe = match Pipe::connect(stream, &identity, &peer, false).await {
            Ok(pipe) => pipe,
            Err(error) => {
                eprintln!("Discarded unauthenticated connection: {error:#}");
                continue;
            }
        };
        println!("Windows peer authenticated.");
        let mut transfer = None;
        let mut last_id = 0;
        let result = tokio::select! {
            result = &mut shutdown => {
                result?;
                None
            }
            result = session(&mut pipe, &mut portal, video.as_ref(), &mut transfer, &mut last_id) => Some(result),
        };
        drop(pipe);
        let released = portal.release();
        if let Some(Err(error)) = &result {
            eprintln!("Session ended: {error:#}");
        }
        if transfer.as_ref().is_some_and(|active: &Transfer| active.mode == Mode::Video)
            && let Some((inputs, worker)) = &video
        {
            eprintln!("Attempting to restore the shared monitor to Windows.");
            if let Err(error) = worker.recover(*inputs).await {
                eprintln!("Automatic video recovery could not be verified: {error:#}");
            }
        }
        if let Err(error) = released {
            portal.close().await;
            return Err(error.context("Wayland input was closed after a release error"));
        }
        if result.is_none() {
            break;
        }
    }
    portal.close().await;
    Ok(())
}

async fn session(
    pipe: &mut Pipe,
    portal: &mut Portal,
    video: Option<&(Inputs, VideoWorker)>,
    transfer: &mut Option<Transfer>,
    last_id: &mut u64,
) -> Result<()> {
    let mut lease = Instant::now() + LEASE;
    let mut next_read = Instant::now();
    let mut job: Option<oneshot::Receiver<Result<VideoResult>>> = None;
    let mut tick = interval(Duration::from_millis(25));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        ensure!(Instant::now() < lease, "Windows heartbeat timed out");
        tokio::select! {
            _ = tick.tick() => {
                let Some(active) = transfer.as_mut() else { continue };
                if let Some(pending) = &mut job {
                    match pending.try_recv() {
                        Ok(result) => {
                            job = None;
                            match active.stage {
                                Stage::Reading => {
                                    let (inputs, _) = video.unwrap();
                                    if matches!(result, Ok(VideoResult::Read(input)) if input == inputs.ubuntu) {
                                        active.stage = Stage::Verified;
                                        pipe.send(Message::Observed { id: active.id, input: inputs.ubuntu }).await?;
                                    } else {
                                        if let Err(error) = result {
                                            eprintln!("Waiting for Ubuntu-side DDC read: {error:#}");
                                        }
                                        next_read = Instant::now() + Duration::from_millis(300);
                                    }
                                }
                                Stage::Returning => {
                                    result.context("Ubuntu could not select the Windows monitor input")?;
                                    active.stage = Stage::Returned;
                                    pipe.send(Message::Returning(active.id)).await?;
                                }
                                _ => bail!("unexpected DDC job result"),
                            }
                        }
                        Err(oneshot::error::TryRecvError::Empty) => {}
                        Err(oneshot::error::TryRecvError::Closed) => bail!("Ubuntu DDC worker stopped"),
                    }
                }
                if active.stage == Stage::Reading && job.is_none() && Instant::now() >= next_read {
                    job = Some(video.unwrap().1.submit(Job::Read)?);
                }
            }
            event = portal.stream.next() => {
                portal.handle(event.context("EIS device stream closed")??)?;
            }
            message = pipe.messages.recv() => {
                let message = message.context("Windows disconnected")??;
                if let Message::Ping(nonce) = message {
                    lease = Instant::now() + LEASE;
                    pipe.send(Message::Pong(nonce)).await?;
                    continue;
                }
                if let Message::Prepare { id, mode, edge } = message {
                    ensure!(id > *last_id && transfer.is_none(), "stale or overlapping transfer");
                    *last_id = id;
                    if let Err(error) = portal.can_start(edge) {
                        eprintln!("Transfer rejected: {error:#}");
                        pipe.send(Message::Rejected(id)).await?;
                        continue;
                    }
                    if mode == Mode::Video && video.is_none() {
                        eprintln!("Transfer rejected: configure the shared monitor on Ubuntu.");
                        pipe.send(Message::Rejected(id)).await?;
                        continue;
                    }
                    *transfer = Some(Transfer { id, mode, edge, stage: Stage::Armed });
                    pipe.send(Message::Ready(id)).await?;
                    continue;
                }
                let active = transfer.as_mut().context("transfer message without a transfer")?;
                match message {
                    Message::Verify(id) if active.id == id && active.mode == Mode::Video && active.stage == Stage::Armed => {
                        active.stage = Stage::Reading;
                        next_read = Instant::now() + SETTLE;
                    }
                    Message::Activate(id) if active.id == id && (
                        active.stage == Stage::Verified || (active.stage == Stage::Armed && active.mode == Mode::Input)
                    ) => {
                        portal.start(active.edge)?;
                        active.stage = Stage::Active;
                        pipe.send(Message::Active(id)).await?;
                        println!("Input active on Ubuntu ({:?}).", active.mode);
                    }
                    Message::Event(input) if active.stage == Stage::Active => {
                        if let Some(position) = portal.event(input)? {
                            active.stage = Stage::Exiting;
                            pipe.send(Message::ExitEdge { id: active.id, position }).await?;
                        }
                    }
                    Message::Event(_) if active.stage == Stage::Exiting => {}
                    Message::Return(id) if active.id == id && matches!(active.stage, Stage::Active | Stage::Exiting) => {
                        portal.release()?;
                        if active.mode == Mode::Video {
                            let (inputs, worker) = video.unwrap();
                            active.stage = Stage::Returning;
                            job = Some(worker.submit(Job::Select { from: inputs.ubuntu, to: inputs.windows })?);
                        } else {
                            active.stage = Stage::Returned;
                            pipe.send(Message::Returning(id)).await?;
                        }
                    }
                    Message::Returned(id) if active.id == id && active.stage == Stage::Returned => {
                        *transfer = None;
                        println!("Input returned to Windows.");
                    }
                    _ => bail!("unexpected, stale or out-of-phase command"),
                }
            }
        }
    }
}
