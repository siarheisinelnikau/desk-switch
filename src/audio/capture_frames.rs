use super::link::{FRAME_BYTES, Pcm};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

// Real packets follow the capture device's clock. A wall clock is needed only
// for loopback endpoints that stop supplying packets when all apps are silent.
pub struct CaptureFrames {
    pub pending: VecDeque<u8>,
    silent_at: Instant,
}

impl CaptureFrames {
    pub fn new(now: Instant) -> Self {
        Self {
            pending: VecDeque::with_capacity(FRAME_BYTES * 4),
            silent_at: now + Duration::from_millis(20),
        }
    }

    pub fn take(&mut self, now: Instant) -> Option<Pcm> {
        while self.pending.len() > FRAME_BYTES * 4 {
            self.pending.drain(..FRAME_BYTES);
        }
        if self.pending.len() >= FRAME_BYTES {
            let mut pcm = [0; FRAME_BYTES];
            for byte in &mut pcm {
                *byte = self.pending.pop_front().unwrap();
            }
            self.silent_at = now + Duration::from_millis(20);
            Some(pcm)
        } else if now >= self.silent_at {
            // Do not pad every short packet: that changes the effective sample
            // rate. A partial packet is abandoned only after capture goes idle.
            self.pending.clear();
            self.silent_at =
                if now.saturating_duration_since(self.silent_at) > Duration::from_millis(40) {
                    now + Duration::from_millis(10)
                } else {
                    self.silent_at + Duration::from_millis(10)
                };
            Some([0; FRAME_BYTES])
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn irregular_polling_preserves_the_hardware_sample_rate() {
        let start = Instant::now();
        let mut frames = CaptureFrames::new(start);
        let mut emitted = 0;
        // Poll every 3 ms, as on Windows. The old wall-clock gate emitted just
        // 83 frames per second and discarded a sixth of the captured audio.
        let mut produced = 0;
        for ms in (0..=3000).step_by(3) {
            let due = ms / 10;
            while produced < due {
                frames.pending.extend(std::iter::repeat_n(7, FRAME_BYTES));
                produced += 1;
            }
            while let Some(pcm) = frames.take(start + Duration::from_millis(ms)) {
                assert!(pcm.iter().all(|&byte| byte == 7));
                emitted += 1;
            }
        }
        assert_eq!(emitted, 300);
        assert!(frames.pending.is_empty());
    }

    #[test]
    fn partial_packets_are_joined_and_idle_loopback_supplies_silence() {
        let now = Instant::now();
        let mut frames = CaptureFrames::new(now);
        frames
            .pending
            .extend(std::iter::repeat_n(3, FRAME_BYTES / 2));
        assert!(frames.take(now + Duration::from_millis(10)).is_none());
        frames
            .pending
            .extend(std::iter::repeat_n(5, FRAME_BYTES / 2));
        let pcm = frames.take(now + Duration::from_millis(12)).unwrap();
        assert!(pcm[..FRAME_BYTES / 2].iter().all(|&b| b == 3));
        assert!(pcm[FRAME_BYTES / 2..].iter().all(|&b| b == 5));
        assert_eq!(
            frames.take(now + Duration::from_millis(32)),
            Some([0; FRAME_BYTES])
        );
        assert!(frames.take(now + Duration::from_millis(33)).is_none());
    }
}
