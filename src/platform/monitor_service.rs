//! Bidirectional monitor-only transactions. No input adapter or portal is opened.
use crate::{
    config::{Identity, Key, Role},
    controller::{Controller, Effect, Transfer},
    lifecycle::{self, Command, Event, InputStatus},
    transport::{LEASE, PING_PERIOD, Pipe},
    video::{Inputs, Job, SETTLE, VideoResult, VideoWorker},
    wire::{Message, Mode},
};
use anyhow::{Context, Result, bail, ensure};
use std::{net::SocketAddr, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{broadcast, oneshot},
    time::{Instant, MissedTickBehavior, interval, sleep, timeout},
};

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Armed,
    Selecting,
    Reading,
    Fallback,
    Verified,
}
enum Exchange {
    Source(Controller),
    Target { id: u64, stage: Stage },
}
struct Session<'a> {
    video: &'a VideoWorker,
    inputs: Inputs,
    exchange: Option<Exchange>,
    job: Option<oneshot::Receiver<Result<VideoResult>>>,
    read_after: Instant,
    deadline: Instant,
    sequence: u64,
    peer_sequence: u64,
    #[cfg(test)]
    completed: Option<tokio::sync::mpsc::UnboundedSender<()>>,
}
impl<'a> Session<'a> {
    fn new(video: &'a VideoWorker, inputs: Inputs) -> Self {
        Self {
            video,
            inputs,
            exchange: None,
            job: None,
            read_after: Instant::now(),
            deadline: Instant::now(),
            sequence: 0,
            peer_sequence: 0,
            #[cfg(test)]
            completed: None,
        }
    }
    fn complete(&mut self) {
        self.exchange = None;
        #[cfg(test)]
        if let Some(completed) = &self.completed {
            let _ = completed.send(());
        }
    }
    async fn effect(&mut self, pipe: &mut Pipe, effect: Effect) -> Result<()> {
        match effect {
            Effect::Send(message) => pipe.send(message).await?,
            Effect::Ddc(job @ Job::Select { .. }) => self.job = Some(self.video.submit(job)?),
            Effect::Forward => {
                self.complete();
                println!("Shared monitor confirmed on the other PC.");
            }
            _ => bail!("unexpected monitor-only effect"),
        }
        Ok(())
    }
    async fn start(&mut self, pipe: &mut Pipe) -> Result<()> {
        if self.exchange.is_some() {
            return Ok(());
        }
        lifecycle::report(Event::Switch { error: None });
        self.sequence += 1;
        let mut controller = Controller::new(Some(self.inputs));
        let effect = controller.start(Transfer {
            id: self.sequence,
            mode: Mode::Video,
            edge: None,
        })?;
        self.exchange = Some(Exchange::Source(controller));
        self.deadline = Instant::now() + Duration::from_secs(15);
        self.effect(pipe, effect).await
    }
    async fn message(&mut self, pipe: &mut Pipe, message: Message) -> Result<()> {
        if let Message::Prepare { id, inputs, edge } = message {
            ensure!(id > self.peer_sequence, "stale monitor request");
            self.peer_sequence = id;
            if self.exchange.is_some()
                || edge.is_some()
                || inputs
                    != Some(Inputs {
                        host: self.inputs.peer,
                        peer: self.inputs.host,
                    })
            {
                pipe.send(Message::Rejected(id)).await?;
                return Ok(());
            }
            lifecycle::report(Event::Switch { error: None });
            self.exchange = Some(Exchange::Target {
                id,
                stage: Stage::Armed,
            });
            self.deadline = Instant::now() + Duration::from_secs(15);
            return pipe.send(Message::Ready(id)).await;
        }
        match &mut self.exchange {
            Some(Exchange::Source(controller)) => {
                let effect = controller.message(message)?;
                self.effect(pipe, effect).await
            }
            Some(Exchange::Target { id, stage }) => match message {
                Message::SelectMonitor(reply) if reply == *id && *stage == Stage::Armed => {
                    *stage = Stage::Selecting;
                    self.job = Some(self.video.submit(Job::Select {
                        from: self.inputs.peer,
                        to: self.inputs.host,
                    })?);
                    Ok(())
                }
                Message::Verify(reply) if reply == *id && *stage == Stage::Fallback => {
                    *stage = Stage::Reading;
                    self.read_after = Instant::now() + SETTLE;
                    Ok(())
                }
                Message::Activate(reply) if reply == *id && *stage == Stage::Verified => {
                    pipe.send(Message::Active(*id)).await?;
                    self.complete();
                    println!("Shared monitor confirmed on this PC.");
                    Ok(())
                }
                _ => bail!("unexpected monitor-only command"),
            },
            None => bail!("monitor reply without a transaction"),
        }
    }
    async fn poll(&mut self, pipe: &mut Pipe) -> Result<()> {
        ensure!(
            self.exchange.is_none() || Instant::now() < self.deadline,
            "monitor confirmation timed out"
        );
        let result = if let Some(job) = &mut self.job {
            match job.try_recv() {
                Ok(result) => {
                    self.job = None;
                    Some(result)
                }
                Err(oneshot::error::TryRecvError::Empty) => None,
                Err(oneshot::error::TryRecvError::Closed) => bail!("monitor worker stopped"),
            }
        } else {
            None
        };
        if let Some(result) = result {
            match &mut self.exchange {
                Some(Exchange::Source(controller)) => {
                    if let Err(error) = result
                        && !crate::video::write_attempted(&error)
                    {
                        return Err(error.context(
                            "the active port could not select the other PC's remembered input",
                        ));
                    }
                    let effect = controller.written()?;
                    self.effect(pipe, effect).await?;
                }
                Some(Exchange::Target { id, stage }) => {
                    if *stage == Stage::Selecting
                        && (matches!(result, Ok(VideoResult::Written))
                            || result.as_ref().is_err_and(crate::video::write_attempted))
                    {
                        *stage = Stage::Reading;
                        self.read_after = Instant::now() + SETTLE;
                    } else if *stage == Stage::Reading
                        && matches!(result, Ok(VideoResult::Read(input)) if input == self.inputs.host)
                    {
                        *stage = Stage::Verified;
                        pipe.send(Message::Observed {
                            id: *id,
                            input: self.inputs.host,
                        })
                        .await?;
                    } else {
                        if let Err(error) = result {
                            eprintln!("Monitor selection/read: {error:#}");
                        }
                        if *stage == Stage::Selecting {
                            *stage = Stage::Fallback;
                            pipe.send(Message::SelectFallback(*id)).await?;
                        } else {
                            self.read_after = Instant::now() + Duration::from_millis(300);
                        }
                    }
                }
                None => bail!("monitor job without a transaction"),
            }
        }
        if matches!(
            self.exchange,
            Some(Exchange::Target {
                stage: Stage::Reading,
                ..
            })
        ) && self.job.is_none()
            && Instant::now() >= self.read_after
        {
            self.job = Some(self.video.submit(Job::Read)?);
        }
        Ok(())
    }
    async fn recover(&mut self) {
        // A failed confirmation must not start another input selection. The
        // last requested monitor input remains in place until the user retries.
        self.exchange = None;
        self.job.take();
    }
}

