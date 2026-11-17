//! One input owner merges phones and follows the authenticated workspace route.
use super::{
    Backend, Destination, Device, Event, Notify, Routing, Text, relay, saved, screen, stopped,
};
use crate::{config, feedback::Reporter, wire::Input};
use anyhow::{Result, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, watch};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct Plan {
    pub epoch: u64,
    pub routing: Routing,
    pub settings: u64,
    pub sharing: BTreeMap<String, bool>,
    pub allow_screen: bool,
}

enum Command {
    Register {
        source: u64,
        key: String,
        name: String,
        local: bool,
        active: watch::Receiver<bool>,
        preview: watch::Receiver<screen::Options>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Input {
        source: u64,
        epoch: u64,
        local: bool,
        event: Input,
    },
    Text {
        source: u64,
        epoch: u64,
        local: bool,
        text: Text,
    },
    Release(u64),
    Released {
        source: u64,
        reply: oneshot::Sender<()>,
    },
    Unregister(u64),
}

#[derive(Clone)]
pub(super) struct Handle {
    commands: mpsc::Sender<Command>,
    releases: mpsc::UnboundedSender<Command>,
    plan: watch::Receiver<Plan>,
    local_ready: Arc<AtomicU64>,
    remote_ready: Arc<AtomicU64>,
    next: Arc<AtomicU64>,
    frames: watch::Receiver<Option<(u64, screen::Frame)>>,
}
pub(crate) struct Source {
    handle: Handle,
    pub source: u64,
    local: bool,
    shared: bool,
    key: String,
    stop: watch::Sender<bool>,
    preview: watch::Sender<screen::Options>,
}
impl Handle {
    pub async fn register(
        &self,
        key: String,
        name: String,
        local: bool,
        shared: bool,
    ) -> Result<Source> {
        let source = self.next.fetch_add(1, Ordering::Relaxed);
        let (stop, active) = watch::channel(false);
        let (preview, preview_options) = watch::channel(screen::Options::default());
        let source = Source {
            handle: self.clone(),
            source,
            local,
            shared,
            key: key.clone(),
            stop,
            preview,
        };
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Register {
                source: source.source,
                key,
                name,
                local,
                active,
                preview: preview_options,
                reply,
            })
            .await?;
        result.await?.map_err(anyhow::Error::msg)?;
        Ok(source)
    }
}
impl Source {
    pub(super) fn remote_preview(
        &self,
    ) -> (
        watch::Sender<screen::Options>,
        watch::Receiver<Option<(u64, screen::Frame)>>,
    ) {
        (self.preview.clone(), self.handle.frames.clone())
    }
    fn route(&self) -> Option<(u64, bool)> {
        let plan = self.handle.plan.borrow();
        let local = self.local
            || !plan.sharing.get(&self.key).copied().unwrap_or(self.shared)
            || matches!(plan.routing.destination, Destination::Local);
        if self.local && matches!(plan.routing.destination, Destination::Paused { .. }) {
            return None;
        }
        let ready = if local {
            self.handle.local_ready.load(Ordering::Acquire) == plan.epoch.wrapping_add(1)
        } else {
            matches!(plan.routing.destination, Destination::Remote { .. })
                && self.handle.remote_ready.load(Ordering::Acquire) == plan.epoch.wrapping_add(1)
        };
        ready.then_some((plan.epoch, local))
    }
    pub fn input(&self, event: Input) -> Result<()> {
        ensure!(
            !self.handle.commands.is_closed(),
            "Phone input service stopped"
        );
        let Some((epoch, local)) = self.route() else {
            return Ok(());
        };
        let command = Command::Input {
            source: self.source,
            epoch,
            local,
            event,
        };
        if let Err(error) = self.handle.commands.try_send(command) {
            // Motion is disposable. Never block a native heartbeat behind a
            // large motion burst; losing a transition closes this source.
            if matches!(event, Input::Move { .. } | Input::Scroll { .. }) {
                return Ok(());
            }
            return Err(error.into());
        }
        Ok(())
    }
    pub async fn text(&self, text: Text) -> Result<()> {
        let Some((epoch, local)) = self.route() else {
            let _ = text
                .reply
                .send(Err("Input is paused while the destination changes".into()));
            return Ok(());
        };
        self.handle
            .commands
            .send(Command::Text {
                source: self.source,
                epoch,
                local,
                text,
            })
            .await?;
        Ok(())
    }
    pub fn release(&self) -> Result<()> {
        self.handle
            .releases
            .send(Command::Release(self.source))
            .map_err(Into::into)
    }
    pub async fn release_wait(&self) -> Result<()> {
        self.stop.send_replace(true);
        let (reply, result) = oneshot::channel();
        self.handle.releases.send(Command::Released {
            source: self.source,
            reply,
        })?;
        result.await?;
        Ok(())
    }
}
impl Drop for Source {
    fn drop(&mut self) {
        self.stop.send_replace(true);
        let _ = self.handle.releases.send(Command::Unregister(self.source));
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Held {
    Key(u16),
    Button(u16),
}
impl Held {
    fn up(self) -> Input {
        match self {
            Self::Key(code) => Input::Key { code, down: false },
            Self::Button(code) => Input::Button { code, down: false },
        }
    }
}
#[derive(Default)]
struct Holds(BTreeMap<u64, BTreeSet<Held>>);
impl Holds {
    fn event(&mut self, source: u64, event: Input) -> Option<Input> {
        let (held, down) = match event {
            Input::Key { code, down } => (Held::Key(code), down),
            Input::Button { code, down } => (Held::Button(code), down),
            _ => return Some(event),
        };
        let previously = self.0.values().any(|keys| keys.contains(&held));
        let keys = self.0.entry(source).or_default();
        let changed = if down {
            keys.insert(held)
        } else {
            keys.remove(&held)
        };
        let currently = self.0.values().any(|keys| keys.contains(&held));
        (changed && previously != currently).then_some(event)
    }
    fn release(&mut self, source: u64) -> Vec<Input> {
        self.0
            .remove(&source)
            .into_iter()
            .flatten()
            .filter(|held| !self.0.values().any(|keys| keys.contains(held)))
            .map(Held::up)
            .collect()
    }
    fn clear(&mut self) {
        self.0.clear();
    }
}
struct Entry {
    key: String,
    local: bool,
    shared: bool,
    active: watch::Receiver<bool>,
    preview: watch::Receiver<screen::Options>,
}
async fn route_changed(mut plan: watch::Receiver<Plan>, epoch: u64, key: String, shared: bool) {
    loop {
        {
            let current = plan.borrow();
            if current.epoch != epoch
                || current.sharing.get(&key).copied().unwrap_or(true) != shared
            {
                return;
            }
        }
        if plan.changed().await.is_err() {
            return;
        }
    }
}
struct Engine {
    dir: PathBuf,
    identity: config::Identity,
    name: String,
    backend: Backend,
    notify: Notify,
    plan: watch::Receiver<Plan>,
    applied: Plan,
    sources: BTreeMap<u64, Entry>,
    local: Option<crate::platform::mobile::Native>,
    remote: Option<relay::Client>,
    frames: watch::Sender<Option<(u64, screen::Frame)>>,
    local_holds: Holds,
    remote_holds: Holds,
    visual_holds: Holds,
    feedback: Reporter,
    local_ready: Arc<AtomicU64>,
    remote_ready: Arc<AtomicU64>,
    retry: tokio::time::Instant,
    status: String,
}

pub(super) fn start(
    dir: PathBuf,
    identity: config::Identity,
    name: String,
    backend: Backend,
    notify: Notify,
    plan: watch::Receiver<Plan>,
    stop: watch::Receiver<bool>,
) -> (Handle, tokio::task::JoinHandle<()>) {
    let (send, commands) = mpsc::channel(128);
    let (releases, releasing) = mpsc::unbounded_channel();
    let local_ready = Arc::new(AtomicU64::new(0));
    let remote_ready = Arc::new(AtomicU64::new(0));
    let (send_frame, frames) = watch::channel(None);
    let handle = Handle {
        commands: send,
        releases,
        plan: plan.clone(),
        local_ready: local_ready.clone(),
        remote_ready: remote_ready.clone(),
        next: Arc::new(AtomicU64::new(1)),
        frames,
    };
    let applied = plan.borrow().clone();
    let mut engine = Engine {
        dir,
        identity,
        name,
        backend,
        notify,
        plan,
        applied,
        sources: BTreeMap::new(),
        local: None,
        remote: None,
        frames: send_frame,
        local_holds: Holds::default(),
        remote_holds: Holds::default(),
        visual_holds: Holds::default(),
        feedback: Reporter::default(),
        local_ready,
        remote_ready,
        retry: tokio::time::Instant::now(),
        status: String::new(),
    };
    let task = tokio::task::spawn_local(async move {
        if engine.run(commands, releasing, stop).await.is_err() {
            engine.notify.send(Event::Status(
                "Phone input stopped. Reconnect from the phone to try again.".into(),
            ));
        }
        engine.close().await;
    });
    (handle, task)
}
impl Engine {
    fn status(&mut self, status: &str) {
        if self.status != status {
            self.status = status.into();
            self.notify.send(Event::Status(self.status.clone()));
        }
    }
    fn local_source(&self, source: u64) -> bool {
        self.sources.get(&source).is_some_and(|entry| {
            entry.local
                || !entry.shared
                || matches!(self.applied.routing.destination, Destination::Local)
        })
    }
    fn visual(&mut self, event: Input) {
        if let Some(snapshot) = self.feedback.record(event, std::time::Instant::now())
            && let Some(ctx) = &self.notify.ctx
        {
            crate::gui::activity::receive(ctx, u32::MAX, snapshot);
        }
    }
    fn devices(&self) {
        let peers = saved(&self.dir).unwrap_or_default();
        let devices = peers
            .into_iter()
            .map(|(key, trusted)| {
                let connected = self
                    .sources
                    .values()
                    .any(|entry| !entry.local && entry.key == key);
                (
                    key,
                    Device {
                        name: trusted.name,
                        connected,
                        shared: trusted.shared,
                    },
                )
            })
            .collect();
        self.notify.send(Event::Devices(devices));
    }
    async fn paths(&mut self) -> Result<()> {
        let want_local = self.sources.iter().any(|(source, entry)| {
            self.local_source(*source)
                && (!entry.local
                    || !matches!(self.applied.routing.destination, Destination::Paused { .. }))
        });
        if want_local && self.local.is_none() {
            self.local =
                Some(crate::platform::mobile::Native::open(self.backend.clone(), &self.dir).await?);
        }
        self.local_ready.store(
            if self.local.is_some() {
                self.applied.epoch.wrapping_add(1)
            } else {
                0
            },
            Ordering::Release,
        );
        let want_remote = self
            .sources
            .values()
            .any(|entry| !entry.local && entry.shared)
            && matches!(&self.applied.routing.destination, Destination::Remote { key, .. } if self.applied.routing.members.contains(key));
        if !want_remote && self.remote.is_some() {
            self.drop_remote();
        }
        if want_remote && self.remote.is_none() && tokio::time::Instant::now() >= self.retry {
            let mut changed = self.plan.clone();
            let client = tokio::select! {
                result = relay::Client::connect_with_preview(&self.identity, &self.name, &self.applied.routing.destination, self.applied.epoch, self.frames.clone()) => result,
                _ = changed.changed() => return Ok(()),
            };
            match client {
                Ok(client) => {
                    self.remote = Some(client);
                    self.status("Phone remotes follow the selected computer");
                }
                Err(_) => {
                    self.retry = tokio::time::Instant::now() + Duration::from_secs(2);
                    self.status("Phone input is waiting for the selected computer. It must run the experimental phone-enabled build.");
                }
            }
        }
        self.remote_ready.store(
            if self.remote.is_some() {
                self.applied.epoch.wrapping_add(1)
            } else {
                0
            },
            Ordering::Release,
        );
        Ok(())
    }
    async fn release(&mut self, source: u64) {
        for event in self.local_holds.release(source) {
            if let Some(local) = &mut self.local {
                let _ = local.event(event);
            }
        }
        for event in self.remote_holds.release(source) {
            if let Some(remote) = &mut self.remote
                && remote.input(event).await.is_err()
            {
                self.remote = None;
                self.remote_ready.store(0, Ordering::Release);
            }
        }
        for event in self.visual_holds.release(source) {
            self.visual(event);
        }
    }
    async fn reset(&mut self) {
        self.local_ready.store(0, Ordering::Release);
        self.remote_ready.store(0, Ordering::Release);
        let sources: Vec<_> = self.local_holds.0.keys().copied().collect();
        for source in sources {
            for event in self.local_holds.release(source) {
                if let Some(local) = &mut self.local {
                    let _ = local.event(event);
                }
            }
        }
        if let Some(mut local) = self.local.take() {
            local.close().await;
        }
        self.remote = None;
        self.frames.send_replace(None);
        self.local_holds.clear();
        self.remote_holds.clear();
        self.visual_holds.clear();
        self.feedback.release();
        if let Some(ctx) = &self.notify.ctx {
            crate::gui::activity::disconnected(ctx, u32::MAX);
        }
        self.retry = tokio::time::Instant::now();
    }
    fn drop_remote(&mut self) {
        self.remote = None;
        self.frames.send_replace(None);
        self.remote_ready.store(0, Ordering::Release);
        let sources: Vec<_> = self.remote_holds.0.keys().copied().collect();
        self.remote_holds.clear();
        for source in sources {
            for event in self.visual_holds.release(source) {
                self.visual(event);
            }
        }
    }
    async fn reconcile(&mut self) -> Result<()> {
        let current = self.plan.borrow_and_update().clone();
        if current != self.applied {
            let route_changed = current.epoch != self.applied.epoch;
            if route_changed {
                self.reset().await;
            }
            self.applied = current;
            let peers = saved(&self.dir)?;
            let removed: Vec<_> = self
                .sources
                .iter()
                .filter(|(_, entry)| !entry.local && !peers.contains_key(&entry.key))
                .map(|(source, _)| *source)
                .collect();
            for source in removed {
                self.release(source).await;
                self.sources.remove(&source);
            }
            let changed: Vec<_> = self
                .sources
                .iter()
                .filter(|(_, entry)| !entry.local)
                .filter_map(|(source, entry)| {
                    peers
                        .get(&entry.key)
                        .filter(|peer| peer.shared != entry.shared)
                        .map(|peer| (*source, peer.shared))
                })
                .collect();
            for (source, shared) in changed {
                self.release(source).await;
                if let Some(entry) = self.sources.get_mut(&source) {
                    entry.shared = shared;
                }
            }
            self.devices();
        }
        self.paths().await
    }
    async fn run(
        &mut self,
        mut commands: mpsc::Receiver<Command>,
        mut releases: mpsc::UnboundedReceiver<Command>,
        mut stop: watch::Receiver<bool>,
    ) -> Result<()> {
        let mut heartbeat = tokio::time::interval(crate::transport::PING_PERIOD);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = stopped(&mut stop) => return Ok(()),
                result = self.reconcile() => result?,
            }
            enum Wake {
                Command(Command),
                Route,
                Stop,
                Tick,
                Remote(Result<()>),
                Local(Result<()>),
            }
            let wake = tokio::select! {
                biased;
                _ = stopped(&mut stop) => Wake::Stop,
                command = releases.recv() => command.map_or(Wake::Stop, Wake::Command),
                command = commands.recv() => command.map_or(Wake::Stop, Wake::Command),
                _ = self.plan.changed() => Wake::Route,
                _ = heartbeat.tick() => Wake::Tick,
                result = async { self.remote.as_mut().unwrap().poll().await }, if self.remote.is_some() => Wake::Remote(result),
                result = async { self.local.as_mut().unwrap().poll().await }, if self.local.is_some() => Wake::Local(result),
            };
            match wake {
                Wake::Stop => return Ok(()),
                Wake::Route => {}
                Wake::Tick => {
                    let preview = if self.applied.allow_screen {
                        self.sources
                            .values()
                            .filter(|entry| !entry.local && entry.shared)
                            .map(|entry| *entry.preview.borrow())
                            .filter(|options| options.enabled)
                            .max_by_key(|options| options.fps)
                            .unwrap_or_default()
                    } else {
                        screen::Options::default()
                    };
                    if let Some(remote) = &mut self.remote {
                        let mut changed = self.plan.clone();
                        let result = tokio::select! {
                            _ = changed.changed() => None,
                            _ = stopped(&mut stop) => return Ok(()),
                            result = async { remote.heartbeat().await?; remote.set_preview(preview).await } => Some(result),
                        };
                        if result.is_none_or(|result| result.is_err()) {
                            self.drop_remote();
                        }
                    }
                }
                Wake::Remote(Err(_)) => {
                    self.drop_remote();
                    self.retry = tokio::time::Instant::now() + Duration::from_secs(1);
                }
                Wake::Local(Err(_)) => {
                    anyhow::bail!("Native phone input permission was lost");
                }
                Wake::Remote(Ok(())) | Wake::Local(Ok(())) => {}
                Wake::Command(Command::Register {
                    source,
                    key,
                    name,
                    local,
                    active,
                    preview,
                    reply,
                }) => {
                    ensure!(
                        self.sources.len() < 64,
                        "Too many active remote input sources"
                    );
                    let shared = saved(&self.dir)?
                        .get(&key)
                        .is_none_or(|trusted| trusted.shared);
                    self.sources.insert(
                        source,
                        Entry {
                            key,
                            local,
                            shared,
                            active,
                            preview,
                        },
                    );
                    let result = tokio::select! {
                        biased;
                        _ = stopped(&mut stop) => return Ok(()),
                        result = self.paths() => result.map_err(|error| format!("Could not prepare phone input: {error:#}")),
                    };
                    if result.is_err() {
                        self.sources.remove(&source);
                    }
                    let _ = reply.send(result);
                    if !local {
                        self.notify
                            .send(Event::Status(format!("{name} is connected")));
                    }
                    self.devices();
                }
                Wake::Command(Command::Release(source)) => self.release(source).await,
                Wake::Command(Command::Released { source, reply }) => {
                    self.release(source).await;
                    let _ = reply.send(());
                }
                Wake::Command(Command::Unregister(source)) => {
                    self.release(source).await;
                    self.sources.remove(&source);
                    self.devices();
                    if self.sources.is_empty() {
                        self.close().await;
                        self.notify
                            .send(Event::Status("Ready for phone remotes".into()));
                    }
                }
                Wake::Command(Command::Input {
                    source,
                    epoch,
                    local,
                    event,
                }) => {
                    if epoch != self.applied.epoch
                        || local != self.local_source(source)
                        || !self
                            .sources
                            .get(&source)
                            .is_some_and(|entry| !*entry.active.borrow())
                    {
                        continue;
                    }
                    let accepted = if self.local_source(source) {
                        if let Some(local) = &mut self.local {
                            if let Some(event) = self.local_holds.event(source, event) {
                                local.event(event)?;
                            }
                            true
                        } else {
                            false
                        }
                    } else if let Some(remote) = &mut self.remote {
                        if let Some(event) = self.remote_holds.event(source, event)
                            && remote.input(event).await.is_err()
                        {
                            self.drop_remote();
                            continue;
                        }
                        true
                    } else {
                        false
                    };
                    if accepted && let Some(event) = self.visual_holds.event(source, event) {
                        self.visual(event);
                    }
                }
                Wake::Command(Command::Text {
                    source,
                    epoch,
                    local,
                    text,
                }) => {
                    if epoch != self.applied.epoch
                        || local != self.local_source(source)
                        || !self
                            .sources
                            .get(&source)
                            .is_some_and(|entry| !*entry.active.borrow())
                    {
                        let _ = text
                            .reply
                            .send(Err("The destination changed; text was discarded".into()));
                        continue;
                    }
                    let changed = route_changed(
                        self.plan.clone(),
                        self.applied.epoch,
                        self.sources[&source].key.clone(),
                        self.sources[&source].shared,
                    );
                    tokio::pin!(changed);
                    let mut source_stop = self.sources[&source].active.clone();
                    let mut shutdown = stop.clone();
                    let local_source = self.local_source(source);
                    let result = if local_source {
                        if let Some(local) = &mut self.local {
                            tokio::select! { biased; _ = &mut changed => Err(anyhow::anyhow!("The destination changed while typing; text was not retried")), _ = stopped(&mut source_stop) => Err(anyhow::anyhow!("The phone disconnected while typing")), _ = stopped(&mut shutdown) => Err(anyhow::anyhow!("Desk Switch is stopping")), result = local.text(text.value) => result }
                        } else {
                            Err(anyhow::anyhow!("Local input is not ready"))
                        }
                    } else if let Some(remote) = &mut self.remote {
                        tokio::select! { result = remote.text(text.value) => result, _ = &mut changed => Err(anyhow::anyhow!("The destination changed while typing; text was not retried")), _ = stopped(&mut source_stop) => Err(anyhow::anyhow!("The phone disconnected while typing")), _ = stopped(&mut shutdown) => Err(anyhow::anyhow!("Desk Switch is stopping")) }
                    } else {
                        Err(anyhow::anyhow!("The selected computer is not ready"))
                    };
                    if result.is_err() && !local_source {
                        self.drop_remote();
                    }
                    let _ = text.reply.send(result.map_err(|error| error.to_string()));
                }
            }
        }
    }
    async fn close(&mut self) {
        self.reset().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_phone_disconnect_never_releases_another_phones_modifier() {
        let mut holds = Holds::default();
        let down = Input::Key {
            code: 42,
            down: true,
        };
        assert_eq!(holds.event(1, down), Some(down));
        assert_eq!(holds.event(2, down), None);
        assert!(holds.release(1).is_empty());
        assert_eq!(
            holds.release(2),
            vec![Input::Key {
                code: 42,
                down: false
            }]
        );
    }
    #[test]
    fn route_reset_discards_holds_instead_of_replaying_them_to_the_next_screen() {
        let mut holds = Holds::default();
        holds.event(
            1,
            Input::Button {
                code: 272,
                down: true,
            },
        );
        holds.clear();
        assert_eq!(
            holds.event(
                1,
                Input::Button {
                    code: 272,
                    down: false
                }
            ),
            None
        );
        assert_eq!(
            holds.event(1, Input::Move { dx: 5, dy: 2 }),
            Some(Input::Move { dx: 5, dy: 2 })
        );
    }
}
