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
use windows_sys::{
    Win32::{
        Foundation::HANDLE,
        System::Threading::{AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW},
    },
    core::w,
};

struct AudioScheduling(HANDLE);
impl AudioScheduling {
    fn new() -> Self {
        let mut task = 0;
        // SAFETY: the task out-parameter is a live `u32` and the returned
        // thread handle is owned by this guard, which reverts it on drop.
        let handle = unsafe { AvSetMmThreadCharacteristicsW(w!("Audio"), &mut task) };
        if handle.is_null() {
            eprintln!(
                "WASAPI: multimedia scheduling is unavailable; using the normal event-driven audio thread."
            );
        }
        Self(handle)
    }
}
impl Drop for AudioScheduling {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the handle was returned by
            // AvSetMmThreadCharacteristicsW and is reverted exactly once.
            // SAFETY: the handle was returned by AvSetMmThreadCharacteristicsW and is
            // reverted exactly once.
            unsafe {
                AvRevertMmThreadCharacteristics(self.0);
            }
        }
    }
}

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
    Ok(all_outputs()?
        .into_iter()
        .filter(|d| !is_microphone_bridge(&d.name))
        .collect())
}

fn all_outputs() -> Result<Vec<super::AudioDevice>> {
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

pub fn microphones() -> Result<Vec<super::AudioDevice>> {
    let _com = Com::open()?;
    let enumerator = DeviceEnumerator::new()?;
    let default = enumerator
        .get_default_device(&Direction::Capture)
        .and_then(|d| d.get_id())
        .ok();
    let mut result = Vec::new();
    for device in &enumerator.get_device_collection(&Direction::Capture)? {
        let device = device?;
        let id = device.get_id()?;
        let name = device.get_friendlyname()?;
        if is_cable_recording(&name) {
            continue;
        }
        result.push(super::AudioDevice {
            default: default.as_ref() == Some(&id),
            id,
            name,
        });
    }
    Ok(result)
}

fn is_cable_recording(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.contains("vb-audio")
        || name.contains("vb-cable")
        || name.contains("cable output")
        || name.contains("voicemeeter")
        || name.contains("desk switch")
        || name.contains("desk-switch")
}

pub(super) fn is_microphone_bridge(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.contains("cable input") && (name.contains("vb-audio") || name.contains("vb-cable"))
}

pub fn microphone_bridges() -> Result<Vec<super::AudioDevice>> {
    // Only known render-to-capture cables are eligible. Keep this list separate
    // from speakers so system audio can never be routed into the microphone.
    Ok(all_outputs()?
        .into_iter()
        .filter(|d| is_microphone_bridge(&d.name))
        .collect())
}

pub(super) fn microphone_device(id: Option<&str>) -> Result<Device> {
    let enumerator = DeviceEnumerator::new()?;
    let device = match id {
        Some(id) => enumerator
            .get_device(id)
            .context("the selected Windows microphone is unavailable")?,
        None => enumerator
            .get_default_device(&Direction::Capture)
            .context("no default Windows microphone")?,
    };
    ensure!(
        device.get_direction() == Direction::Capture
            && device.get_state()? == DeviceState::Active
            && !is_cable_recording(&device.get_friendlyname()?),
        "select an active physical microphone"
    );
    Ok(device)
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
    render_device(id, false)
}

fn render_device(id: Option<&str>, microphone: bool) -> Result<Device> {
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
    ensure!(
        is_microphone_bridge(&device.get_friendlyname()?) == microphone,
        "Choose regular headphones/speakers for system audio and VB-CABLE only for the microphone"
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
    fn open(id: Option<&str>, media: super::Media) -> Result<Self> {
        let device = render_device(id, media == super::Media::Microphone)?;
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
        let session: windows::Win32::Media::Audio::IAudioSessionControl =
            // SAFETY: the returned interface is owned by this playback loop
            // and is released when it ends.
            unsafe { audio.as_iaudioclient().GetService()? };
        // SAFETY: the session interface is owned by this loop and the null
        // event pointer is allowed by the API.
        // SAFETY: the session interface is owned by this loop and the null event
        // pointer is allowed by the API.
        unsafe {
            session.SetGroupingParam(&super::SESSION_GROUP, std::ptr::null())?;
        }
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
    pub fn start_microphone(output: Option<String>, mix: Arc<Mutex<Mix>>) -> Result<Self> {
        let bridges = microphone_bridges()?;
        let bridge = crate::workspace::devices::selected(&bridges, output.as_deref())
            .context("Install VB-CABLE on this PC, then select CABLE Output as microphone in your calling app")?;
        Self::start_media(Some(bridge.id.clone()), mix, super::Media::Microphone)
    }

    pub fn start(output: Option<String>, mix: Arc<Mutex<Mix>>) -> Result<Self> {
        Self::start_media(output, mix, super::Media::System)
    }
    fn start_media(
        output: Option<String>,
        mix: Arc<Mutex<Mix>>,
        media: super::Media,
    ) -> Result<Self> {
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
                    let mut output = Output::open(output.as_deref(), media)?;
                    let _scheduling = AudioScheduling::new();
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

#[cfg(test)]
mod device_tests {
    #[test]
    fn only_the_microphone_cable_is_classified_as_a_bridge() {
        assert!(super::is_microphone_bridge(
            "CABLE Input (VB-Audio Virtual Cable)"
        ));
        for name in [
            "Headphones (USB)",
            "HDMI Speakers",
            "Microphone (Realtek)",
            "CABLE Output (VB-Audio)",
        ] {
            assert!(!super::is_microphone_bridge(name));
        }
    }
}
