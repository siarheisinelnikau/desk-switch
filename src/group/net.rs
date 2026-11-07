//! Concurrent authenticated workspace links. The controller serializes settings;
//! input and media retain their own low-latency, authenticated data connections.
use super::model::{self, Member, MemberPreferences, Settings, Workspace};
use crate::{
    config,
    workspace::{
        self, Node,
        net::{Candidate, Request},
    },
};
use anyhow::{Context, Result, bail, ensure};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc as queue, oneshot},
    time::timeout,
};

mod clipboard_bytes {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let value = String::deserialize(deserializer)?;
        if value.len() > 12 * 1024 * 1024 {
            return Err(serde::de::Error::custom("Clipboard data is too large"));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(value)
            .map_err(serde::de::Error::custom)?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(serde::de::Error::custom("Clipboard data is too large"));
        }
        Ok(bytes)
    }
}

const MAX_PACKET: usize = 16 * 1024 * 1024;
const CHUNK: usize = 60 * 1024;
const AUTH: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Action {
    Settings {
        expected: u64,
        value: Settings,
    },
    Member {
        key: String,
        expected: MemberPreferences,
        value: MemberPreferences,
    },
    Activate {
        key: String,
    },
    ActivateEdge {
        key: String,
        position: u16,
    },
    Invite {
        address: SocketAddr,
        key: String,
    },
    Manage {
        target: String,
        message: workspace::management::Message,
    },
    Update {
        target: String,
        message: workspace::update::Message,
    },
    Clipboard {
        mime: String,
        #[serde(with = "clipboard_bytes")]
        bytes: Vec<u8>,
        digest: String,
    },
}

