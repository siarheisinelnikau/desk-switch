//! CoreAudio playback/microphone and ScreenCaptureKit system capture.
use super::{AudioDevice, Media};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::process::Stdio;

#[derive(Deserialize)]
pub(super) struct Devices {
    pub speakers: Vec<AudioDevice>,
    pub microphones: Vec<AudioDevice>,
    pub bridges: Vec<AudioDevice>,
}
pub(super) fn devices() -> Result<Devices> {
    crate::macos_native::query("devices", &[])
}
pub struct Capture {
    child: tokio::process::Child,
    samples: tokio::process::ChildStdout,
}
impl Capture {
    pub async fn open(source: Option<&str>) -> Result<Self> {
        Self::open_media(source, Media::System).await
    }
    pub async fn open_media(source: Option<&str>, media: Media) -> Result<Self> {
        // ScreenCaptureKit captures the system mix, not an arbitrary CoreAudio
        // device's loopback. Refuse a selected nondefault sink instead of
        // silently recording a different output.
        if media == Media::System
            && let Some(source) = source
        {
            ensure!(
                devices()?
                    .speakers
                    .iter()
                    .any(|device| device.id == source && device.default),
                "macOS system capture follows the default output. Select Automatic or make this output the macOS default"
            );
        }
        if media == Media::Microphone
            && let Some(source) = source
        {
            ensure!(
                devices()?
                    .microphones
                    .iter()
                    .any(|device| device.id == source),
                "The selected physical macOS microphone is unavailable"
            );
        }
        let mut command = crate::macos_native::async_command(if media == Media::System {
            "capture-system"
        } else {
            "capture-microphone"
        })?;
        if media == Media::Microphone
            && let Some(source) = source
        {
            command.arg(source);
        }
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Starting macOS audio capture")?;
        let label = crate::macos_native::ready(&mut child).await?;
        let samples = child
            .stdout
            .take()
            .context("macOS audio capture pipe is unavailable")?;
        println!("Capturing {media:?} from {label}.");
        Ok(Self { child, samples })
    }
    pub async fn stop(&mut self) -> Result<()> {
        self.child
            .kill()
            .await
            .context("Stopping macOS audio capture")
    }
}
impl tokio::io::AsyncRead for Capture {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.samples).poll_read(cx, buffer)
    }
}
pub(super) async fn playback(
    output: Option<&str>,
    media: Media,
) -> Result<(tokio::process::Child, String)> {
    let mut command = crate::macos_native::async_command("playback")?;
    if media == Media::Microphone {
        let bridges = devices()?.bridges;
        let selected=match output {Some(id)=>bridges.iter().find(|device|device.id==id),None=>bridges.iter().find(|device|device.default).or_else(||bridges.first())}.context("Install VB-CABLE or BlackHole on this Mac to receive a shared microphone, then select its virtual playback endpoint")?;
        command.arg(&selected.id);
    } else if let Some(output) = output {
        ensure!(
            devices()?.speakers.iter().any(|device| device.id == output),
            "The selected macOS playback device is unavailable"
        );
        command.arg(output);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("Starting CoreAudio playback")?;
    let label = crate::macos_native::ready(&mut child).await?;
    Ok((child, label))
}
