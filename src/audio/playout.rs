use super::link::{BLOCK_ALIGN, CHANNELS, Pcm, RATE};
use std::{
    collections::VecDeque,
    sync::Mutex,
    time::{Duration, Instant},
};

const QUEUE_CAPACITY: usize = 12;
const PREBUFFER: usize = 4;
const MAX_PREBUFFER: usize = 8;
const MAX_QUEUE_AGE: Duration = Duration::from_millis(180);
const TRANSITION: i32 = RATE as i32 / 500; // 2 ms for a discontinuity.
const GAIN_RAMP: i32 = RATE as i32 / 200; // 5 ms for volume/mute changes.
const UNITY: i32 = 65536;

#[derive(Clone, Copy)]
pub struct Mix {
    pub volume: u8,
    pub muted: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Stats {
    pub underruns: u64,
    pub discarded: u64,
    pub capture_gaps: u64,
}
impl Stats {
    pub fn report(self) {
        if self != Self::default() {
            println!(
                "Audio continuity: {} underruns, {} discarded 10 ms blocks, {} missing capture blocks.",
                self.underruns, self.discarded, self.capture_gaps
            );
        }
    }
}

struct Frame {
    pcm: Pcm,
    queued_at: Instant,
    discontinuity: bool,
}

#[derive(Default)]
struct Continuity {
    last: [i32; CHANNELS],
    from: [i32; CHANNELS],
    remaining: i32,
    gain: Option<i32>,
    gain_from: i32,
    gain_target: i32,
    gain_remaining: i32,
}
impl Continuity {
    fn transition(&mut self) {
        self.from = self.last;
        self.remaining = TRANSITION;
    }
    fn mix(&mut self, mix: Mix) {
        let target = if mix.muted {
            0
        } else {
            i32::from(mix.volume) * UNITY / 100
        };
        match self.gain {
            None => {
                self.gain = Some(target);
                self.gain_target = target;
            }
            Some(gain) if target != self.gain_target => {
                self.gain_from = gain;
                self.gain_target = target;
                self.gain_remaining = GAIN_RAMP;
            }
            _ => {}
        }
    }
    fn sample(&mut self, input: [i16; CHANNELS]) -> [i16; CHANNELS] {
        if self.gain_remaining > 0 {
            self.gain_remaining -= 1;
            self.gain = Some(
                self.gain_target
                    + ((i64::from(self.gain_from - self.gain_target)
                        * i64::from(self.gain_remaining))
                        / i64::from(GAIN_RAMP)) as i32,
            );
        }
        let gain = self.gain.unwrap_or(UNITY);
        if self.remaining > 0 {
            self.remaining -= 1;
        }
        let mut output = [0; CHANNELS];
        for channel in 0..CHANNELS {
            let value = (i64::from(input[channel]) * i64::from(gain) / i64::from(UNITY)) as i32;
            let value = if self.remaining > 0 {
                (self.from[channel] * self.remaining + value * (TRANSITION - self.remaining))
                    / TRANSITION
            } else {
                value
            };
            self.last[channel] = value;
            output[channel] = value.clamp(i16::MIN.into(), i16::MAX.into()) as i16;
        }
        output
    }
}

pub struct Playout {
    queue: VecDeque<Frame>,
    next: Option<u64>,
    offset: usize,
    playing: bool,
    prebuffer: usize,
    discontinuity: bool,
    continuity: Continuity,
    stats: Stats,
}

impl Playout {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::with_capacity(QUEUE_CAPACITY),
            next: None,
            offset: 0,
            playing: false,
            prebuffer: PREBUFFER,
            discontinuity: true,
            continuity: Continuity::default(),
            stats: Stats::default(),
        }
    }

    pub fn push(&mut self, sequence: u64, pcm: Pcm, now: Instant) {
        if self.next.is_some_and(|next| sequence < next) {
            return;
        }
        let gap = self.next.map_or(0, |next| sequence.saturating_sub(next));
        self.stats.capture_gaps = self.stats.capture_gaps.saturating_add(gap);
        self.next = sequence.checked_add(1);
        if self.queue.len() == QUEUE_CAPACITY {
            self.queue.pop_front();
            self.offset = 0;
            self.stats.discarded += 1;
            self.discontinuity = true;
        }
        self.queue.push_back(Frame {
            pcm,
            queued_at: now,
            discontinuity: gap != 0,
        });
    }

    pub fn clear(&mut self) {
        self.queue.clear();
        self.next = None;
        self.offset = 0;
        self.playing = false;
        self.prebuffer = PREBUFFER;
        self.discontinuity = true;
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    pub fn render(&mut self, output: &mut [u8], mix: Mix, now: Instant) {
        debug_assert!(output.len().is_multiple_of(BLOCK_ALIGN));
        self.continuity.mix(mix);
        while self
            .queue
            .front()
            .is_some_and(|frame| now.saturating_duration_since(frame.queued_at) >= MAX_QUEUE_AGE)
        {
            self.queue.pop_front();
            self.offset = 0;
            self.playing = false;
            self.discontinuity = true;
            self.stats.discarded += 1;
        }
        if !self.playing && self.queue.len() >= self.prebuffer {
            self.playing = true;
            self.discontinuity = true;
        }
        for destination in output.as_chunks_mut::<BLOCK_ALIGN>().0 {
            let mut input = [0; CHANNELS];
            if self.playing {
                if let Some(frame) = self.queue.front() {
                    self.discontinuity |= self.offset == 0 && frame.discontinuity;
                    for (channel, sample) in input.iter_mut().enumerate() {
                        let index = self.offset + channel * 2;
                        *sample =
                            i16::from_le_bytes(frame.pcm[index..index + 2].try_into().unwrap());
                    }
                    self.offset += BLOCK_ALIGN;
                    if self.offset == frame.pcm.len() {
                        self.queue.pop_front();
                        self.offset = 0;
                    }
                } else {
                    self.playing = false;
                    self.prebuffer = (self.prebuffer + 1).min(MAX_PREBUFFER);
                    self.stats.underruns += 1;
                    self.discontinuity = true;
                }
            }
            if std::mem::take(&mut self.discontinuity) {
                self.continuity.transition();
            }
            let samples = self.continuity.sample(input);
            for (sample, bytes) in samples.iter().zip(destination.as_chunks_mut::<2>().0) {
                *bytes = sample.to_le_bytes();
            }
        }
    }
}