#[derive(Debug)]
pub enum Event {
    Request(Request),
    State(Box<Workspace>),
    Management {
        source: String,
        message: workspace::management::Message,
    },
    Update {
        source: String,
        message: workspace::update::Message,
    },
    UpdateSent(workspace::update::Message),
    Clipboard {
        source: String,
        mime: String,
        bytes: Vec<u8>,
        digest: String,
    },
    Error(String),
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
enum Intent {
    Invite,
    Join,
}

#[derive(Deserialize, Serialize)]
enum Packet {
    Hello {
        protocol: u16,
        intent: Intent,
        member: Box<Member>,
        hub: String,
        count: usize,
    },
    Decision(bool),
    State(Box<Workspace>),
    Node(Box<Node>, Option<workspace::AudioLevel>, u64),
    Action(Action),
    Management {
        source: String,
        target: String,
        message: workspace::management::Message,
    },
    Update {
        source: String,
        target: String,
        message: workspace::update::Message,
    },
    UpdateAck {
        source: String,
        target: String,
        message: workspace::update::Message,
    },
    Clipboard {
        source: String,
        mime: String,
        #[serde(with = "clipboard_bytes")]
        bytes: Vec<u8>,
        digest: String,
    },
    Ping,
    Error(String),
}

enum Command {
    Action(Action),
    Node(Box<Node>, Option<workspace::AudioLevel>, u64),
    Approve(String, bool),
    Disconnect,
}
enum Incoming {
    Consent {
        request: Request,
        accept: oneshot::Sender<bool>,
    },
    Ready {
        candidate: Candidate,
        member: Box<Member>,
        controller: bool,
    },
    Packet(String, Packet),
    Closed(String, Option<[u8; 32]>, String),
}

pub struct Service {
    commands: queue::Sender<Command>,
    events: mpsc::Receiver<Event>,
    snapshot: Arc<Mutex<Option<Box<Workspace>>>>,
    clipboard: Arc<Mutex<Option<Event>>>,
    pub port: u16,
}
impl Service {
    pub fn start(dir: PathBuf, mut local: Member, port: u16, ctx: egui::Context) -> Result<Self> {
        let listener = std::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], port)))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        local.address.set_port(port);
        let mut state = load(&dir, local)?;
        for member in state.members.values_mut() {
            member.online = member.node.key == config::hex(&config::load_identity(&dir)?.public);
        }
        state.validate()?;
        let (send, commands) = queue::channel(128);
        let (events_send, events) = mpsc::channel();
        let snapshot = Arc::new(Mutex::new(None));
        let clipboard = Arc::new(Mutex::new(None));
        let notify = Notify {
            events: events_send,
            snapshot: snapshot.clone(),
            clipboard: clipboard.clone(),
            ctx,
        };
        std::thread::Builder::new()
            .name("Desk Switch workspace links".into())
            .spawn(move || {
                if let Err(error) = crate::runtime().and_then(|runtime| {
                    runtime.block_on(run(listener, dir, state, commands, notify.clone()))
                }) {
                    notify.event(Event::Error(format!("{error:#}")));
                }
            })?;
        Ok(Self {
            commands: send,
            events,
            snapshot,
            clipboard,
            port,
        })
    }
    pub fn action(&self, action: Action) -> Result<()> {
        self.commands
            .try_send(Command::Action(action))
            .context("Workspace command queue is busy")?;
        Ok(())
    }
    pub fn publish(&self, node: Node, native: Option<workspace::AudioLevel>, ready: u64) {
        let _ = self
            .commands
            .try_send(Command::Node(Box::new(node), native, ready));
    }
    pub fn approve(&self, key: String, accept: bool) {
        let _ = self.commands.try_send(Command::Approve(key, accept));
    }
    pub fn poll(&self) -> Vec<Event> {
        let mut events = self.events.try_iter().take(128).collect::<Vec<_>>();
        if let Some(state) = self.snapshot.lock().unwrap().take() {
            events.push(Event::State(state));
        }
        if let Some(clipboard) = self.clipboard.lock().unwrap().take() {
            events.push(clipboard);
        }
        events
    }
    pub fn stop(&self) {
        let _ = self.commands.try_send(Command::Disconnect);
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone)]
struct Notify {
    events: mpsc::Sender<Event>,
    snapshot: Arc<Mutex<Option<Box<Workspace>>>>,
    clipboard: Arc<Mutex<Option<Event>>>,
    ctx: egui::Context,
}
impl Notify {
    fn event(&self, event: Event) {
        if matches!(event, Event::Clipboard { .. }) {
            *self.clipboard.lock().unwrap() = Some(event);
        } else {
            let _ = self.events.send(event);
        }
        self.ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
    fn state(&self, state: &Workspace) {
        *self.snapshot.lock().unwrap() = Some(Box::new(state.clone()));
        self.ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
}

fn load(dir: &Path, local: Member) -> Result<Workspace> {
    let path = dir.join("workspace.json");
    let mut state: Workspace = if path.exists() {
        serde_json::from_slice(&std::fs::read(path)?)?
    } else {
        Workspace {
            hub: local.node.key.clone(),
            revision: 0,
            settings: Settings::from(&local.node.shared),
            members: BTreeMap::new(),
            active_input: None,
            active_input_entry: None,
            activation: 0,
            active_monitor: None,
        }
    };
    let mut local = local;
    if state.members.is_empty() && dir.join("peer").exists() {
        if let Ok(peer) = config::load_peer(dir) {
            let key = config::hex(&peer);
            let document = std::fs::read_to_string(dir.join("desktop.toml"))
                .ok()
                .and_then(|text| toml::from_str::<toml::Value>(&text).ok());
            let previous = document.as_ref().and_then(|value| value.get("workspace"));
            let address = previous
                .and_then(|value| value.get("last_address"))
                .and_then(toml::Value::as_str)
                .and_then(|text| text.parse::<SocketAddr>().ok());
            if let Some(mut address) = address {
                address.set_port(model::PORT);
                let mut node = local.node.clone();
                node.key = key.clone();
                node.name = previous
                    .and_then(|value| value.get("peer_name"))
                    .and_then(toml::Value::as_str)
                    .filter(|name| !name.is_empty())
                    .unwrap_or("Linked computer")
                    .into();
                node.hardware = Default::default();
                node.devices = Default::default();
                node.version = None;
                node.management = None;
                node.monitor = Default::default();
                node.shared = Default::default();
                node.ack.clear();
                state.members.insert(
                    key.clone(),
                    Member {
                        node,
                        address,
                        online: false,
                        native_output: None,
                        preferences: Default::default(),
                        input_ready: 0,
                    },
                );
                state.hub = state.hub.min(key);
            }
        }
    }
    if let Some(saved) = state.members.get(&local.node.key) {
        local.preferences = saved.preferences.clone();
    }
    state.members.insert(local.node.key.clone(), local);
    state.validate()?;
    Ok(state)
}
fn save(dir: &Path, state: &Workspace) -> Result<()> {
    config::write_atomic(
        &dir.join("workspace.json"),
        &serde_json::to_string_pretty(state)?,
    )
}

async fn write_packet(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    noise: &Arc<Mutex<snow::TransportState>>,
    packet: &Packet,
) -> Result<()> {
    let bytes = serde_json::to_vec(packet)?;
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_PACKET,
        "Workspace message is too large"
    );
    writer.write_u32(bytes.len() as u32).await?;
    for plain in bytes.chunks(CHUNK) {
        let mut encrypted = vec![0; plain.len() + 16];
        let count = noise.lock().unwrap().write_message(plain, &mut encrypted)?;
        writer.write_u16(count as u16).await?;
        writer.write_all(&encrypted[..count]).await?;
    }
    Ok(())
}
async fn read_packet(
    reader: &mut (impl tokio::io::AsyncRead + Unpin),
    noise: &Arc<Mutex<snow::TransportState>>,
) -> Result<Packet> {
    let size = reader.read_u32().await? as usize;
    ensure!(
        (1..=MAX_PACKET).contains(&size),
        "Invalid workspace message length"
    );
    let mut bytes = Vec::with_capacity(size.min(CHUNK));
    while bytes.len() < size {
        let count = reader.read_u16().await? as usize;
        ensure!(
            (17..=CHUNK + 16).contains(&count),
            "Invalid encrypted workspace block"
        );
        let mut encrypted = vec![0; count];
        reader.read_exact(&mut encrypted).await?;
        let mut plain = vec![0; count];
        let count = noise.lock().unwrap().read_message(&encrypted, &mut plain)?;
        ensure!(bytes.len() + count <= size, "Workspace block overflow");
        bytes.extend_from_slice(&plain[..count]);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

async fn setup(
    mut candidate: Candidate,
    local: Member,
    state: Workspace,
    intent: Intent,
    internal: queue::Sender<Incoming>,
) -> Result<()> {
    let peer = config::hex(&candidate.key);
    let trusted = state.members.contains_key(&peer);
    let outgoing = candidate.outgoing;
    let noise = Arc::new(Mutex::new(candidate.noise));
    write_packet(
        &mut candidate.stream,
        &noise,
        &Packet::Hello {
            protocol: model::PROTOCOL,
            intent,
            member: Box::new(local.clone()),
            hub: state.hub.clone(),
            count: state.members.len(),
        },
    )
    .await?;
    let Packet::Hello {
        protocol,
        intent: remote_intent,
        mut member,
        hub: remote_hub,
        count,
    } = timeout(AUTH, read_packet(&mut candidate.stream, &noise)).await??
    else {
        bail!("Missing workspace handshake")
    };
    ensure!(
        protocol == model::PROTOCOL && member.node.key == peer,
        "Incompatible workspace handshake"
    );
    member.address = candidate.address;
    member.online = true;
    member.validate(&remote_hub)?;
    let requested = if outgoing { intent } else { remote_intent };
    let controller = matches!(
        (outgoing, requested),
        (true, Intent::Invite) | (false, Intent::Join)
    );
    if controller {
        ensure!(
            state.hub == local.node.key,
            "Add computers through the workspace controller"
        );
    } else {
        ensure!(remote_hub == peer, "Connect to the workspace controller");
        ensure!(
            state.members.len() <= 1 || state.hub == peer,
            "Leave the existing workspace before joining another"
        );
    }
    if controller && matches!(requested, Intent::Invite) {
        ensure!(
            count <= 1 || remote_hub == local.node.key,
            "The invited computer already belongs to another workspace"
        );
    }
    let accepted = if trusted || outgoing {
        true
    } else {
        let (accept, approved) = oneshot::channel();
        internal
            .send(Incoming::Consent {
                request: Request {
                    key: peer.clone(),
                    name: candidate.name.clone(),
                    code: candidate.code.clone(),
                },
                accept,
            })
            .await?;
        timeout(Duration::from_secs(90), approved).await??
    };
    write_packet(&mut candidate.stream, &noise, &Packet::Decision(accepted)).await?;
    ensure!(
        accepted
            && matches!(
                timeout(
                    Duration::from_secs(90),
                    read_packet(&mut candidate.stream, &noise)
                )
                .await??,
                Packet::Decision(true)
            ),
        "Workspace invitation declined"
    );
    candidate.noise = Arc::try_unwrap(noise)
        .map_err(|_| anyhow::anyhow!("Handshake still in use"))?
        .into_inner()
        .unwrap();
    internal
        .send(Incoming::Ready {
            candidate,
            member,
            controller,
        })
        .await?;
    Ok(())
}

fn dial(
    identity: &config::Identity,
    local: &Member,
    state: &Workspace,
    address: SocketAddr,
    expected: String,
    intent: Intent,
    internal: queue::Sender<Incoming>,
) {
    let identity = config::Identity {
        private: identity.private,
        public: identity.public,
    };
    let local = local.clone();
    let state = state.clone();
    tokio::spawn(async move {
        let result = async {
            let stream = timeout(AUTH, TcpStream::connect(address)).await??;
            let candidate = timeout(
                AUTH,
                workspace::net::authenticate(
                    stream,
                    &identity,
                    &local.node.name,
                    true,
                    (!expected.is_empty()).then_some(expected.as_str()),
                    local.address.port(),
                ),
            )
            .await??;
            setup(candidate, local, state, intent, internal.clone()).await
        }
        .await;
        if let Err(error) = result {
            let _ = internal
                .send(Incoming::Closed(expected, None, format!("{error:#}")))
                .await;
        }
    });
}

fn start_session(candidate: Candidate, internal: queue::Sender<Incoming>) -> Session {
    let generation = candidate.connection;
    let (send, mut outgoing) = queue::channel(128);
    let (clipboard, mut clips) = tokio::sync::watch::channel::<Option<Arc<Packet>>>(None);
    tokio::spawn(async move {
        let key = config::hex(&candidate.key);
        let noise = Arc::new(Mutex::new(candidate.noise));
        let (mut read, mut write) = candidate.stream.into_split();
        let reader_noise = noise.clone();
        let reader_internal = internal.clone();
        let reader_key = key.clone();
        let mut reader = tokio::spawn(async move {
            loop {
                let packet = timeout(
                    Duration::from_secs(45),
                    read_packet(&mut read, &reader_noise),
                )
                .await??;
                reader_internal
                    .send(Incoming::Packet(reader_key.clone(), packet))
                    .await?;
            }
            #[allow(unreachable_code)]
            Ok::<(), anyhow::Error>(())
        });
        let result = loop {
            tokio::select! {
                result = &mut reader => break result.map_err(anyhow::Error::from).and_then(|result|result),
                packet = outgoing.recv() => {
                    let Some(packet) = packet else { break Ok(()); };
                    if let Err(error) = timeout(Duration::from_secs(30), write_packet(&mut write, &noise, &packet)).await.map_err(anyhow::Error::from).and_then(|result|result) { break Err(error); }
                }
                changed=clips.changed()=>{
                    if changed.is_err(){break Ok(());}
                    let latest=clips.borrow_and_update().clone();
                    if let Some(latest)=latest && let Err(error)=timeout(Duration::from_secs(30),write_packet(&mut write,&noise,&latest)).await.map_err(anyhow::Error::from).and_then(|result|result){break Err(error);}
                }
                _ = tokio::time::sleep(Duration::from_secs(2)) => {
                    if let Err(error) = write_packet(&mut write, &noise, &Packet::Ping).await { break Err(error); }
                }
            }
        };
        reader.abort();
        let _ = internal
            .send(Incoming::Closed(
                key,
                Some(generation),
                result
                    .err()
                    .map(|error| format!("{error:#}"))
                    .unwrap_or_default(),
            ))
            .await;
    });
    (generation, send, clipboard)
}

type Session = (
    [u8; 32],
    queue::Sender<Packet>,
    tokio::sync::watch::Sender<Option<Arc<Packet>>>,
);
type Sessions = BTreeMap<String, Session>;
fn send(sessions: &Sessions, key: &str, packet: Packet) -> Result<()> {
    sessions
        .get(key)
        .context("The computer is offline")?
        .1
        .try_send(packet)
        .context("The computer's command queue is busy")?;
    Ok(())
}
fn broadcast(sessions: &Sessions, state: &Workspace) {
    for (_, connection, _) in sessions.values() {
        let _ = connection.try_send(Packet::State(Box::new(state.clone())));
    }
}

#[allow(clippy::too_many_arguments)]
fn apply(
    action: Action,
    source: &str,
    own: &str,
    state: &mut Workspace,
    sessions: &Sessions,
    notify: &Notify,
    identity: &config::Identity,
    internal: &queue::Sender<Incoming>,
) -> Result<()> {
    ensure!(
        state.members.contains_key(source),
        "Unknown workspace member"
    );
    match action {
        Action::Settings { expected, value } => {
            state.set_settings(expected, value)?;
            state.bind_microphone();
        }
        Action::Member {
            key,
            expected,
            value,
        } => state.set_member(&key, &expected, value)?,
        Action::Activate { key } => {
            state.active_input_entry = None;
            ensure!(
                state.members.get(&key).is_some_and(Member::compatible),
                "The destination is offline or needs an update"
            );
            let monitor =
                state.settings.monitor_enabled && state.members[&key].node.monitor.enabled;
            if monitor {
                state.monitor_input(&key)?;
            }
            state.active_input = Some(key.clone());
            if monitor {
                state.active_monitor = Some(key);
            }
            state.revision = state
                .revision
                .checked_add(1)
                .context("Workspace revision exhausted")?;
            state.activation = state
                .activation
                .checked_add(1)
                .context("Too many input switches")?;
        }
        Action::ActivateEdge { key, position } => {
            apply(
                Action::Activate { key },
                source,
                own,
                state,
                sessions,
                notify,
                identity,
                internal,
            )?;
            state.active_input_entry = Some(position);
        }
        Action::Invite { address, key } => {
            if !key.is_empty() {
                model::valid_key(&key)?;
            }
            dial(
                identity,
                &state.members[own],
                state,
                address,
                key,
                Intent::Invite,
                internal.clone(),
            );
        }
        Action::Manage { target, message } => {
            message.validate()?;
            if target == own {
                notify.event(Event::Management {
                    source: source.into(),
                    message,
                });
            } else {
                send(
                    sessions,
                    &target,
                    Packet::Management {
                        source: source.into(),
                        target: target.clone(),
                        message,
                    },
                )?;
            }
        }
        Action::Update { target, message } => {
            message.validate()?;
            if target == own {
                if source != own {
                    send(
                        sessions,
                        source,
                        Packet::UpdateAck {
                            source: own.into(),
                            target: source.into(),
                            message: message.clone(),
                        },
                    )?;
                }
                notify.event(Event::Update {
                    source: source.into(),
                    message,
                });
            } else {
                send(
                    sessions,
                    &target,
                    Packet::Update {
                        source: source.into(),
                        target: target.clone(),
                        message,
                    },
                )?;
            }
        }
        Action::Clipboard {
            mime,
            bytes,
            digest,
        } => {
            ensure!(state.settings.clipboard_enabled, "Shared clipboard is off");
            ensure!(
                ["text/plain", "image/png"].contains(&mime.as_str())
                    && bytes.len() <= 8 * 1024 * 1024,
                "Unsupported clipboard data"
            );
            use sha2::{Digest, Sha256};
            ensure!(
                format!("{:x}", Sha256::digest(&bytes)) == digest,
                "Clipboard checksum mismatch"
            );
            notify.event(Event::Clipboard {
                source: source.into(),
                mime: mime.clone(),
                bytes: bytes.clone(),
                digest: digest.clone(),
            });
            let packet = Arc::new(Packet::Clipboard {
                source: source.into(),
                mime,
                bytes,
                digest,
            });
            for (_, _, clipboard) in sessions.values() {
                clipboard.send_replace(Some(packet.clone()));
            }
        }
    }
    Ok(())
}

async fn run(
    listener: std::net::TcpListener,
    dir: PathBuf,
    mut state: Workspace,
    mut commands: queue::Receiver<Command>,
    notify: Notify,
) -> Result<()> {
    let listener = TcpListener::from_std(listener)?;
    let identity = config::load_identity(&dir)?;
    let own = config::hex(&identity.public);
    let (internal, mut incoming) = queue::channel(256);
    let permits = Arc::new(tokio::sync::Semaphore::new(8));
    let mut sessions = BTreeMap::new();
    let mut approvals: BTreeMap<String, oneshot::Sender<bool>> = BTreeMap::new();
    let mut reconnect = tokio::time::interval(Duration::from_secs(3));
    let mut connected_once = false;
    reconnect.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    notify.state(&state);
    loop {
        let before = state.clone();
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                if let Ok(permit) = permits.clone().try_acquire_owned() {
                    let local = state.members[&own].clone(); let current = state.clone(); let internal = internal.clone();
                    let identity = config::Identity { private: identity.private, public: identity.public };
                    tokio::spawn(async move {
                        let _permit = permit;
                        let result = async {
                            let candidate = timeout(AUTH, workspace::net::authenticate(stream, &identity, &local.node.name, false, None, local.address.port())).await??;
                            setup(candidate, local, current, Intent::Invite, internal.clone()).await
                        }.await;
                        if let Err(error) = result { let _ = internal.send(Incoming::Closed(String::new(), None, format!("{error:#}"))).await; }
                    });
                }
            }
            command = commands.recv() => match command {
                Some(Command::Disconnect) | None => break,
                Some(Command::Approve(key, accept)) => { if let Some(approved) = approvals.remove(&key) { let _: Result<(), bool> = approved.send(accept); } }
                Some(Command::Node(node,native,ready)) => {
                    state.update_node(&own, *node.clone())?;
                    state.members.get_mut(&own).unwrap().input_ready=ready;
                    if let Some(native)=native.filter(|level|level.volume<=100) {state.members.get_mut(&own).unwrap().native_output=Some(native);}
                    if state.hub != own { let _ = send(&sessions, &state.hub, Packet::Node(node,native,ready)); }
                }
                Some(Command::Action(action)) => {
                    let result = if state.hub == own { apply(action, &own, &own, &mut state, &sessions, &notify, &identity, &internal) }
                        else { send(&sessions, &state.hub, Packet::Action(action)) };
                    if let Err(error) = result { notify.event(Event::Error(error.to_string())); }
                }
            },
            _ = reconnect.tick() => {
                if !connected_once && !state.members[&own].node.management.as_ref().is_none_or(|management|management.application.auto_connect) {continue;}
                if state.hub != own && !sessions.contains_key(&state.hub) {
                    if let Some(hub) = state.members.get(&state.hub) { dial(&identity, &state.members[&own], &state, hub.address, state.hub.clone(), Intent::Join, internal.clone()); }
                } else if state.hub == own {
                    for (key, member) in &state.members { if key != &own && !sessions.contains_key(key) { dial(&identity, &state.members[&own], &state, member.address, key.clone(), Intent::Invite, internal.clone()); } }
                }
            }
            event = incoming.recv() => match event.context("Workspace event loop stopped")? {
                Incoming::Consent { request, accept } => { if !approvals.contains_key(&request.key) { approvals.insert(request.key.clone(), accept); notify.event(Event::Request(request)); } }
                Incoming::Ready { candidate, member, controller } => {
                    connected_once=true;
                    let key = member.node.key.clone();
                    if sessions.get(&key).is_some_and(|(generation, _, _)| generation <= &candidate.connection) { continue; }
                    let mut local_address = candidate.stream.local_addr()?; local_address.set_port(listener.local_addr()?.port());
                    state.members.get_mut(&own).unwrap().address = local_address;
                    let connection = start_session(candidate, internal.clone());
                    if controller {
                        let mut member = *member;
                        if let Some(saved) = state.members.get(&key) { member.preferences = saved.preferences.clone(); }
                        state.members.insert(key.clone(), member);
                        state.revision += 1;
                        let _ = connection.1.try_send(Packet::State(Box::new(state.clone())));
                    } else { state.hub = key.clone(); state.members.insert(key.clone(), *member); }
                    sessions.insert(key, connection);
                }
                Incoming::Packet(key, packet) => {
                    let result = match packet {
                        Packet::Ping => Ok(()),
                        Packet::State(remote) if key == state.hub && state.hub != own => {
                            remote.validate()?;
                            ensure!(remote.hub == key && remote.members.contains_key(&own), "Invalid controller state");
                            let local = state.members[&own].node.clone();
                            state = *remote;
                            state.members.get_mut(&own).unwrap().node = local;
                            Ok(())
                        }
                        Packet::Node(node,native,ready) if state.hub == own => {
                            state.update_node(&key, *node)?;
                            state.members.get_mut(&key).unwrap().input_ready=ready;
                            if let Some(native)=native.filter(|level|level.volume<=100) {state.members.get_mut(&key).unwrap().native_output=Some(native);}
                            Ok(())
                        },
                        Packet::Action(action) if state.hub == own => apply(action, &key, &own, &mut state, &sessions, &notify, &identity, &internal),
                        Packet::Management { source, target, message } if key == state.hub && target == own => { message.validate()?; notify.event(Event::Management { source, message }); Ok(()) }
                        Packet::Update { source, target, message } if key == state.hub && target == own => {
                            message.validate()?;
                            send(&sessions,&state.hub,Packet::UpdateAck{source:own.clone(),target:source.clone(),message:message.clone()})?;
                            notify.event(Event::Update { source, message }); Ok(())
                        }
                        Packet::UpdateAck {source,target,message} if (state.hub==own && source==key) || (key==state.hub && target==own) => {
                            message.validate()?;
                            if target==own {notify.event(Event::UpdateSent(message));Ok(())}
                            else {send(&sessions,&target.clone(),Packet::UpdateAck{source,target,message})}
                        }
                        Packet::Clipboard { source, mime, bytes, digest } if key == state.hub && state.settings.clipboard_enabled => { notify.event(Event::Clipboard { source, mime, bytes, digest }); Ok(()) }
                        Packet::Error(error) => { notify.event(Event::Error(error)); Ok(()) }
                        _ => Err(anyhow::anyhow!("Unexpected workspace message")),
                    };
                    if let Err(error) = result { let _ = send(&sessions, &key, Packet::Error(error.to_string())); }
                }
                Incoming::Closed(key, generation, error) => {
                    if generation.is_some() && sessions.get(&key).is_some_and(|(active, _, _)| Some(*active) == generation) {
                        sessions.remove(&key);
                        if let Some(member) = state.members.get_mut(&key) { member.online = false; }
                        if key == state.hub { for member in state.members.values_mut() { member.online = member.node.key == own; } }
                    }
                    if !error.is_empty() && !key.is_empty() && !sessions.contains_key(&key) { notify.event(Event::Error(error)); }
                }
            }
        }
        if state != before {
            if state.hub == own {
                state.bind_microphone();
                if state
                    .active_input
                    .as_ref()
                    .is_none_or(|key| !state.members.get(key).is_some_and(Member::compatible))
                {
                    state.active_input = state.plan().input;
                }
                broadcast(&sessions, &state);
            }
            notify.state(&state);
            if state.revision != before.revision || state.hub != before.hub {
                save(&dir, &state)?;
            }
        }
    }
    save(&dir, &state)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    struct Computer {
        service: Service,
        state: Option<Workspace>,
        dir: PathBuf,
        member: Member,
    }
    fn pump(computers: &mut [Computer], forwarded: &mut Vec<(usize, Event)>) {
        for (index, computer) in computers.iter_mut().enumerate() {
            for event in computer.service.poll() {
                match event {
                    Event::Request(request) => computer.service.approve(request.key, true),
                    Event::State(state) => computer.state = Some(*state),
                    other => forwarded.push((index, other)),
                }
            }
        }
    }
    fn until(
        computers: &mut [Computer],
        forwarded: &mut Vec<(usize, Event)>,
        condition: impl Fn(&[Computer], &[(usize, Event)]) -> bool,
    ) {
        let since = Instant::now();
        loop {
            pump(computers, forwarded);
            if condition(computers, forwarded) {
                return;
            }
            assert!(
                since.elapsed() < Duration::from_secs(20),
                "Timed out: states={:?}, events={forwarded:?}",
                computers
                    .iter()
                    .map(|computer| computer.state.as_ref().map(|state| (
                        &state.hub,
                        state.members.len(),
                        state
                            .members
                            .values()
                            .filter(|member| member.online)
                            .count()
                    )))
                    .collect::<Vec<_>>()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn four_authenticated_computers_share_settings_manage_each_other_and_reconnect() {
        let root =
            std::env::temp_dir().join(format!("desk-switch-four-links-{}", std::process::id()));
        let fixture = model::tests::fixture(4);
        let mut computers = Vec::new();
        for (index, member) in fixture.members.values().enumerate() {
            let dir = root.join(index.to_string());
            std::fs::create_dir_all(&dir).unwrap();
            let identity = config::create_identity(&dir).unwrap();
            let mut member = member.clone();
            member.node.key = config::hex(&identity.public);
            let service =
                Service::start(dir.clone(), member.clone(), 0, egui::Context::default()).unwrap();
            computers.push(Computer {
                service,
                state: None,
                dir,
                member,
            });
        }
        let keys = computers
            .iter()
            .map(|computer| computer.member.node.key.clone())
            .collect::<Vec<_>>();
        let mut forwarded = Vec::new();
        for index in 1..4 {
            computers[0]
                .service
                .action(Action::Invite {
                    address: SocketAddr::from(([127, 0, 0, 1], computers[index].service.port)),
                    key: keys[index].clone(),
                })
                .unwrap();
            until(&mut computers, &mut forwarded, |computers, _| {
                computers[0]
                    .state
                    .as_ref()
                    .is_some_and(|state| state.members.len() == index + 1)
            });
        }
        until(&mut computers, &mut forwarded, |computers, _| {
            computers.iter().all(|computer| {
                computer.state.as_ref().is_some_and(|state| {
                    state.members.len() == 4 && state.members.values().all(|member| member.online)
                })
            })
        });
        let revision = computers[3].state.as_ref().unwrap().revision;
        let mut settings = computers[3].state.as_ref().unwrap().settings.clone();
        settings.clipboard_enabled = true;
        computers[3]
            .service
            .action(Action::Settings {
                expected: revision,
                value: settings,
            })
            .unwrap();
        until(&mut computers, &mut forwarded, |computers, _| {
            computers
                .iter()
                .all(|computer| computer.state.as_ref().unwrap().settings.clipboard_enabled)
        });
        let before = computers[1].state.as_ref().unwrap().members[&keys[2]]
            .preferences
            .clone();
        let mut after = before.clone();
        after.audio.volume = 23;
        computers[1]
            .service
            .action(Action::Member {
                key: keys[2].clone(),
                expected: before,
                value: after,
            })
            .unwrap();
        until(&mut computers, &mut forwarded, |computers, _| {
            computers.iter().all(|computer| {
                computer.state.as_ref().unwrap().members[&keys[2]]
                    .preferences
                    .audio
                    .volume
                    == 23
            })
        });
        computers[3]
            .service
            .action(Action::Manage {
                target: keys[2].clone(),
                message: workspace::management::Message::CheckUpdates { id: 42 },
            })
            .unwrap();
        until(&mut computers, &mut forwarded, |_, events| {
            events.iter().any(|(index,event)|*index==2 && matches!(event,Event::Management { source, message: workspace::management::Message::CheckUpdates { id:42 } } if source==&keys[3]))
        });
        computers[2]
            .service
            .action(Action::Manage {
                target: keys[3].clone(),
                message: workspace::management::Message::Reply {
                    id: 42,
                    error: None,
                },
            })
            .unwrap();
        until(&mut computers, &mut forwarded, |_, events| {
            events.iter().any(|(index,event)|*index==3 && matches!(event,Event::Management { source, message: workspace::management::Message::Reply { id:42, error:None } } if source==&keys[2]))
        });
        computers[1]
            .service
            .action(Action::Activate {
                key: keys[3].clone(),
            })
            .unwrap();
        until(&mut computers, &mut forwarded, |computers, _| {
            computers.iter().all(|computer| {
                computer.state.as_ref().unwrap().active_input.as_ref() == Some(&keys[3])
            })
        });
        computers[3].service.stop();
        until(&mut computers, &mut forwarded, |computers, _| {
            computers[0]
                .state
                .as_ref()
                .is_some_and(|state| !state.members[&keys[3]].online)
        });
        std::thread::sleep(Duration::from_millis(50));
        computers[3].service = Service::start(
            computers[3].dir.clone(),
            computers[3].member.clone(),
            0,
            egui::Context::default(),
        )
        .unwrap();
        until(&mut computers, &mut forwarded, |computers, _| {
            computers.iter().all(|computer| {
                computer.state.as_ref().is_some_and(|state| {
                    state.members.len() == 4 && state.members.values().all(|member| member.online)
                })
            })
        });
        assert!(computers.iter().all(|computer| {
            computer.state.as_ref().unwrap().members[&keys[2]]
                .preferences
                .audio
                .volume
                == 23
        }));
        for computer in &computers {
            computer.service.stop();
        }
        drop(computers);
        std::thread::sleep(Duration::from_millis(100));
        std::fs::remove_dir_all(root).unwrap();
    }
}
