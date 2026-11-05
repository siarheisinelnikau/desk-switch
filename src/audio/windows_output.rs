use super::{
    link::{BLOCK_ALIGN, CHANNELS, RATE},
    playout::{Mix, Playout},
};
use anyhow::{Context, Result, ensure};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Instant,
};
use tokio::sync::oneshot;
use wasapi::{
    AudioClient, AudioRenderClient, Device, DeviceEnumerator, DeviceState, Direction, Handle,
    SampleType, StreamMode, WaveFormat,
};

pub(super) struct Com;

impl Com {
    pub(super) fn open() -> Result<Self> {
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

pub fn devices() -> Result<Vec<super::AudioDevice>> {
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
    let mut result = Vec::new();
    for device in &devices {
        let device = device?;
        let id = device.get_id()?;
        result.push(super::AudioDevice {
            default: default.as_ref() == Some(&id),
            id,
            name: device.get_friendlyname()?,
        });
    }
    Ok(result)
}

pub fn list() -> Result<()> {
    for device in devices()? {
        println!(
            "{}{}\n  {}",
            device.name,
            if device.default { " [default]" } else { "" },
            device.id
        );
    }
    Ok(())
}

pub(super) fn output_device(id: Option<&str>) -> Result<Device> {
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

pub struct Renderer {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub queue: Arc<Mutex<Playout>>,
    pub ready: oneshot::Receiver<String>,
    pub done: oneshot::Receiver<Result<()>>,
}

impl Renderer {
    pub fn start(output: Option<String>, mix: Arc<Mutex<Mix>>) -> Result<Self> {
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
