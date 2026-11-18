//! Phone peripherals inside the existing Desk Switch process and protocols.
use crate::{config, workspace::net::Candidate};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, mpsc},
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{mpsc as queue, oneshot, watch},
    time::timeout,
};

mod control;
mod routing;
pub(crate) mod screen;
pub use routing::{Destination, Device, Routing, WorkspaceAccess};
pub(crate) mod hub;
mod relay;

pub const CONTROL_PORT: u16 = 45884;
pub const INPUT_PORT: u16 = 45883;
pub(crate) const LEGACY_CONTROL_PORT: u16 = 45882;
fn enabled() -> bool {
    true
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Trusted {
    pub name: String,
    #[serde(default = "enabled")]
    pub shared: bool,
}
pub struct Pending {
    pub key: String,
    pub name: String,
    pub code: String,
    pub reply: oneshot::Sender<bool>,
}
pub enum Event {
    Pair(Pending),
    Status(String),
    Trusted(BTreeMap<String, Trusted>),
    Devices(BTreeMap<String, Device>),
    WorkspaceApproval {
        workspace: String,
        key: String,
        name: String,
        reapprove: bool,
    },
}
pub struct Text {
    pub value: String,
    pub reply: oneshot::Sender<Result<(), String>>,
}
pub struct Service {
    pub events: mpsc::Receiver<Event>,
    stop: watch::Sender<bool>,
    state: watch::Sender<hub::Plan>,
    dir: PathBuf,
    completed: mpsc::Receiver<()>,
    thread: Option<std::thread::JoinHandle<()>>,
    trust_lock: Arc<std::sync::Mutex<()>>,
}
#[derive(Clone, Default)]
pub(crate) enum Backend {
    #[default]
    Native,
    #[cfg(test)]
    Proof(Arc<std::sync::Mutex<Proof>>),
}
#[cfg(test)]
#[derive(Default)]
pub(crate) struct Proof {
    pub inputs: Vec<crate::wire::Input>,
    pub texts: Vec<String>,
    pub releases: usize,
}
fn saved(dir: &Path) -> Result<BTreeMap<String, Trusted>> {
    match std::fs::read(dir.join("mobile-peers.json")) {
        Ok(bytes) => {
            ensure!(bytes.len() <= 16384, "Phone trust file is too large");
            let peers: BTreeMap<String, Trusted> = serde_json::from_slice(&bytes)?;
            ensure!(peers.len() <= 16, "Too many trusted phone remotes");
            for (key, peer) in &peers {
                config::key(key)?;
                ensure!(
                    !peer.name.is_empty()
                        && peer.name.len() <= 256
                        && !peer.name.chars().any(char::is_control),
                    "Invalid phone name"
                );
            }
            Ok(peers)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(error.into()),
    }
}
impl Service {
    pub fn start(dir: PathBuf, name: String, ctx: eframe::egui::Context) -> Result<Self> {
        Self::start_at(
            dir,
            name,
            Some(ctx),
            CONTROL_PORT,
            INPUT_PORT,
            Backend::Native,
        )
        .map(|(service, _, _)| service)
    }
    fn start_at(
        dir: PathBuf,
        name: String,
        ctx: Option<eframe::egui::Context>,
        control_port: u16,
        input_port: u16,
        backend: Backend,
    ) -> Result<(Self, u16, u16)> {
        std::fs::create_dir_all(&dir)?;
        if !dir.join("identity").exists() {
            config::create_identity(&dir)?;
        }
        let identity = config::load_identity(&dir)?;
        let peers = saved(&dir)?;
        #[cfg(test)]
        let isolated = matches!(&backend, Backend::Proof(_));
        #[cfg(not(test))]
        let isolated = false;
        let bind = |port| -> Result<std::net::TcpListener> {
            if isolated {
                Ok(std::net::TcpListener::bind(std::net::SocketAddr::from((
                    [127, 0, 0, 1],
                    port,
                )))?)
            } else {
                Ok(crate::listen::wildcard_tcp(port, 32)?)
            }
        };
        let control = bind(control_port)?;
        let input = bind(input_port)?;
        let control_port = control.local_addr()?.port();
        let input_port = input.local_addr()?.port();
        control.set_nonblocking(true)?;
        input.set_nonblocking(true)?;
        let (send, events) = mpsc::channel();
        let (done, completed) = mpsc::channel();
        let trust_lock = Arc::new(std::sync::Mutex::new(()));
        let lock = trust_lock.clone();
        let (stop, stopping) = watch::channel(false);
        let plan = hub::Plan {
            sharing: peers
                .iter()
                .map(|(key, trusted)| (key.clone(), trusted.shared))
                .collect(),
            ..Default::default()
        };
        let (state, changing) = watch::channel(plan);
        let notify = Notify { events: send, ctx };
        notify.send(Event::Trusted(peers));
        let profile = dir.clone();
        let publisher = state.clone();
        let thread = std::thread::Builder::new()
            .name("Desk Switch phone remotes".into())
            .spawn(move || {
                let result = crate::runtime().and_then(|runtime| {
                    runtime.block_on(tokio::task::LocalSet::new().run_until(async {
                        let control = TcpListener::from_std(control)?;
                        let input = TcpListener::from_std(input)?;
                        let hub_identity = config::Identity {
                            private: identity.private,
                            public: identity.public,
                        };
                        let preview = match &backend {
                            Backend::Native => screen::Provider::Native,
                            #[cfg(test)]
                            Backend::Proof(_) => screen::Provider::Synthetic,
                        };
                        let (hub, hub_task) = hub::start(
                            profile.clone(),
                            hub_identity,
                            name.clone(),
                            backend,
                            notify.clone(),
                            changing.clone(),
                            stopping.clone(),
                        );
                        let server = Rc::new(Server {
                            dir: profile,
                            identity,
                            name,
                            notify: notify.clone(),
                            trust_lock: lock,
                            state: publisher,
                            hub,
                            preview,
                            pending: RefCell::new(BTreeMap::new()),
                            sessions: RefCell::new(BTreeMap::new()),
                            sequence: std::cell::Cell::new(1),
                        });
                        let result = serve(control, input, server, stopping, changing).await;
                        let _ = timeout(Duration::from_secs(3), hub_task).await;
                        result
                    }))
                });
                if let Err(error) = result {
                    notify.send(Event::Status(format!("Phone remote stopped: {error:#}")));
                }
                let _ = done.send(());
            })?;
        Ok((
            Self {
                events,
                stop,
                state,
                dir,
                completed,
                thread: Some(thread),
                trust_lock,
            },
            control_port,
            input_port,
        ))
    }
    pub fn set_routing(&self, routing: Routing) {
        self.state.send_if_modified(|state| {
            if state.routing == routing {
                return false;
            }
            if state.routing.generation != routing.generation
                || state.routing.destination != routing.destination
            {
                state.epoch = state.epoch.wrapping_add(1);
            }
            state.routing = routing;
            true
        });
    }
    pub fn set_shared(&self, key: &str, shared: bool) -> Result<()> {
        let _lock = self.trust_lock.lock().unwrap();
        let mut peers = saved(&self.dir)?;
        peers.get_mut(key).context("Unknown phone remote")?.shared = shared;
        config::write_atomic(
            &self.dir.join("mobile-peers.json"),
            &serde_json::to_string(&peers)?,
        )?;
        self.state.send_modify(|state| {
            state.sharing.insert(key.into(), shared);
            state.settings = state.settings.wrapping_add(1);
        });
        Ok(())
    }
    pub fn forget(&self, key: &str) -> Result<()> {
        let _lock = self.trust_lock.lock().unwrap();
        let mut peers = saved(&self.dir)?;
        peers.remove(key);
        config::write_atomic(
            &self.dir.join("mobile-peers.json"),
            &serde_json::to_string(&peers)?,
        )?;
        self.state.send_modify(|state| {
            state.sharing.remove(key);
            state.settings = state.settings.wrapping_add(1);
        });
        Ok(())
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.stop.send_replace(true);
        if self.completed.recv_timeout(Duration::from_secs(4)).is_ok()
            && let Some(thread) = self.thread.take()
        {
            let _ = thread.join();
        }
    }
}
#[derive(Clone)]
struct Notify {
    events: mpsc::Sender<Event>,
    ctx: Option<eframe::egui::Context>,
}
impl Notify {
    fn send(&self, event: Event) {
        let _ = self.events.send(event);
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint_of(eframe::egui::ViewportId::ROOT);
        }
    }
}
async fn stopped(stop: &mut watch::Receiver<bool>) {
    while !*stop.borrow_and_update() {
        if stop.changed().await.is_err() {
            break;
        }
    }
}
struct Waiting {
    session: u64,
    ip: std::net::IpAddr,
    send: oneshot::Sender<crate::transport::Pipe>,
}
struct Session {
    id: u64,
    relay: bool,
    admitted: bool,
    workspace: Option<String>,
    stop: watch::Sender<bool>,
}
struct Server {
    dir: PathBuf,
    identity: config::Identity,
    name: String,
    notify: Notify,
    trust_lock: Arc<std::sync::Mutex<()>>,
    state: watch::Sender<hub::Plan>,
    hub: hub::Handle,
    preview: screen::Provider,
    pending: RefCell<BTreeMap<config::Key, Waiting>>,
    sessions: RefCell<BTreeMap<String, Session>>,
    sequence: std::cell::Cell<u64>,
}
async fn serve(
    control: TcpListener,
    input: TcpListener,
    server: Rc<Server>,
    mut stop: watch::Receiver<bool>,
    mut changing: watch::Receiver<hub::Plan>,
) -> Result<()> {
    let mut tasks = tokio::task::JoinSet::new();
    let mut peers = saved(&server.dir)?;
    let mut settings = changing.borrow().settings;
    server
        .notify
        .send(Event::Status("Ready for phone remotes".into()));
    loop {
        tokio::select! {
            _ = stopped(&mut stop) => break,
            _ = changing.changed() => {
                let (current_settings, workspace) = {
                    let plan = changing.borrow();
                    (plan.settings, plan.routing.workspace.clone())
                };
                let settings_changed = settings != current_settings;
                if settings_changed {
                    settings = current_settings;
                    peers = saved(&server.dir)?;
                }
                let pruned = if let Some(workspace) = &workspace {
                    remove_local_duplicates(&server, workspace, &mut peers)?
                } else {
                    false
                };
                if settings_changed || pruned {
                    server.notify.send(Event::Trusted(peers.clone()));
                }
                let plan = changing.borrow();
                for (key, session) in server.sessions.borrow().iter() {
                    let allowed = if session.relay {
                        plan.routing.members.contains(key)
                    } else if !session.admitted {
                        true
                    } else if let Some(workspace) = &session.workspace {
                        plan.routing.workspace.as_ref().is_some_and(|current| &current.id == workspace && current.grants(key))
                    } else {
                        peers.contains_key(key)
                            && !workspace.as_ref().is_some_and(|scope| scope.grants.contains_key(key))
                    };
                    if !allowed { session.stop.send_replace(true); }
                }
            },
            stream = control.accept() => {
                let (stream, _) = stream?;
                if tasks.len() >= 64 { drop(stream); continue; }
                let server = server.clone();
                tasks.spawn_local(async move {
                    let authenticated = timeout(Duration::from_secs(5), crate::workspace::net::authenticate(stream, &server.identity, &server.name, false, None, control_port(&server))).await;
                    if let Ok(Ok(peer)) = authenticated { let _ = session(server, peer).await; }
                });
            },
            stream = input.accept() => {
                let (stream, address) = stream?;
                let pending: BTreeMap<_,_> = server.pending.borrow().iter().filter(|(_, waiting)| waiting.ip == address.ip().to_canonical()).map(|(key,waiting)| (*key,waiting.session)).take(64).collect();
                let keys: Vec<_> = pending.keys().copied().collect();
                if keys.is_empty() || tasks.len() >= 64 { drop(stream); continue; }
                let server = server.clone();
                tasks.spawn_local(async move {
                    if let Ok((key, pipe)) = crate::transport::Pipe::accept_any(stream, &server.identity, &keys).await
                        && server.pending.borrow().get(&key).is_some_and(|waiting| Some(&waiting.session) == pending.get(&key))
                        && let Some(waiting) = server.pending.borrow_mut().remove(&key) { let _ = waiting.send.send(pipe); }
                });
            },
            _ = tasks.join_next(), if !tasks.is_empty() => {},
        }
    }
    for session in server.sessions.borrow().values() {
        session.stop.send_replace(true);
    }
    let drain = async { while tasks.join_next().await.is_some() {} };
    let _ = timeout(Duration::from_secs(3), drain).await;
    tasks.abort_all();
    Ok(())
}

fn remove_local_duplicates(
    server: &Server,
    workspace: &WorkspaceAccess,
    peers: &mut BTreeMap<String, Trusted>,
) -> Result<bool> {
    if !peers.keys().any(|key| workspace.grants.contains_key(key)) {
        return Ok(false);
    }
    let _lock = server.trust_lock.lock().unwrap();
    let mut current = saved(&server.dir)?;
    current.retain(|key, _| !workspace.grants.contains_key(key));
    config::write_atomic(
        &server.dir.join("mobile-peers.json"),
        &serde_json::to_string(&current)?,
    )?;
    server.state.send_if_modified(|plan| {
        let len = plan.sharing.len();
        plan.sharing
            .retain(|key, _| !workspace.grants.contains_key(key));
        plan.sharing.len() != len
    });
    *peers = current;
    Ok(true)
}

fn control_port(_: &Server) -> u16 {
    CONTROL_PORT
}
async fn session(server: Rc<Server>, peer: Candidate) -> Result<()> {
    let peer_key = peer.key;
    let key = config::hex(&peer_key);
    let id = server.sequence.get();
    server.sequence.set(id.wrapping_add(1));
    let relay = server.state.borrow().routing.members.contains(&key);
    let (stop, mut stopping) = watch::channel(false);
    if let Some(old) = server.sessions.borrow_mut().insert(
        key.clone(),
        Session {
            id,
            relay,
            admitted: relay,
            workspace: None,
            stop,
        },
    ) {
        old.stop.send_replace(true);
    }
    let result = {
        let remote = remote(&server, peer, id, relay, stopping.clone());
        tokio::pin!(remote);
        tokio::select! {
            result = &mut remote => result,
            _ = stopped(&mut stopping) => timeout(Duration::from_secs(3), &mut remote).await.unwrap_or_else(|_| Ok(())),
        }
    };
    if server
        .sessions
        .borrow()
        .get(&key)
        .is_some_and(|session| session.id == id)
    {
        server.sessions.borrow_mut().remove(&key);
    }
    if server
        .pending
        .borrow()
        .get(&peer_key)
        .is_some_and(|waiting| waiting.session == id)
    {
        server.pending.borrow_mut().remove(&peer_key);
    }
    server.notify.send(Event::Status(if result.is_ok() {
        "Ready for phone remotes".into()
    } else {
        "Phone disconnected. Reconnect from the phone to continue.".into()
    }));
    result
}
async fn remote(
    server: &Server,
    mut peer: Candidate,
    id: u64,
    relay: bool,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    crate::compatibility::Version::current().require(Some(&peer.version))?;
    ensure!(
        peer.workspace_protocol == Some(crate::group::model::PROTOCOL),
        "Incompatible workspace protocol"
    );
    let key = config::hex(&peer.key);
    let peers = saved(&server.dir)?;
    let local = peers.get(&key);
    let workspace = (!relay)
        .then(|| server.state.borrow().routing.workspace.clone())
        .flatten()
        .filter(|scope| scope.grants.contains_key(&key) || local.is_none_or(|phone| phone.shared));
    let was_trusted = if let Some(scope) = &workspace {
        scope.grants(&key)
            || scope.can_enroll
                && scope.may_migrate(&key)
                && local.is_some_and(|phone| phone.shared)
    } else {
        local.is_some()
    };
    let error = if relay || was_trusted {
        None
    } else if workspace.as_ref().is_some_and(|scope| !scope.can_enroll) {
        Some("Update Desk Switch on each workspace computer before adding a phone.")
    } else if workspace
        .as_ref()
        .map_or(peers.len(), |scope| scope.grants.values().flatten().count())
        >= crate::group::model::MAX_PHONE_REMOTES
    {
        Some("Forget a phone before adding another remote.")
    } else {
        None
    };
    let confirm = !relay && (!was_trusted || peer.phone_known == Some(false));
    if peer.phone_known.is_some() {
        let mut status = json!({"PhonePairing":{"confirm":confirm}});
        if let Some(error) = error {
            status["PhonePairing"]["error"] = json!(error);
        }
        peer.send_json(&status).await?;
    }
    ensure!(error.is_none(), "{}", error.unwrap_or_default());
    let decision = if confirm {
        let (reply, decision) = oneshot::channel();
        server.notify.send(Event::Pair(Pending {
            key: key.clone(),
            name: peer.name.clone(),
            code: peer.code.clone(),
            reply,
        }));
        Some(decision)
    } else {
        None
    };
    let approved = timeout(Duration::from_secs(90), async {
        if let Some(mut decision) = decision {
            {
                let received = peer.receive_json();
                tokio::pin!(received);
                tokio::select! {
                    approved = &mut decision => {
                        if !approved? { return Ok::<_, anyhow::Error>(false); }
                        ensure!(received.await? == json!({"Decision":true}), "Phone did not confirm pairing");
                        return Ok(true);
                    },
                    value = &mut received => {
                        ensure!(value? == json!({"Decision":true}), "Phone did not confirm pairing");
                    },
                }
            }
            tokio::select! {
                approved = decision => Ok(approved?),
                closed = peer.receive_json() => { let _ = closed?; anyhow::bail!("Unexpected phone record before consent") },
            }
        } else { ensure!(peer.receive_json().await? == json!({"Decision":true}), "Phone did not confirm pairing"); Ok(true) }
    }).await??;
    let mut response = json!({"Decision":approved});
    if approved && let Some(scope) = &workspace {
        response["workspace"] = json!({"id": scope.id, "members": scope.peers});
    }
    peer.send_json(&response).await?;
    ensure!(approved, "Pairing declined");
    ensure!(
        timeout(Duration::from_secs(5), peer.receive_json()).await?? == json!("Pinned"),
        "Phone did not finish pairing"
    );
    let shared = if relay {
        ensure!(
            server.state.borrow().routing.members.contains(&key),
            "The relay computer left the workspace"
        );
        false
    } else if let Some(scope) = &workspace {
        let mut plan = server.state.subscribe();
        if !scope.grants(&key) {
            server.notify.send(Event::WorkspaceApproval {
                reapprove: !scope.may_migrate(&key),
                workspace: scope.id.clone(),
                key: key.clone(),
                name: peer.name.clone(),
            });
        }
        timeout(Duration::from_secs(12), async {
            loop {
                let approved = {
                    let state = plan.borrow();
                    let current = state
                        .routing
                        .workspace
                        .as_ref()
                        .filter(|current| current.id == scope.id)
                        .context("The workspace is unavailable")?;
                    current.grants(&key)
                };
                if approved {
                    return Ok::<_, anyhow::Error>(());
                }
                plan.changed().await?;
            }
        })
        .await
        .context("Waiting for workspace phone approval")??;
        true
    } else {
        let current = {
            let _lock = server.trust_lock.lock().unwrap();
            ensure!(
                !server
                    .state
                    .borrow()
                    .routing
                    .workspace
                    .as_ref()
                    .is_some_and(|scope| scope.grants.contains_key(&key)),
                "Workspace phone approval changed; connect again"
            );
            let mut current = saved(&server.dir)?;
            ensure!(
                !was_trusted || current.contains_key(&key),
                "Phone trust was revoked"
            );
            ensure!(
                current.contains_key(&key) || current.len() < 16,
                "Too many trusted phones"
            );
            let shared = current.get(&key).is_none_or(|peer| peer.shared);
            current.insert(
                key.clone(),
                Trusted {
                    name: peer.name.clone(),
                    shared,
                },
            );
            config::write_atomic(
                &server.dir.join("mobile-peers.json"),
                &serde_json::to_string(&current)?,
            )?;
            current
        };
        let shared = current[&key].shared;
        server.state.send_modify(|state| {
            state.sharing.insert(key.clone(), shared);
            state.settings = state.settings.wrapping_add(1);
        });
        server.notify.send(Event::Trusted(current));
        shared
    };
    let workspace_id = workspace.map(|scope| scope.id);
    {
        let mut sessions = server.sessions.borrow_mut();
        let session = sessions
            .get_mut(&key)
            .filter(|session| session.id == id)
            .context("Phone session was replaced")?;
        session.admitted = true;
        session.workspace = workspace_id.clone();
    }
    let (send, native) = oneshot::channel();
    server.pending.borrow_mut().insert(
        peer.key,
        Waiting {
            session: id,
            ip: peer.stream.peer_addr()?.ip().to_canonical(),
            send,
        },
    );
    peer.send_json(&json!("Pinned")).await?;
    let native = timeout(Duration::from_secs(10), native).await??;
    let source = server
        .hub
        .register(
            key.clone(),
            peer.name.clone(),
            relay,
            shared,
            workspace_id.clone(),
        )
        .await?;
    let (texts, receiver) = queue::channel(1);
    let (finished, finishing) = watch::channel(false);
    let control = control::run(
        peer,
        key,
        config::hex(&server.identity.public),
        texts,
        server.state.subscribe(),
        (
            relay,
            shared,
            workspace_id,
            source.remote_preview(),
            server.preview,
        ),
        finishing.clone(),
    );
    let receiving = crate::platform::mobile::run(native, receiver, finishing, source);
    tokio::pin!(control, receiving);
    let (result, native_finished) = tokio::select! { result = &mut control => (result, false), result = &mut receiving => (result,true), _ = stopped(&mut stop) => (Ok(()), false) };
    finished.send_replace(true);
    if !native_finished {
        let _ = timeout(Duration::from_secs(2), &mut receiving).await;
    }
    result
}

#[cfg(test)]
mod tests;