// Tied to one authenticated connection, including a cancelled receive.
pub struct ClearOnDrop<'a>(pub &'a Mutex<Playout>);

impl Drop for ClearOnDrop<'_> {
    fn drop(&mut self) {
        let mut queue = self.0.lock().unwrap();
        queue.stats().report();
        queue.clear();
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
        i16::from_le_bytes(out[out.len() - 2..].try_into().unwrap())
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
                (if tick == 0 {
                    &out[TRANSITION as usize * BLOCK_ALIGN..]
                } else {
                    &out[..]
                })
                .as_chunks::<2>()
                .0
                .iter()
                .all(|s| i16::from_le_bytes(*s) == 1200),
                "underrun at {tick}"
            );
        }
    }

    fn full() -> Mix {
        Mix {
            volume: 100,
            muted: false,
        }
    }
    fn bounded_steps(out: &Pcm, previous: i16, maximum: i32) {
        let mut previous = i32::from(previous);
        for pair in out.as_chunks::<BLOCK_ALIGN>().0 {
            let value = i32::from(i16::from_le_bytes(pair[..2].try_into().unwrap()));
            assert!(
                (value - previous).abs() <= maximum,
                "step {} exceeds {maximum}",
                (value - previous).abs()
            );
            previous = value;
        }
    }
    #[test]
    fn starvation_and_resume_fade_short_clicks_without_retaining_stale_audio() {
        let mut queue = Playout::new();
        let now = Instant::now();
        let mut out = [0; FRAME_BYTES];
        for seq in 0..4 {
            queue.push(seq, frame(24000), now);
        }
        for _ in 0..4 {
            queue.render(&mut out, full(), now);
        }
        assert_eq!(sample(&out), 24000);
        queue.render(&mut out, full(), now);
        bounded_steps(&out, 24000, 260);
        assert_eq!(sample(&out), 0);
        for seq in 8..13 {
            queue.push(seq, frame(-24000), now);
        }
        queue.render(&mut out, full(), now);
        bounded_steps(&out, 0, 260);
        assert_eq!(sample(&out), -24000);
        assert_eq!(queue.stats().underruns, 1);
        assert_eq!(queue.stats().capture_gaps, 4);
    }
    #[test]
    fn overflow_and_capture_discontinuities_crossfade_within_two_ms() {
        for overflow in [false, true] {
            let mut queue = Playout::new();
            let now = Instant::now();
            let mut out = [0; FRAME_BYTES];
            for seq in 0..4 {
                queue.push(seq, frame(24000), now);
            }
            queue.render(&mut out, full(), now);
            if overflow {
                for seq in 4..17 {
                    queue.push(seq, frame(-24000), now);
                }
                assert!(queue.stats().discarded > 0);
            } else {
                for _ in 0..3 {
                    queue.render(&mut out, full(), now);
                }
                queue.push(5, frame(-24000), now);
            }
            queue.render(&mut out, full(), now);
            bounded_steps(&out, 24000, 510);
            assert_eq!(sample(&out), -24000);
        }
    }
    #[test]
    fn volume_and_mute_ramp_but_normal_audio_stays_bit_exact() {
        let mut queue = Playout::new();
        let now = Instant::now();
        let mut out = [0; FRAME_BYTES];
        for seq in 0..8 {
            queue.push(seq, frame(24000), now);
        }
        queue.render(&mut out, full(), now);
        queue.render(
            &mut out,
            Mix {
                volume: 50,
                muted: false,
            },
            now,
        );
        bounded_steps(&out, 24000, 55);
        assert_eq!(sample(&out), 12000);
        queue.render(
            &mut out,
            Mix {
                volume: 50,
                muted: true,
            },
            now,
        );
        bounded_steps(&out, 12000, 55);
        assert_eq!(sample(&out), 0);
        queue.clear();
        let mut pcm = frame(0);
        for (n, pair) in pcm.as_chunks_mut::<BLOCK_ALIGN>().0.iter_mut().enumerate() {
            pair[..2]
                .copy_from_slice(&(if n % 2 == 0 { i16::MAX } else { i16::MIN }).to_le_bytes());
            pair[2..]
                .copy_from_slice(&(if n % 2 == 0 { i16::MIN } else { i16::MAX }).to_le_bytes());
        }
        for seq in 0..4 {
            queue.push(seq, pcm, now);
        }
        queue.render(&mut out, full(), now);
        queue.render(&mut out, full(), now);
        assert_eq!(
            out, pcm,
            "ordinary sample/frame boundaries must not be smoothed"
        );
    }
}
