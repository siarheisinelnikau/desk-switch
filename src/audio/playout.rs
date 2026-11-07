use super::link::{BLOCK_ALIGN, Pcm};
use std::{
    collections::VecDeque,
    sync::Mutex,
    time::{Duration, Instant},
};

const QUEUE_CAPACITY: usize = 12;
const PREBUFFER: usize = 4;
const MAX_PREBUFFER: usize = 8;
const MAX_QUEUE_AGE: Duration = Duration::from_millis(180);

#[derive(Clone, Copy)]
pub struct Mix {
    pub volume: u8,
    pub muted: bool,
}

struct Frame {
    pcm: Pcm,
    queued_at: Instant,
}

pub struct Playout {
    queue: VecDeque<Frame>,
    next: Option<u64>,
    offset: usize,
    playing: bool,
    prebuffer: usize,
}

impl Playout {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::with_capacity(QUEUE_CAPACITY),
            next: None,
            offset: 0,
            playing: false,
            prebuffer: PREBUFFER,
        }
    }

    pub fn push(&mut self, sequence: u64, pcm: Pcm, now: Instant) {
        if let Some(next) = self.next
            && sequence < next
        {
            return;
        }
        // A dropped capture frame must not also erase valid buffered audio.
        self.next = sequence.checked_add(1);
        if self.queue.len() == QUEUE_CAPACITY {
            self.queue.pop_front();
            self.offset = 0;
        }
        self.queue.push_back(Frame {
            pcm,
            queued_at: now,
        });
    }

    pub fn clear(&mut self) {
        self.queue.clear();
        self.next = None;
        self.offset = 0;
        self.playing = false;
        self.prebuffer = PREBUFFER;
    }

    pub fn render(&mut self, output: &mut [u8], mix: Mix, now: Instant) {
        debug_assert!(output.len().is_multiple_of(BLOCK_ALIGN));
        output.fill(0);
        while self
            .queue
            .front()
            .is_some_and(|frame| now.saturating_duration_since(frame.queued_at) >= MAX_QUEUE_AGE)
        {
            self.queue.pop_front();
            self.offset = 0;
            self.playing = false;
        }
        if !self.playing {
            self.playing = self.queue.len() >= self.prebuffer;
            if !self.playing {
                return;
            }
        }

        let mut written = 0;
        while written < output.len() {
            let Some(frame) = self.queue.front() else {
                self.playing = false;
                self.prebuffer = (self.prebuffer + 1).min(MAX_PREBUFFER);
                break;
            };
            let count = (output.len() - written).min(frame.pcm.len() - self.offset);
            if !mix.muted && mix.volume != 0 {
                let samples = &frame.pcm[self.offset..self.offset + count];
                let target = &mut output[written..written + count];
                if mix.volume == 100 {
                    target.copy_from_slice(samples);
                } else {
                    for (sample, scaled) in samples
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .zip(target.as_chunks_mut::<2>().0)
                    {
                        let value =
                            i32::from(i16::from_le_bytes(*sample)) * i32::from(mix.volume) / 100;
                        *scaled = (value as i16).to_le_bytes();
                    }
                }
            }
            self.offset += count;
            written += count;
            if self.offset == frame.pcm.len() {
                self.queue.pop_front();
                self.offset = 0;
            }
        }
    }
}

// Tied to one authenticated connection, including a cancelled receive.
pub struct ClearOnDrop<'a>(pub &'a Mutex<Playout>);

impl Drop for ClearOnDrop<'_> {
    fn drop(&mut self) {
        self.0.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::link::FRAME_BYTES;

    fn frame(value: i16) -> Pcm {
        let mut pcm = [0; FRAME_BYTES];
        for sample in pcm.as_chunks_mut::<2>().0 {
            *sample = value.to_le_bytes();
        }
        pcm
    }

    fn sample(out: &Pcm) -> i16 {
        i16::from_le_bytes(out[..2].try_into().unwrap())
    }

    #[test]
    fn mixes_only_the_remote_stream_and_respects_mute() {
        let mut queue = Playout::new();
        let now = Instant::now();
        let mut out = [0; FRAME_BYTES];
        queue.push(0, frame(1200), now);
        queue.render(
            &mut out,
            Mix {
                volume: 100,
                muted: false,
            },
            now,
        );
        assert_eq!(sample(&out), 0);
        queue.push(1, frame(-800), now);
        queue.push(2, frame(400), now);
        queue.push(3, frame(400), now);
        queue.render(
            &mut out,
            Mix {
                volume: 50,
                muted: false,
            },
            now,
        );
        assert_eq!(sample(&out), 600);
        queue.render(
            &mut out,
            Mix {
                volume: 50,
                muted: true,
            },
            now,
        );
        assert_eq!(sample(&out), 0);
    }

    #[test]
    fn bounds_latency_but_preserves_buffered_frames_across_capture_gaps() {
        let mut queue = Playout::new();
        let now = Instant::now();
        let mut out = [0; FRAME_BYTES];
        let mix = Mix {
            volume: 100,
            muted: false,
        };
        for sequence in 0..16 {
            queue.push(sequence, frame(sequence as i16 + 1), now);
        }
        queue.render(&mut out, mix, now);
        assert_eq!(sample(&out), 5);
        let later = now + MAX_QUEUE_AGE;
        queue.render(&mut out, mix, later);
        assert_eq!(sample(&out), 0);
        queue.push(16, frame(10), later);
        queue.push(19, frame(11), later);
        queue.render(&mut out, mix, later);
        assert_eq!(sample(&out), 0);
        queue.push(20, frame(12), later);
        queue.push(21, frame(13), later);
        queue.render(&mut out, mix, later);
        assert_eq!(sample(&out), 10);
        queue.clear();
        queue.render(&mut out, mix, later);
        assert_eq!(sample(&out), 0);
    }

    #[test]
    fn plays_continuously_when_network_delivers_forty_ms_bursts() {
        let mut queue = Playout::new();
        let start = Instant::now();
        let mix = Mix {
            volume: 100,
            muted: false,
        };
        let mut out = [0; FRAME_BYTES];
        let mut sequence = 0;
        for tick in 0..500 {
            let now = start + Duration::from_millis(tick * 10);
            if tick % 4 == 0 {
                for _ in 0..4 {
                    queue.push(sequence, frame(1200), now);
                    sequence += 1;
                }
            }
            queue.render(&mut out, mix, now);
            assert!(
                out.as_chunks::<2>()
                    .0
                    .iter()
                    .all(|s| i16::from_le_bytes(*s) == 1200),
                "underrun at {tick}"
            );
        }
    }
}
