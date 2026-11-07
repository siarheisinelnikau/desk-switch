//! A source gain for native applications, excluding Desk Switch's received audio.
//! Only the listening PC runs this controller. Changes are journaled before use
//! and restored on shutdown/reconfiguration; endpoint master volume is untouched.
#[cfg(target_os = "linux")]
#[path = "mixer_linux.rs"]
mod native;
#[cfg(windows)]
#[path = "mixer_windows.rs"]
mod native;

use crate::{config, workspace::AudioLevel};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};
use tokio::sync::oneshot;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct State {
    channels: Vec<f64>,
    muted: bool,
}
impl State {
    fn valid(&self) -> bool {
        !self.channels.is_empty()
            && self.channels.len() <= 32
            && self
                .channels
                .iter()
                .all(|v| v.is_finite() && (0.0..=64.0).contains(v))
    }
    fn same_volume(&self, other: &Self) -> bool {
        self.channels.len() == other.channels.len()
            && self
                .channels
                .iter()
                .zip(&other.channels)
                .all(|(a, b)| (a - b).abs() < 0.0002 * a.abs().max(b.abs()).max(1.0))
    }
    fn same(&self, other: &Self) -> bool {
        self.same_volume(other) && self.muted == other.muted
    }
    fn scaled(&self, level: AudioLevel) -> Self {
        Self {
            // Zero uses mute, retaining the pre-existing channel balance/levels.
            channels: self
                .channels
                .iter()
                .map(|v| {
                    v * if level.volume == 0 {
                        1.0
                    } else {
                        f64::from(level.volume) / 100.0
                    }
                })
                .collect(),
            muted: self.muted || level.muted || level.volume == 0,
        }
    }
}
struct Session {
    id: String,
    group: String,
    state: State,
    ours: bool,
}
trait Backend {
    fn sessions(&mut self) -> Result<Vec<Session>>;
    fn apply(&mut self, id: &str, state: &State) -> Result<()>;
}
#[derive(Clone, Deserialize, PartialEq, Serialize)]
struct Saved {
    group: String,
    base: State,
    applied: State,
}
#[derive(Default)]
struct Controller {
    path: Option<PathBuf>,
    saved: BTreeMap<String, Saved>,
}
impl Controller {
    fn load(dir: &Path) -> Result<Self> {
        let path = dir.join("native-audio-restore.json");
        let saved: BTreeMap<String, Saved> = if path.exists() {
            ensure!(
                path.metadata()?.len() <= 4 * 1024 * 1024,
                "native audio recovery file is too large"
            );
            serde_json::from_slice(&std::fs::read(&path)?)
                .context("reading native audio recovery")?
        } else {
            BTreeMap::new()
        };
        ensure!(
            saved.len() <= 1024
                && saved.iter().all(|(id, s)| id.len() <= 4096
                    && s.group.len() <= 4096
                    && s.base.valid()
                    && s.applied.valid()),
            "invalid native audio recovery state"
        );
        Ok(Self {
            path: Some(path),
            saved,
        })
    }
    fn save(&self) -> Result<()> {
        if let Some(path) = &self.path {
            config::write_atomic(path, &serde_json::to_string(&self.saved)?)?;
        }
        Ok(())
    }
    fn update(&mut self, backend: &mut impl Backend, level: AudioLevel) -> Result<()> {
        if level == AudioLevel::default() {
            return self.restore(backend);
        }
        let sessions = backend.sessions()?;
        let live = sessions
            .iter()
            .map(|s| s.id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        for session in sessions {
            if session.ours {
                continue;
            }
            ensure!(session.state.valid(), "invalid native session volume");
            let old = self.saved.get(&session.id).cloned();
            let inherited = self
                .saved
                .iter()
                .find(|(_, s)| s.group == session.group && session.state.same_volume(&s.applied))
                .map(|(id, s)| (id.clone(), s.clone()));
            let mut base = old
                .as_ref()
                .or(inherited.as_ref().map(|(_, s)| s))
                .map(|s| s.base.clone())
                .unwrap_or_else(|| session.state.clone());
            if let Some(old) = &old {
                // Preserve changes made in the system/app mixer while connected.
                if !session.state.same_volume(&old.applied) {
                    base.channels = session.state.channels.clone();
                }
                if session.state.muted != old.applied.muted {
                    base.muted = session.state.muted;
                }
            } else if let Some((_, old)) = &inherited
                && session.state.muted != old.applied.muted
            {
                base.muted = session.state.muted;
            }
            let desired = base.scaled(level);
            let saved = Saved {
                group: session.group,
                base,
                applied: desired.clone(),
            };
            if old.as_ref() != Some(&saved) {
                if old.is_none()
                    && let Some((id, _)) = &inherited
                    && !live.contains(id)
                {
                    self.saved.remove(id);
                }
                ensure!(
                    self.saved.len() < 1024 || old.is_some(),
                    "too many native audio sessions"
                );
                self.saved.insert(session.id.clone(), saved);
                self.save()?; // The original levels survive an interrupted process.
            }
            if !session.state.same(&desired) {
                backend.apply(&session.id, &desired)?;
            }
        }
        Ok(())
    }
    fn restore(&mut self, backend: &mut impl Backend) -> Result<()> {
        if self.saved.is_empty() {
            return Ok(());
        }
        let mut error = None;
        for session in backend.sessions()? {
            if session.ours {
                continue;
            }
            // Group matching also recovers an app restarted with a saved OS volume.
            let record = self.saved.get_key_value(&session.id).or_else(|| {
                self.saved.iter().find(|(_, s)| {
                    s.group == session.group && session.state.same_volume(&s.applied)
                })
            });
            let Some((id, saved)) = record else { continue };
            let id = id.clone();
            let mut restore = session.state.clone();
            if session.state.same_volume(&saved.applied) {
                restore.channels = saved.base.channels.clone();
            }
            if session.state.muted == saved.applied.muted {
                restore.muted = saved.base.muted;
            }
            if !session.state.same(&restore)
                && let Err(e) = backend.apply(&session.id, &restore)
            {
                error = Some(e);
                continue;
            }
            self.saved.remove(&id);
            self.save()?;
        }
        match error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

enum Command {
    #[cfg(test)]
    Set(AudioLevel),
    Stop,
}
pub struct NativeMixer {
    commands: mpsc::Sender<Command>,
    pub done: oneshot::Receiver<Result<()>>,
    ready: oneshot::Receiver<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl NativeMixer {
    pub fn start(dir: PathBuf, level: AudioLevel) -> Result<Self> {
        Self::start_with(dir, level, native::Native::open)
    }
    fn start_with<B: Backend + 'static>(
        dir: PathBuf,
        level: AudioLevel,
        open: impl FnOnce() -> Result<B> + Send + 'static,
    ) -> Result<Self> {
        let (commands, receive) = mpsc::channel();
        let (done, result) = oneshot::channel();
        let (ready, initialized) = oneshot::channel();
        let thread = std::thread::Builder::new()
            .name("Desk Switch native audio mix".into())
            .spawn(move || {
                #[cfg(test)]
                let mut level = level;
                let run = || -> Result<()> {
                    let mut backend = open()?;
                    let mut controller = Controller::load(&dir)?;
                    controller.restore(&mut backend)?;
                    let result = (|| {
                        let mut ready = Some(ready);
                        loop {
                            controller.update(&mut backend, level)?;
                            if let Some(ready) = ready.take() {
                                let _ = ready.send(());
                            }
                            match receive.recv_timeout(Duration::from_millis(300)) {
                                #[cfg(test)]
                                Ok(Command::Set(value)) => level = value,
                                Ok(Command::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                                    break;
                                }
                                Err(mpsc::RecvTimeoutError::Timeout) => {}
                            }
                        }
                        Ok(())
                    })();
                    let restored = controller.restore(&mut backend);
                    result.and(restored)
                };
                let _ = done.send(run());
            })?;
        Ok(Self {
            commands,
            done: result,
            ready: initialized,
            thread: Some(thread),
        })
    }
    #[cfg(test)]
    fn set(&self, level: AudioLevel) -> Result<()> {
        self.commands
            .send(Command::Set(level))
            .context("native audio mixer stopped")
    }
    pub async fn wait_ready(&mut self) -> Result<()> {
        if (&mut self.ready).await.is_err() {
            (&mut self.done)
                .await
                .context("native audio mixer stopped during startup")??;
            anyhow::bail!("native audio mixer did not initialize");
        }
        Ok(())
    }
    pub async fn stop(&mut self) -> Result<()> {
        let _ = self.commands.send(Command::Stop);
        (&mut self.done)
            .await
            .context("native audio mixer stopped without restoring volumes")?
    }
}
impl Drop for NativeMixer {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn restore_pending(dir: &Path) -> Result<()> {
    if !dir.join("native-audio-restore.json").exists() {
        return Ok(());
    }
    let mut controller = Controller::load(dir)?;
    if controller.saved.is_empty() {
        return Ok(());
    }
    controller.restore(&mut native::Native::open()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Fake {
        states: BTreeMap<String, (String, State, bool)>,
    }
    impl Backend for Fake {
        fn sessions(&mut self) -> Result<Vec<Session>> {
            Ok(self
                .states
                .iter()
                .map(|(id, (group, state, ours))| Session {
                    id: id.clone(),
                    group: group.clone(),
                    state: state.clone(),
                    ours: *ours,
                })
                .collect())
        }
        fn apply(&mut self, id: &str, state: &State) -> Result<()> {
            self.states.get_mut(id).unwrap().1 = state.clone();
            Ok(())
        }
    }
    impl Backend for std::sync::Arc<std::sync::Mutex<Fake>> {
        fn sessions(&mut self) -> Result<Vec<Session>> {
            self.lock().unwrap().sessions()
        }
        fn apply(&mut self, id: &str, state: &State) -> Result<()> {
            self.lock().unwrap().apply(id, state)
        }
    }
    fn state(a: f64, b: f64, muted: bool) -> State {
        State {
            channels: vec![a, b],
            muted,
        }
    }
    #[test]
    fn native_gain_excludes_received_audio_and_restores_app_levels_and_mutes() {
        let mut backend = Fake::default();
        backend.states.insert(
            "browser".into(),
            ("browser".into(), state(0.8, 0.4, false), false),
        );
        backend.states.insert(
            "muted-app".into(),
            ("game".into(), state(0.6, 0.6, true), false),
        );
        backend.states.insert(
            "desk-switch".into(),
            ("desk-switch".into(), state(1.0, 1.0, false), true),
        );
        let mut c = Controller::default();
        c.update(
            &mut backend,
            AudioLevel {
                volume: 50,
                muted: false,
            },
        )
        .unwrap();
        assert_eq!(backend.states["browser"].1, state(0.4, 0.2, false));
        assert_eq!(backend.states["muted-app"].1, state(0.3, 0.3, true));
        assert_eq!(backend.states["desk-switch"].1, state(1.0, 1.0, false));
        c.update(
            &mut backend,
            AudioLevel {
                volume: 0,
                muted: false,
            },
        )
        .unwrap();
        assert_eq!(backend.states["browser"].1, state(0.8, 0.4, true));
        c.restore(&mut backend).unwrap();
        assert_eq!(backend.states["browser"].1, state(0.8, 0.4, false));
        assert_eq!(backend.states["muted-app"].1, state(0.6, 0.6, true));
        assert!(c.saved.is_empty());
    }
    #[test]
    fn app_restart_does_not_multiply_gain_and_external_changes_survive_shutdown() {
        let mut backend = Fake::default();
        backend.states.insert(
            "old".into(),
            ("browser".into(), state(0.8, 0.4, false), false),
        );
        let mut c = Controller::default();
        let level = AudioLevel {
            volume: 50,
            muted: false,
        };
        c.update(&mut backend, level).unwrap();
        let old = backend.states.remove("old").unwrap();
        backend.states.insert("new".into(), old);
        c.update(&mut backend, level).unwrap();
        assert_eq!(backend.states["new"].1, state(0.4, 0.2, false));
        assert_eq!(c.saved.len(), 1);
        backend.states.get_mut("new").unwrap().1 = state(0.7, 0.5, true);
        c.restore(&mut backend).unwrap();
        assert_eq!(backend.states["new"].1, state(0.7, 0.5, true));
    }

    #[test]
    fn recovery_journal_restores_after_an_interrupted_volume_and_mute_change() {
        let dir =
            std::env::temp_dir().join(format!("desk-switch-native-mix-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = Controller::load(&dir).unwrap();
        let mut backend = Fake::default();
        backend.states.insert(
            "browser".into(),
            ("browser".into(), state(0.8, 0.4, false), false),
        );
        c.update(
            &mut backend,
            AudioLevel {
                volume: 50,
                muted: true,
            },
        )
        .unwrap();
        // Simulate a process interruption between the OS volume and mute writes.
        backend.states.get_mut("browser").unwrap().1.muted = false;
        drop(c);
        let mut recovered = Controller::load(&dir).unwrap();
        recovered.restore(&mut backend).unwrap();
        assert_eq!(backend.states["browser"].1, state(0.8, 0.4, false));
        assert!(Controller::load(&dir).unwrap().saved.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn managed_mixer_waits_for_initial_gain_and_restores_before_stop_finishes() {
        use std::sync::{Arc, Mutex};
        let dir =
            std::env::temp_dir().join(format!("desk-switch-native-thread-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let backend = Arc::new(Mutex::new(Fake::default()));
        backend.lock().unwrap().states.insert(
            "browser".into(),
            ("browser".into(), state(0.8, 0.4, false), false),
        );
        let worker_backend = Arc::clone(&backend);
        let mut worker = NativeMixer::start_with(
            dir.clone(),
            AudioLevel {
                volume: 50,
                muted: false,
            },
            move || Ok(worker_backend),
        )
        .unwrap();
        worker.wait_ready().await.unwrap();
        assert_eq!(
            backend.lock().unwrap().states["browser"].1,
            state(0.4, 0.2, false)
        );
        worker
            .set(AudioLevel {
                volume: 0,
                muted: false,
            })
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !backend.lock().unwrap().states["browser"].1.muted {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        worker.stop().await.unwrap();
        assert_eq!(
            backend.lock().unwrap().states["browser"].1,
            state(0.8, 0.4, false)
        );
        assert!(Controller::load(&dir).unwrap().saved.is_empty());
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