async fn session(
    pipe: &mut Pipe,
    state: &mut Session<'_>,
    mut commands: broadcast::Receiver<Command>,
) -> Result<()> {
    let mut tick = interval(Duration::from_millis(20));
    let mut ping = interval(PING_PERIOD);
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    ping.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut nonce = 0;
    let mut pong = 0;
    let mut lease = Instant::now() + LEASE;
    loop {
        ensure!(Instant::now() < lease, "monitor peer heartbeat timed out");
        tokio::select! {
            _ = tick.tick() => state.poll(pipe).await?,
            _ = ping.tick() => { nonce += 1; pipe.send(Message::Ping(nonce)).await?; }
            command = commands.recv() => match command {
                Ok(Command::Switch) => state.start(pipe).await?,
                Ok(Command::Emergency) => bail!("monitor recovery requested"),
                _ => {}
            },
            message = pipe.messages.recv() => match message.context("monitor peer disconnected")?? {
                Message::Ping(nonce) => { lease = Instant::now() + LEASE; pipe.send(Message::Pong(nonce)).await?; }
                Message::Pong(reply) => { ensure!(reply > pong && reply <= nonce, "invalid monitor heartbeat"); pong = reply; lease = Instant::now() + LEASE; }
                message => state.message(pipe, message).await?,
            }
        }
    }
}

