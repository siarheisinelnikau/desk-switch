use super::{
    link::{BLOCK_ALIGN, CHANNELS, Link, RATE},
    playout::{Mix, Playout},
    stream,
};
use crate::config::{Identity, Key};
use anyhow::{Context, Result, bail, ensure};
use std::{
    io::{self, BufRead},
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tokio::{
    net::TcpStream,
    sync::{mpsc, oneshot},
    time::{sleep, timeout},
};
use wasapi::{
    AudioClient, AudioRenderClient, Device, DeviceEnumerator, DeviceState, Direction, Handle,
    SampleType, StreamMode, WaveFormat,
};

struct Com;

impl Com {
    fn open() -> Result<Self> {
        wasapi::initialize_mta()
            .ok()
            .context("initializing WASAPI COM")?;
        Ok(Self)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        wasapi::deinitialize();
    }
}

pub fn list() -> Result<()> {
    let _com = Com::open()?;
    let enumerator = DeviceEnumerator::new()?;
    let default = enumerator
        .get_default_device(&Direction::Render)
        .and_then(|device| device.get_id())
        .ok();
    let devices = enumerator.get_device_collection(&Direction::Render)?;
    ensure!(
        devices.get_nbr_devices()? > 0,
        "no active Windows playback outputs"
    );
    for device in &devices {
        let device = device?;
        let id = device.get_id()?;
        let marker = if default.as_ref() == Some(&id) {
            " [default]"
        } else {
            ""
        };
        println!("{}{}\n  {}", device.get_friendlyname()?, marker, id);
    }
    Ok(())
}

fn output_device(id: Option<&str>) -> Result<Device> {
    let enumerator = DeviceEnumerator::new()?;
    let device = match id {
        Some(id) => enumerator
            .get_device(id)
            .context("the selected Windows output is unavailable")?,
        None => enumerator
            .get_default_device(&Direction::Render)
            .context("no default Windows playback output")?,
    };
    ensure!(
        device.get_direction() == Direction::Render && device.get_state()? == DeviceState::Active,
        "the selected device must be an active Windows playback output"
    );
    Ok(device)
}

struct Output {
    label: String,
    audio: AudioClient,
    render: AudioRenderClient,
    event: Handle,
    capacity: u32,
    target: u32,
    data: Vec<u8>,
}

impl Output {
    fn open(id: Option<&str>) -> Result<Self> {
        let device = output_device(id)?;
        let label = device.get_friendlyname()?;
        let mut audio = device.get_iaudioclient()?;
        let (period, _) = audio.get_device_period()?;
        let mode = StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: period.saturating_mul(2).clamp(200_000, 1_000_000),
        };
        let format = WaveFormat::new(16, 16, &SampleType::Int, RATE as usize, CHANNELS, None);
        audio
            .initialize_client(&format, &Direction::Render, &mode)
            .context("opening shared-mode WASAPI playback")?;
        let event = audio.set_get_eventhandle()?;
        let render = audio.get_audiorenderclient()?;
        let capacity = audio.get_buffer_size()?;
        // Keep the app's queued audio bounded even if an endpoint offers a large buffer.
        let target = capacity.min(RATE / 10);
        ensure!(
            target > 0,
            "the selected Windows output has no render buffer"
        );
        Ok(Self {
            label,
            audio,
            render,
            event,
            capacity,
            target,
            data: vec![0; target as usize * BLOCK_ALIGN],
        })
    }

    fn run(&mut self, stop: &AtomicBool, queue: &Mutex<Playout>, mix: &Mutex<Mix>) -> Result<()> {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        self.render
            .write_to_device(self.target as usize, &self.data, None)?;
        self.audio.start_stream()?;
        while !stop.load(Ordering::Acquire) {
            // A short bounded wait allows Off to close the device even if no event arrives.
            let _ = self.event.wait_for_event(30);
            if stop.load(Ordering::Acquire) {
                break;
            }
            let available = self.audio.get_available_space_in_frames()?;
            let frames = self
                .target
                .saturating_sub(self.capacity.saturating_sub(available))
                .min(available);
            if frames == 0 {
                continue;
            }
            let samples = &mut self.data[..frames as usize * BLOCK_ALIGN];
            let settings = *mix.lock().unwrap();
            queue
                .lock()
                .unwrap()
                .render(samples, settings, Instant::now());
            self.render
                .write_to_device(frames as usize, samples, None)?;
        }
        Ok(())
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        let _ = self.audio.stop_stream();
        let _ = self.audio.reset_stream();
    }
}

struct Renderer {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    queue: Arc<Mutex<Playout>>,
    ready: oneshot::Receiver<String>,
    done: oneshot::Receiver<Result<()>>,
}

