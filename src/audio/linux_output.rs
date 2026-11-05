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
        let queue = Arc::new(Mutex::new(Playout::new()));
        let playout = Arc::clone(&queue);
        let (send_ready, ready) = oneshot::channel();
        let (send_done, done) = oneshot::channel();
        let task = tokio::spawn(async move {
            let result = async {
                let mut child = super::linux::playback(output.as_deref()).await?;
                let mut samples = child.stdin.take().context("pw-play stdin unavailable")?;
                if send_ready
                    .send(output.unwrap_or_else(|| "default PipeWire output".into()))
                    .is_err()
                {
                    child.kill().await?;
                    return Ok(());
                }
                let mut tick = interval(Duration::from_millis(10));
                tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
                let mut pcm = [0; FRAME_BYTES];
                loop {
                    tokio::select! {
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
