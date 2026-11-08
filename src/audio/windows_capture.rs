use super::{
    link::{BLOCK_ALIGN, CHANNELS, FRAME_BYTES, Pcm, RATE},
    output::{Com, output_device},
};
use anyhow::{Context, Result, ensure};
use std::{
    io,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context as TaskContext, Poll},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, ReadBuf},
    sync::{mpsc, oneshot},
};
use wasapi::{Direction, SampleType, StreamMode, WaveFormat};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT, IAudioCaptureClient,
};

struct TimedPcm {
    pcm: Pcm,
    time: Instant,
}

pub struct Capture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    receive: mpsc::Receiver<TimedPcm>,
    current: Option<TimedPcm>,
    offset: usize,
}

impl Capture {
    pub async fn open(source: Option<&str>) -> Result<Self> {
        Self::open_media(source, super::Media::System).await
    }
    pub async fn open_media(source: Option<&str>, media: super::Media) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let (send, receive) = mpsc::channel(4);
        let (ready_send, ready) = oneshot::channel();
        let source = source.map(str::to_owned);
        let thread = thread::Builder::new()
            .name("Desk Switch capture".into())
            .spawn(move || {
                let mut ready_send = Some(ready_send);
                let result = (|| {
                    let _com = Com::open()?;
                    let device = match media {
                        super::Media::System => output_device(source.as_deref())?,
                        super::Media::Microphone => {
                            super::output::microphone_device(source.as_deref())?
                        }
                    };
                    let label = device.get_friendlyname()?;
                    let mut audio = device.get_iaudioclient()?;
                    let format =
                        WaveFormat::new(16, 16, &SampleType::Int, RATE as usize, CHANNELS, None);
                    audio
                        .initialize_client(
                            &format,
                            &Direction::Capture,
                            &StreamMode::PollingShared {
                                autoconvert: true,
                                buffer_duration_hns: 600_000,
                            },
                        )
                        .context("initializing shared WASAPI capture capture at 48 kHz stereo")?;
                    let capture: IAudioCaptureClient =
                        // SAFETY: the returned interface is owned by this
                        // capture loop and is released when it ends.
                        unsafe { audio.as_iaudioclient().GetService()? };
                    audio.start_stream()?;
                    println!("Capturing {media:?} from {label}.");
                    if ready_send.take().unwrap().send(Ok(())).is_err() {
                        audio.stop_stream()?;
                        return Ok(());
                    }
                    let result = record(&capture, &worker_stop, &send);
                    let _ = audio.stop_stream();
                    let _ = audio.reset_stream();
                    result
                })();
                if let Err(error) = result {
                    if let Some(ready) = ready_send {
                        let _ = ready.send(Err(error));
                    } else {
                        eprintln!("WASAPI capture stopped: {error:#}");
                    }
                }
            })?;
        let result = Self {
            stop,
            thread: Some(thread),
            receive,
            current: None,
            offset: 0,
        };
        ready
            .await
            .context("WASAPI capture worker stopped during setup")??;
        Ok(result)
    }

    pub async fn stop(&mut self) -> Result<()> {
        self.close();
        Ok(())
    }

    fn close(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
        self.current = None;
        self.receive.close();
    }
}

fn record(
    capture: &IAudioCaptureClient,
    stop: &AtomicBool,
    send: &mpsc::Sender<TimedPcm>,
) -> Result<()> {
    let mut buffer = super::capture_frames::CaptureFrames::new(Instant::now());
    while !stop.load(Ordering::Acquire) {
        // SAFETY: the capture client is owned by this loop and is only used
        // while it is alive.
        while unsafe { capture.GetNextPacketSize()? } != 0 {
            let mut pointer = std::ptr::null_mut();
            let mut frames = 0;
            let mut flags = 0;
            unsafe {
                // SAFETY: the three out-parameters are live locals and the
                // null event arguments are allowed by the API.
                capture.GetBuffer(&mut pointer, &mut frames, &mut flags, None, None)?;
            }
            let bytes = frames as usize * BLOCK_ALIGN;
            let read = (|| {
                ensure!(
                    bytes <= RATE as usize / 5 * BLOCK_ALIGN,
                    "WASAPI capture packet exceeds its buffer bound"
                );
                if flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0 {
                    buffer.pending.clear();
                }
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                    buffer.pending.extend(std::iter::repeat_n(0, bytes));
                } else {
                    ensure!(
                        bytes == 0 || !pointer.is_null(),
                        "WASAPI returned a null audio buffer"
                    );
                    if bytes != 0 {
                        buffer.pending.extend(unsafe {
                            // SAFETY: the pointer is the capture buffer that
                            // WASAPI just handed out and it holds exactly
                            // `frames` frames of `BLOCK_ALIGN` bytes. The
                            // slice is copied before the buffer is released.
                            std::slice::from_raw_parts(pointer, bytes)
                        });
                    }
                }
                Ok::<_, anyhow::Error>(())
            })();
            unsafe {
                // SAFETY: the same frame count that GetBuffer reported is
                // handed back, which is what the API requires.
                capture.ReleaseBuffer(frames)?;
            }
            read?;
        }
        let now = Instant::now();
        while let Some(pcm) = buffer.take(now) {
            match send.try_send(TimedPcm { pcm, time: now }) {
                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                Err(mpsc::error::TrySendError::Closed(_)) => return Ok(()),
            }
        }
        thread::park_timeout(Duration::from_millis(3));
    }
    Ok(())
}

impl AsyncRead for Capture {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            if let Some(current) = &self.current {
                if current.time.elapsed() >= Duration::from_millis(60) {
                    self.current = None;
                    self.offset = 0;
                    continue;
                }
                let count = output.remaining().min(FRAME_BYTES - self.offset);
                output.put_slice(&current.pcm[self.offset..self.offset + count]);
                self.offset += count;
                if self.offset == FRAME_BYTES {
                    self.current = None;
                    self.offset = 0;
                }
                return Poll::Ready(Ok(()));
            }
            match self.receive.poll_recv(cx) {
                Poll::Ready(Some(pcm)) => self.current = Some(pcm),
                Poll::Ready(None) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "WASAPI capture ended",
                    )));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.close();
    }
}