impl Renderer {
    fn start(output: Option<String>, mix: Arc<Mutex<Mix>>) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let queue = Arc::new(Mutex::new(Playout::new()));
        let (ready_send, ready) = oneshot::channel();
        let (done_send, done) = oneshot::channel();
        let stop_flag = Arc::clone(&stop);
        let worker_queue = Arc::clone(&queue);
        let thread = thread::Builder::new()
            .name("Desk Switch WASAPI".into())
            .spawn(move || {
                let result = (|| {
                    let _com = Com::open()?;
                    let mut output = Output::open(output.as_deref())?;
                    if ready_send.send(output.label.clone()).is_ok() {
                        output.run(&stop_flag, &worker_queue, &mix)?;
                    }
                    Ok(())
                })();
                let _ = done_send.send(result);
            })?;
        Ok(Self {
            stop,
            thread: Some(thread),
            queue,
            ready,
            done,
        })
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Clone)]
struct Player {
    address: SocketAddr,
    identity: Arc<Identity>,
    peer: Key,
    output: Option<String>,
    mix: Arc<Mutex<Mix>>,
    status: Arc<Mutex<String>>,
}

impl Player {
    fn status(&self, text: String) {
        let mut status = self.status.lock().unwrap();
        if *status != text {
            println!("Ubuntu audio: {text}");
            *status = text;
        }
    }

    async fn session(&self) -> Result<()> {
        let socket = timeout(Duration::from_secs(2), TcpStream::connect(self.address))
            .await
            .context("connecting to the Ubuntu audio port timed out")??;
        let mut link = Link::open(socket, &self.identity, &self.peer, true).await?;
        let mut renderer = Renderer::start(self.output.clone(), Arc::clone(&self.mix))?;
        let output = match (&mut renderer.ready).await {
            Ok(output) => output,
            Err(_) => {
                (&mut renderer.done)
                    .await
                    .context("Windows audio worker stopped during startup")??;
                bail!("Windows audio output did not start");
            }
        };
        self.status(format!("paired; waiting for samples on {output}"));
        let mut started = false;
        tokio::select! {
            result = stream::receive(&mut link, &renderer.queue, |_| {
                if !started {
                    self.status(format!("playing on {output}"));
                    started = true;
                }
            }) => result,
            result = &mut renderer.done => {
                result.context("Windows audio worker stopped")??;
                bail!("Windows audio output stopped");
            }
        }
    }

    async fn reconnect(self) {
        loop {
            if let Err(error) = self.session().await {
                self.status(format!("reconnecting ({error:#})"));
            }
            sleep(Duration::from_secs(1)).await;
        }
    }
}

pub async fn play(
    address: SocketAddr,
    identity: Identity,
    peer: Key,
    output: Option<String>,
    mix: Mix,
) -> Result<()> {
    let player = Player {
        address,
        identity: Arc::new(identity),
        peer,
        output,
        mix: Arc::new(Mutex::new(mix)),
        status: Arc::new(Mutex::new(String::new())),
    };
    let (send, mut commands) = mpsc::channel(8);
    thread::Builder::new()
        .name("Desk Switch audio console".into())
        .spawn(move || {
            for line in io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                if send.blocking_send(line).is_err() {
                    break;
                }
            }
        })?;
    println!("Audio commands: on, off, volume 0–100, mute, unmute, status, quit");
    player.status(format!("connecting to {address}"));
    let mut task = Some(tokio::spawn(player.clone().reconnect()));
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    let mut read_commands = true;
    let signal = loop {
        tokio::select! {
            result = &mut shutdown => break result,
            command = commands.recv(), if read_commands => {
                let Some(command) = command else { read_commands = false; continue };
                match command.split_whitespace().collect::<Vec<_>>().as_slice() {
                    ["on"] if task.is_none() => {
                        player.status(format!("connecting to {address}"));
                        task = Some(tokio::spawn(player.clone().reconnect()));
                    }
                    ["on"] => println!("Ubuntu audio is already enabled."),
                    ["off"] => {
                        stop(&mut task).await;
                        player.status("off".into());
                    }
                    ["mute"] => {
                        player.mix.lock().unwrap().muted = true;
                        println!("Ubuntu audio muted.");
                    }
                    ["unmute"] => {
                        player.mix.lock().unwrap().muted = false;
                        println!("Ubuntu audio unmuted.");
                    }
                    ["volume", value] => match super::volume(value) {
                        Ok(volume) => {
                            player.mix.lock().unwrap().volume = volume;
                            println!("Ubuntu audio volume {volume}%.");
                        }
                        Err(error) => eprintln!("{error}"),
                    },
                    ["status"] => {
                        let mix = *player.mix.lock().unwrap();
                        println!("Ubuntu audio: {} | {}%{}", player.status.lock().unwrap(), mix.volume,
                            if mix.muted { " | muted" } else { "" });
                    }
                    ["quit"] => break Ok(()),
                    [] => {}
                    _ => eprintln!("Use on, off, volume 0–100, mute, unmute, status, or quit."),
                }
            }
        }
    };
    stop(&mut task).await;
    player.status("off".into());
    Ok(signal?)
}

async fn stop(task: &mut Option<tokio::task::JoinHandle<()>>) {
    if let Some(task) = task.take() {
        task.abort();
        let _ = task.await;
    }
}
