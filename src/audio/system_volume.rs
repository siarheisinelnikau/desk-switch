//! The shared output's actual system volume. Opening/closing never changes it.
#[cfg(all(test, not(target_os = "linux")))]
#[path = "system_volume_linux.rs"]
mod linux_backend_tests;
#[cfg(target_os = "linux")]
#[path = "system_volume_linux.rs"]
mod native;
#[cfg(windows)]
#[path = "system_volume_windows.rs"]
mod native;

use crate::{lifecycle, workspace::AudioLevel};
use anyhow::{Context, Result, ensure};
use std::{sync::mpsc, thread, time::Duration};
use tokio::sync::oneshot;

#[derive(Clone, Copy, Default)]
pub(super) struct Change {
    pub volume: Option<u8>,
    pub muted: Option<bool>,
}

trait Backend {
    fn read(&mut self) -> Result<AudioLevel>;
    fn set_volume(&mut self, volume: u8) -> Result<()>;
    fn set_muted(&mut self, muted: bool) -> Result<()>;
}

struct Controller<B> {
    backend: B,
    reported: Option<(u64, AudioLevel)>,
    request: u64,
}

impl<B: Backend> Controller<B> {
    fn new(backend: B) -> Self {
        Self {
            backend,
            reported: None,
            request: 0,
        }
    }

    fn apply(&mut self, request: u64, change: Change) -> Result<()> {
        ensure!(
            change.volume.is_none_or(|volume| volume <= 100),
            "system volume must be between 0 and 100"
        );
        let current = self.backend.read()?;
        if let Some(volume) = change.volume
            && volume != current.volume
        {
            self.backend.set_volume(volume)?;
        }
        if let Some(muted) = change.muted
            && muted != current.muted
        {
            self.backend.set_muted(muted)?;
        }
        self.request = request;
        Ok(())
    }

    fn observe(&mut self) -> Result<Option<(u64, AudioLevel)>> {
        let level = self.backend.read()?;
        ensure!(level.volume <= 100, "invalid system volume");
        let current = (self.request, level);
        if self.reported == Some(current) {
            return Ok(None);
        }
        self.reported = Some(current);
        Ok(Some(current))
    }
}

enum Command {
    Set(u64, Change),
    Stop,
}

pub(super) struct SystemVolume {
    commands: mpsc::Sender<Command>,
    pub done: oneshot::Receiver<Result<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl SystemVolume {
    pub fn start(output: Option<String>, initial: Change) -> Result<Self> {
        let (commands, receive) = mpsc::channel();
        let (done, result) = oneshot::channel();
        let thread = thread::Builder::new()
            .name("Desk Switch system volume".into())
            .spawn(move || {
                let result = (|| {
                    let mut controller = Controller::new(native::Native::open(output.as_deref())?);
                    controller.apply(0, initial)?;
                    loop {
                        if let Some((request, level)) = controller.observe()? {
                            lifecycle::report(lifecycle::Event::OutputLevel { request, level });
                        }
                        match receive.recv_timeout(Duration::from_millis(500)) {
                            Ok(Command::Set(request, change)) => {
                                controller.apply(request, change)?
                            }
                            Ok(Command::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                        }
                    }
                    Ok(())
                })();
                let _ = done.send(result);
            })?;
        Ok(Self {
            commands,
            done: result,
            thread: Some(thread),
        })
    }

    pub fn set(&self, request: u64, change: Change) -> Result<()> {
        self.commands
            .send(Command::Set(request, change))
            .context("system volume controller stopped")
    }
}

impl Drop for SystemVolume {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        level: AudioLevel,
        writes: usize,
    }
    impl Backend for Fake {
        fn read(&mut self) -> Result<AudioLevel> {
            Ok(self.level)
        }
        fn set_volume(&mut self, volume: u8) -> Result<()> {
            self.level.volume = volume;
            self.writes += 1;
            Ok(())
        }
        fn set_muted(&mut self, muted: bool) -> Result<()> {
            self.level.muted = muted;
            self.writes += 1;
            Ok(())
        }
    }

    #[test]
    fn startup_adopts_system_volume_and_external_changes_without_writing() {
        let mut controller = Controller::new(Fake {
            level: AudioLevel {
                volume: 37,
                muted: true,
            },
            writes: 0,
        });
        controller.apply(0, Change::default()).unwrap();
        assert_eq!(
            controller.observe().unwrap(),
            Some((
                0,
                AudioLevel {
                    volume: 37,
                    muted: true
                }
            ))
        );
        assert!(controller.observe().unwrap().is_none());
        controller.backend.level = AudioLevel {
            volume: 90,
            muted: false,
        };
        assert_eq!(
            controller.observe().unwrap(),
            Some((0, controller.backend.level))
        );
        assert_eq!(controller.backend.writes, 0);
    }

    #[test]
    fn absolute_volume_is_idempotent_and_mute_does_not_replace_the_level() {
        let mut controller = Controller::new(Fake {
            level: AudioLevel {
                volume: 60,
                muted: false,
            },
            writes: 0,
        });
        let change = Change {
            volume: Some(90),
            muted: Some(true),
        };
        controller.apply(1, change).unwrap();
        controller.apply(2, change).unwrap();
        assert_eq!(controller.backend.writes, 2);
        assert_eq!(
            controller.observe().unwrap(),
            Some((
                2,
                AudioLevel {
                    volume: 90,
                    muted: true
                }
            ))
        );
        controller
            .apply(
                3,
                Change {
                    volume: None,
                    muted: Some(false),
                },
            )
            .unwrap();
        assert_eq!(
            controller.backend.level,
            AudioLevel {
                volume: 90,
                muted: false
            }
        );
        controller
            .apply(
                4,
                Change {
                    volume: Some(0),
                    muted: None,
                },
            )
            .unwrap();
        assert_eq!(
            controller.backend.level,
            AudioLevel {
                volume: 0,
                muted: false
            }
        );
        assert!(
            controller
                .apply(
                    5,
                    Change {
                        volume: Some(101),
                        muted: None
                    }
                )
                .is_err()
        );
        assert_eq!(controller.backend.writes, 4);
    }
}