pub async fn run(
    role: Role,
    address: SocketAddr,
    identity: Identity,
    peer: Key,
    (inputs, video): (Inputs, VideoWorker),
) -> Result<()> {
    let listener = if role == Role::Receiver {
        Some(TcpListener::bind(address).await?)
    } else {
        None
    };
    let shutdown = lifecycle::shutdown();
    tokio::pin!(shutdown);
    loop {
        let connection: Result<Pipe> = tokio::select! {
            result = &mut shutdown => return Ok(result?),
            connection = async {
                let stream = if let Some(listener) = &listener { listener.accept().await?.0 }
                    else { timeout(Duration::from_secs(2), TcpStream::connect(address)).await?? };
                Pipe::connect(stream, &identity, &peer, role == Role::Host).await
            } => connection,
        };
        match connection {
            Ok(mut pipe) => {
                lifecycle::report(Event::Input(InputStatus::Here));
                let mut state = Session::new(&video, inputs);
                let result = tokio::select! {
                    result = &mut shutdown => { result?; None }
                    result = session(&mut pipe, &mut state, lifecycle::commands()) => Some(result),
                };
                drop(pipe);
                if let Some(Err(error)) = &result {
                    eprintln!("Monitor connection: {error:#}");
                    if state.exchange.is_some() {
                        lifecycle::report(Event::Switch {
                            error: Some(format!("{error:#}")),
                        });
                    }
                }
                state.recover().await;
                if result.is_none() {
                    return Ok(());
                }
            }
            Err(error) => eprintln!("Waiting for the monitor peer: {error:#}"),
        }
        lifecycle::report(Event::Input(InputStatus::Waiting));
        tokio::select! { result = &mut shutdown => return Ok(result?), _ = sleep(Duration::from_secs(1)) => {} }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::Monitor;
    use std::sync::{
        Arc,
        atomic::{AtomicU16, AtomicUsize, Ordering::SeqCst},
    };
    struct Display {
        input: Arc<AtomicU16>,
        writes: Arc<AtomicUsize>,
        own: u16,
        active_only: bool,
        delayed: bool,
        write_error: bool,
        pending: Option<(u16, std::time::Instant)>,
    }
    impl Monitor for Display {
        fn read(&mut self) -> Result<u16> {
            if let Some((input, since)) = self.pending
                && since.elapsed() >= Duration::from_millis(1100)
            {
                self.input.store(input, SeqCst);
                self.pending = None;
            }
            let input = self.input.load(SeqCst);
            ensure!(!self.active_only || input == self.own, "inactive DDC port");
            Ok(input)
        }
        fn write(&mut self, input: u16) -> Result<()> {
            if !self.active_only {
                ensure!(
                    input == self.own,
                    "the destination must select its own saved input"
                );
            }
            self.writes.fetch_add(1, SeqCst);
            if self.delayed {
                self.pending = Some((input, std::time::Instant::now()));
            } else {
                self.input.store(input, SeqCst);
            }
            ensure!(
                !self.write_error,
                "simulated I/O error after accepted DDC write"
            );
            Ok(())
        }
    }
    fn identity() -> Identity {
        let pair = snow::Builder::new(crate::config::NOISE.parse().unwrap())
            .generate_keypair()
            .unwrap();
        Identity {
            private: pair.private.try_into().unwrap(),
            public: pair.public.try_into().unwrap(),
        }
    }
    async fn fixture(active_only: bool, initial: u16, delayed: bool, write_error: bool) {
        let a = identity();
        let b = identity();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (left, right) = tokio::join!(
            TcpStream::connect(listener.local_addr().unwrap()),
            listener.accept()
        );
        let (left, right) = tokio::join!(
            Pipe::connect(left.unwrap(), &a, &b.public, true),
            Pipe::connect(right.unwrap().0, &b, &a.public, false)
        );
        let (mut left, mut right) = (left.unwrap(), right.unwrap());
        let input = Arc::new(AtomicU16::new(initial));
        let a_writes = Arc::new(AtomicUsize::new(0));
        let b_writes = Arc::new(AtomicUsize::new(0));
        let a_video = VideoWorker::start(Display {
            input: input.clone(),
            writes: a_writes.clone(),
            own: 17,
            active_only,
            delayed,
            write_error,
            pending: None,
        })
        .unwrap();
        let b_video = VideoWorker::start(Display {
            input: input.clone(),
            writes: b_writes.clone(),
            own: 15,
            active_only,
            delayed,
            write_error,
            pending: None,
        })
        .unwrap();
        let (a_commands, a_receive) = broadcast::channel(8);
        let (b_commands, b_receive) = broadcast::channel(8);
        let (done, mut completed) = tokio::sync::mpsc::unbounded_channel();
        let b_done = done.clone();
        let a_task = tokio::spawn(async move {
            let mut state = Session::new(&a_video, Inputs { host: 17, peer: 15 });
            state.completed = Some(done);
            session(&mut left, &mut state, a_receive).await
        });
        let b_task = tokio::spawn(async move {
            let mut state = Session::new(&b_video, Inputs { host: 15, peer: 17 });
            state.completed = Some(b_done);
            session(&mut right, &mut state, b_receive).await
        });
        assert_eq!(a_writes.load(SeqCst) + b_writes.load(SeqCst), 0);
        for _ in 0..4 {
            let source_is_a = input.load(SeqCst) == 17;
            if source_is_a {
                a_commands.send(Command::Switch).unwrap();
            } else {
                b_commands.send(Command::Switch).unwrap();
            }
            for _ in 0..2 {
                timeout(Duration::from_secs(3), completed.recv())
                    .await
                    .unwrap()
                    .unwrap();
            }
            assert_eq!(input.load(SeqCst), if source_is_a { 15 } else { 17 });
        }
        assert_eq!(a_writes.load(SeqCst), 2);
        assert_eq!(b_writes.load(SeqCst), 2);
        a_task.abort();
        b_task.abort();
        let _ = tokio::join!(a_task, b_task);
    }
    #[tokio::test]
    async fn each_destination_selects_its_own_port_without_input_devices() {
        fixture(false, 17, false, false).await;
    }
    #[tokio::test]
    async fn starting_on_the_other_input_still_switches_on_the_first_click() {
        fixture(false, 15, false, false).await;
    }
    #[tokio::test]
    async fn inactive_ports_fall_back_to_the_active_pc_in_both_directions() {
        fixture(true, 17, false, false).await;
    }
    #[tokio::test]
    async fn slow_monitor_confirmation_never_repeats_an_issued_input_write() {
        fixture(false, 17, true, false).await;
    }
    #[tokio::test]
    async fn error_reported_after_an_accepted_write_does_not_trigger_a_second_write() {
        fixture(false, 15, true, true).await;
    }
}
