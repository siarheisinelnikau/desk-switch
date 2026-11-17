//! Ephemeral worker-to-GUI feedback. Never sent over the network or logged.
use crate::wire::Input;
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const PREFIX: &str = "@desk-switch-feedback ";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub keys: [u64; 4],
    pub released: [u64; 4],
    pub buttons: u8,
    pub released_buttons: u8,
    pub motion: Option<[i32; 2]>,
    pub wheel: bool,
}
impl Snapshot {
    pub fn key(&self, code: u16) -> bool {
        code < 256 && self.keys[usize::from(code) / 64] & (1 << (code % 64)) != 0
    }
    pub fn was_released(&self, code: u16) -> bool {
        code < 256 && self.released[usize::from(code) / 64] & (1 << (code % 64)) != 0
    }
    pub fn merge(&mut self, newer: Self) {
        self.keys = newer.keys;
        self.buttons = newer.buttons;
        self.released_buttons |= newer.released_buttons;
        for (old, new) in self.released.iter_mut().zip(newer.released) {
            *old |= new;
        }
        self.motion = newer.motion.or(self.motion);
        self.wheel |= newer.wheel;
    }
}

#[derive(Default)]
pub(crate) struct Reporter {
    state: Snapshot,
    last: Option<Instant>,
}
impl Reporter {
    pub(crate) fn record(&mut self, event: Input, now: Instant) -> Option<Snapshot> {
        if !event.valid() {
            return None;
        }
        let important = match event {
            Input::Key { code, down } => {
                let previous = self.state.key(code);
                let bit = 1 << (code % 64);
                if down {
                    self.state.keys[usize::from(code) / 64] |= bit;
                } else {
                    self.state.keys[usize::from(code) / 64] &= !bit;
                    self.state.released[usize::from(code) / 64] |= bit;
                }
                previous != down
            }
            Input::Button { code, down } => {
                let bit = 1 << (code - 272);
                let previous = self.state.buttons & bit != 0;
                if down {
                    self.state.buttons |= bit;
                } else {
                    self.state.buttons &= !bit;
                    self.state.released_buttons |= bit;
                }
                previous != down
            }
            Input::Move { dx, dy } => {
                self.state.motion = Some([dx, dy]);
                false
            }
            Input::Scroll { .. } => {
                self.state.wheel = true;
                false
            }
        };
        if !important
            && self
                .last
                .is_some_and(|last| now.duration_since(last) < Duration::from_millis(60))
        {
            return None;
        }
        self.last = Some(now);
        let result = self.state.clone();
        self.state.released = [0; 4];
        self.state.released_buttons = 0;
        self.state.motion = None;
        self.state.wheel = false;
        Some(result)
    }
    pub(crate) fn release(&mut self) -> Option<Snapshot> {
        let old = std::mem::take(&mut self.state);
        self.last = None;
        (old.keys != [0; 4] || old.buttons != 0).then_some(Snapshot {
            released: old.keys,
            released_buttons: old.buttons,
            ..Default::default()
        })
    }
}

fn reporter() -> &'static Mutex<Reporter> {
    static REPORTER: OnceLock<Mutex<Reporter>> = OnceLock::new();
    REPORTER.get_or_init(Default::default)
}
pub fn record(event: Input) {
    if !crate::lifecycle::managed() {
        return;
    }
    if let Some(value) = reporter().lock().unwrap().record(event, Instant::now())
        && let Ok(json) = serde_json::to_string(&value)
    {
        use std::io::Write;
        let _ = writeln!(std::io::stdout().lock(), "{PREFIX}{json}");
    }
}
pub fn release() {
    if !crate::lifecycle::managed() {
        return;
    }
    if let Some(value) = reporter().lock().unwrap().release()
        && let Ok(json) = serde_json::to_string(&value)
    {
        use std::io::Write;
        let _ = writeln!(std::io::stdout().lock(), "{PREFIX}{json}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quick_releases_survive_motion_coalescing_and_merge_without_a_key_history() {
        let now = Instant::now();
        let mut reporter = Reporter::default();
        let mut down = reporter
            .record(
                Input::Key {
                    code: 30,
                    down: true,
                },
                now,
            )
            .unwrap();
        assert!(down.key(30));
        assert!(
            reporter
                .record(Input::Move { dx: 2, dy: -1 }, now)
                .is_none()
        );
        down.merge(
            reporter
                .record(
                    Input::Key {
                        code: 30,
                        down: false,
                    },
                    now,
                )
                .unwrap(),
        );
        assert!(!down.key(30));
        assert!(down.was_released(30));
        assert_eq!(down.motion, Some([2, -1]));
        assert!(reporter.release().is_none());
    }
    #[test]
    fn disconnect_releases_only_current_keys_and_buttons() {
        let mut reporter = Reporter::default();
        reporter.record(
            Input::Key {
                code: 42,
                down: true,
            },
            Instant::now(),
        );
        reporter.record(
            Input::Button {
                code: 272,
                down: true,
            },
            Instant::now(),
        );
        let released = reporter.release().unwrap();
        assert_eq!(released.keys, [0; 4]);
        assert_eq!(released.buttons, 0);
        assert!(released.was_released(42));
        assert!(reporter.release().is_none());
    }
}
