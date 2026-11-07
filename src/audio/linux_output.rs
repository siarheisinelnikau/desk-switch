use super::{
    link::FRAME_BYTES,
    playout::{Mix, Playout},
};
use anyhow::{Context, Result};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::AsyncWriteExt,
    sync::oneshot,
    task::JoinHandle,
    time::{MissedTickBehavior, interval},
};

pub struct Renderer {
    task: JoinHandle<()>,
    pub queue: Arc<Mutex<Playout>>,
    pub ready: oneshot::Receiver<String>,
    pub done: oneshot::Receiver<Result<()>>,
}

impl Renderer {
    pub fn start(output: Option<String>, mix: Arc<Mutex<Mix>>) -> Result<Self> {
        Self::start_media(output, mix, super::Media::System)
    }
    pub fn start_microphone(output: Option<String>, mix: Arc<Mutex<Mix>>) -> Result<Self> {
        Self::start_media(output, mix, super::Media::Microphone)
    }
    fn start_media(
        output: Option<String>,
        mix: Arc<Mutex<Mix>>,
        media: super::Media,
    ) -> Result<Self> {
        let queue = Arc::new(Mutex::new(Playout::new()));
        let playout = Arc::clone(&queue);
        let (send_ready, ready) = oneshot::channel();
        let (send_done, done) = oneshot::channel();
        let task = tokio::spawn(async move {
            let result = async {
                let (mut child, mut bridge) = if media == super::Media::Microphone {
                    let (child, bridge) = super::linux::microphone_playback().await?;
                    (child, Some(bridge))
                } else { (super::linux::playback(output.as_deref()).await?, None) };
                let mut samples = child.stdin.take().context("pw-play stdin unavailable")?;
                if send_ready
                    .send(output.unwrap_or_else(|| "default PipeWire output".into()))
                    .is_err()
                {
                    child.kill().await?;
                    return Ok(());
                }
                let mut tick = interval(Duration::from_millis(10));
                // Keep the original sample clock after scheduler jitter. Delay
                // would permanently slow down every subsequent audio block.
                tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
                let mut pcm = [0; FRAME_BYTES];
                loop {
                    tokio::select! {
                        status = async { match &mut bridge { Some(bridge) => bridge.wait().await, None => std::future::pending().await } } => {
                            anyhow::bail!("PipeWire microphone bridge ended ({})", status?);
                        }
                        status = child.wait() => {
                            anyhow::bail!("PipeWire playback ended ({})", status?);
                        }
                        _ = tick.tick() => {
                            let settings = *mix.lock().unwrap();
                            playout.lock().unwrap().render(&mut pcm, settings, Instant::now());
                            samples.write_all(&pcm).await.context("writing to PipeWire playback")?;
                        }
                    }
                }
            }
            .await;
            let _ = send_done.send(result);
        });
        Ok(Self {
            task,
            queue,
            ready,
            done,
        })
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
