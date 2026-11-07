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
    collections::{BTreeMap, VecDeque},
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
    ReturnInput,
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
    Action(Box<Action>),
    Approve(String, bool),
    Disconnect,
    #[cfg(test)]
    Raw(String, Box<Packet>),
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
    Packet(String, [u8; 32], Packet),
    Closed(String, Option<[u8; 32]>, String),
}

pub struct Service {
    commands: queue::Sender<Command>,
    events: mpsc::Receiver<Event>,
    snapshot: Arc<Mutex<Option<Box<Workspace>>>>,
    clipboard: Arc<Mutex<Option<Event>>>,
    clipboard_commands: Arc<ClipboardCommand>,
    status_commands: Arc<StatusCommand>,
}
type StatusUpdate = (Box<Node>, Option<workspace::AudioLevel>, u64);
#[derive(Default)]
struct StatusCommand {
    latest: Mutex<Option<StatusUpdate>>,
    changed: tokio::sync::Notify,
}
#[derive(Default)]
struct ClipboardCommand {
    latest: Mutex<Option<Action>>,
    changed: tokio::sync::Notify,
}
fn listen(port: u16) -> Result<std::net::TcpListener> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV6,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )
    .and_then(|socket| {
        socket.set_only_v6(false)?;
        socket.set_reuse_address(true)?;
        socket.bind(&SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], port)).into())?;
        Ok(socket)
    })
    .or_else(|_| {
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )?;
        socket.set_reuse_address(true)?;
        socket.bind(&SocketAddr::from(([0, 0, 0, 0], port)).into())?;
        Ok::<_, std::io::Error>(socket)
    })?;
    socket.listen(16)?;
    socket.set_nonblocking(true)?;
    Ok(socket.into())
}
impl Service {
    pub fn start(dir: PathBuf, mut local: Member, port: u16, ctx: egui::Context) -> Result<Self> {
        let listener = listen(port)?;
        let port = listener.local_addr()?.port();
        local.address.set_port(port);
        let mut state = load(&dir, local)?;
        let own = config::hex(&config::load_identity(&dir)?.public);
        for member in state.members.values_mut() {
            member.online = member.node.key == own;
        }
        state.validate()?;
        let (send, commands) = queue::channel(128);
        let (events_send, events) = mpsc::channel();
        let snapshot = Arc::new(Mutex::new(None));
        let clipboard = Arc::new(Mutex::new(None));
        let clipboard_commands = Arc::new(ClipboardCommand::default());
        let clipboard_worker = clipboard_commands.clone();
        let status_commands = Arc::new(StatusCommand::default());
        let status_worker = status_commands.clone();
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
                    runtime.block_on(run(
                        listener,
                        dir,
                        state,
                        commands,
                        status_worker,
                        clipboard_worker,
                        notify.clone(),
                    ))
                }) {
                    notify.event(Event::Error(format!("{error:#}")));
                }
            })?;
        Ok(Self {
            commands: send,
            events,
            snapshot,
            clipboard,
            clipboard_commands,
            status_commands,
        })
    }
    pub fn action(&self, action: Action) -> Result<()> {
        if matches!(&action, Action::Clipboard { .. }) {
            ensure!(!self.commands.is_closed(), "The workspace is disconnected");
            *self.clipboard_commands.latest.lock().unwrap() = Some(action);
            self.clipboard_commands.changed.notify_one();
            return Ok(());
        }
        self.commands
            .try_send(Command::Action(Box::new(action)))
            .context("Workspace command queue is busy")?;
        Ok(())
    }
    pub fn publish(&self, node: Node, native: Option<workspace::AudioLevel>, ready: u64) {
        if self.commands.is_closed() {
            return;
        }
        *self.status_commands.latest.lock().unwrap() = Some((Box::new(node), native, ready));
        self.status_commands.changed.notify_one();
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
    if state.members.is_empty() {
        local.preferences.devices = local.node.shared.devices_for(&local.node.key);
        if let Some(level) = local.node.shared.audio_levels.get(&local.node.key) {
            local.preferences.audio = *level;
        }
    }
    if state.members.is_empty()
        && dir.join("peer").exists()
        && let Ok(peer) = config::load_peer(dir)
    {
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
            let preferences = MemberPreferences {
                devices: local.node.shared.devices_for(&key),
                audio: local
                    .node
                    .shared
                    .audio_levels
                    .get(&key)
                    .copied()
                    .unwrap_or(MemberPreferences::default().audio),
                output: None,
            };
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
            node.input = None;
            node.monitor_input = None;
            node.monitor_error = None;
            node.audio_playing = false;
            node.microphone_active = false;
            node.microphone_error = None;
            node.audio_error = None;
            node.input_error = None;
            state.members.insert(
                key.clone(),
                Member {
                    node,
                    address,
                    online: false,
                    native_output: None,
                    preferences,
                    input_ready: 0,
                },
            );
            state.hub = state.hub.min(key);
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
    let (snapshot, mut snapshots) = tokio::sync::watch::channel::<Option<Arc<Packet>>>(None);
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
                    .send(Incoming::Packet(reader_key.clone(), generation, packet))
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
                changed=snapshots.changed()=>{
                    if changed.is_err(){break Ok(());}
                    let latest=snapshots.borrow_and_update().clone();
                    if let Some(latest)=latest && let Err(error)=timeout(Duration::from_secs(30),write_packet(&mut write,&noise,&latest)).await.map_err(anyhow::Error::from).and_then(|result|result){break Err(error);}
                }
                _ = tokio::time::sleep(Duration::from_secs(2)) => {
                    if let Err(error) = timeout(AUTH, write_packet(&mut write, &noise, &Packet::Ping)).await.map_err(anyhow::Error::from).and_then(|result|result) { break Err(error); }
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
    Session {
        generation,
        commands: send,
        clipboard,
        snapshot,
    }
}

struct Session {
    generation: [u8; 32],
    commands: queue::Sender<Packet>,
    clipboard: tokio::sync::watch::Sender<Option<Arc<Packet>>>,
    snapshot: tokio::sync::watch::Sender<Option<Arc<Packet>>>,
}
type Sessions = BTreeMap<String, Session>;
fn send(sessions: &Sessions, key: &str, packet: Packet) -> Result<()> {
    sessions
        .get(key)
        .context("The computer is offline")?
        .commands
        .try_send(packet)
        .context("The computer's command queue is busy")?;
    Ok(())
}
fn broadcast(sessions: &Sessions, state: &Workspace) {
    let snapshot = Arc::new(Packet::State(Box::new(state.clone())));
    for session in sessions.values() {
        session.snapshot.send_replace(Some(snapshot.clone()));
    }
}

fn clipboard_data(mime: &str, bytes: &[u8], digest: &str) -> Result<()> {
    ensure!(
        ["text/plain", "image/png"].contains(&mime) && bytes.len() <= 8 * 1024 * 1024,
        "Unsupported clipboard data"
    );
    use sha2::{Digest, Sha256};
    ensure!(
        digest.len() == 64 && format!("{:x}", Sha256::digest(bytes)) == digest,
        "Clipboard checksum mismatch"
    );
    Ok(())
}

#[derive(Default)]
struct UpdateReceipts(VecDeque<(String, workspace::update::Message)>);
impl UpdateReceipts {
    fn queued(&self, target: &str, message: &workspace::update::Message) -> bool {
        self.0
            .iter()
            .any(|(peer, value)| peer == target && value == message)
    }
    fn permit(&self, target: &str, message: &workspace::update::Message) -> Result<()> {
        ensure!(
            self.0.len() < 128 || self.queued(target, message),
            "Too many update commands are awaiting delivery"
        );
        Ok(())
    }
    fn remember(&mut self, target: String, message: workspace::update::Message) {
        if !self.queued(&target, &message) {
            self.0.push_back((target, message));
        }
    }
    fn confirm(&mut self, source: &str, message: &workspace::update::Message) -> bool {
        let Some(index) = self
            .0
            .iter()
            .position(|(peer, value)| peer == source && value == message)
        else {
            return false;
        };
        self.0.remove(index);
        true
    }
    fn closed(&mut self, peer: &str, hub: &str) {
        if peer == hub {
            self.0.clear();
        } else {
            self.0.retain(|(target, _)| target != peer);
        }
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
        Action::Activate { key } => state.activate(&key, None)?,
        Action::ActivateEdge { key, position } => state.activate(&key, Some(position))?,
        Action::ReturnInput => state.return_input()?,
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
            ensure!(
                state.members.contains_key(&target),
                "Unknown management destination"
            );
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
            ensure!(
                state.members.contains_key(&target),
                "Unknown update destination"
            );
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
                state.members.get(source).is_some_and(Member::compatible),
                "The clipboard source is offline or needs an update"
            );
            clipboard_data(&mime, &bytes, &digest)?;
            if source != own {
                notify.event(Event::Clipboard {
                    source: source.into(),
                    mime: mime.clone(),
                    bytes: bytes.clone(),
                    digest: digest.clone(),
                });
            }
            let packet = Arc::new(Packet::Clipboard {
                source: source.into(),
                mime,
                bytes,
                digest,
            });
            for (key, session) in sessions {
                if key != source && state.members.get(key).is_some_and(Member::compatible) {
                    session.clipboard.send_replace(Some(packet.clone()));
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn local_action(
    action: Action,
    own: &str,
    state: &mut Workspace,
    sessions: &Sessions,
    notify: &Notify,
    identity: &config::Identity,
    internal: &queue::Sender<Incoming>,
    receipts: &mut UpdateReceipts,
) -> Result<()> {
    let update = if let Action::Update { target, message } = &action {
        message.validate()?;
        ensure!(
            state.members.contains_key(target),
            "Unknown update destination"
        );
        receipts.permit(target, message)?;
        Some((target.clone(), message.clone()))
    } else {
        None
    };
    if state.hub == own {
        apply(
            action, own, own, state, sessions, notify, identity, internal,
        )?;
    } else if matches!(&action, Action::Clipboard { .. }) {
        sessions
            .get(&state.hub)
            .context("The workspace controller is offline")?
            .clipboard
            .send_replace(Some(Arc::new(Packet::Action(action))));
    } else {
        send(sessions, &state.hub, Packet::Action(action))?;
    }
    if let Some((target, message)) = update {
        if target == own && state.hub == own {
            notify.event(Event::UpdateSent(message));
        } else {
            receipts.remember(target, message);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn receive_packet(
    key: &str,
    packet: Packet,
    own: &str,
    state: &mut Workspace,
    sessions: &Sessions,
    notify: &Notify,
    identity: &config::Identity,
    internal: &queue::Sender<Incoming>,
    receipts: &mut UpdateReceipts,
) -> Result<()> {
    ensure!(state.members.contains_key(key), "Unknown workspace member");
    match packet {
        Packet::Ping => {}
        Packet::State(mut remote) if key == state.hub && state.hub != own => {
            remote.validate()?;
            ensure!(
                remote.hub == key && remote.members.contains_key(own),
                "Invalid controller state"
            );
            ensure!(remote.revision >= state.revision, "Stale controller state");
            let local = &state.members[own];
            let observed = remote.members.get_mut(own).unwrap();
            observed.node = local.node.clone();
            observed.native_output = local.native_output;
            observed.input_ready = local.input_ready.min(remote.activation);
            remote.validate()?;
            *state = *remote;
        }
        Packet::Node(node, native, ready) if state.hub == own => {
            state.update_node(key, *node, native, ready)?;
        }
        Packet::Action(action) if state.hub == own => apply(
            action, key, own, state, sessions, notify, identity, internal,
        )?,
        Packet::Management {
            source,
            target,
            message,
        } if key == state.hub && target == own => {
            ensure!(
                state.members.contains_key(&source),
                "Unknown management source"
            );
            message.validate()?;
            notify.event(Event::Management { source, message });
        }
        Packet::Update {
            source,
            target,
            message,
        } if key == state.hub && target == own => {
            ensure!(state.members.contains_key(&source), "Unknown update source");
            message.validate()?;
            send(
                sessions,
                &state.hub,
                Packet::UpdateAck {
                    source: own.into(),
                    target: source.clone(),
                    message: message.clone(),
                },
            )?;
            notify.event(Event::Update { source, message });
        }
        Packet::UpdateAck {
            source,
            target,
            message,
        } if (state.hub == own && source == key) || (key == state.hub && target == own) => {
            ensure!(
                state.members.contains_key(&source) && state.members.contains_key(&target),
                "Unknown update acknowledgement"
            );
            message.validate()?;
            if target == own {
                if receipts.confirm(&source, &message) {
                    notify.event(Event::UpdateSent(message));
                }
            } else {
                send(
                    sessions,
                    &target.clone(),
                    Packet::UpdateAck {
                        source,
                        target,
                        message,
                    },
                )?;
            }
        }
        Packet::Clipboard {
            source,
            mime,
            bytes,
            digest,
        } if key == state.hub && state.settings.clipboard_enabled => {
            ensure!(
                state.members.get(&source).is_some_and(Member::compatible),
                "Unknown clipboard source"
            );
            clipboard_data(&mime, &bytes, &digest)?;
            if source != own {
                notify.event(Event::Clipboard {
                    source,
                    mime,
                    bytes,
                    digest,
                });
            }
        }
        Packet::Error(error) => {
            ensure!(
                error.len() <= 1024 && !error.chars().any(char::is_control),
                "Invalid peer error"
            );
            notify.event(Event::Error(error));
        }
        _ => bail!("Unexpected workspace message"),
    }
    Ok(())
}

async fn run(
    listener: std::net::TcpListener,
    dir: PathBuf,
    mut state: Workspace,
    mut commands: queue::Receiver<Command>,
    status_commands: Arc<StatusCommand>,
    clipboard_commands: Arc<ClipboardCommand>,
    notify: Notify,
) -> Result<()> {
    let listener = TcpListener::from_std(listener)?;
    let identity = config::load_identity(&dir)?;
    let own = config::hex(&identity.public);
    let (internal, mut incoming) = queue::channel(256);
    let permits = Arc::new(tokio::sync::Semaphore::new(8));
    let mut sessions = BTreeMap::new();
    let mut receipts = UpdateReceipts::default();
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
                #[cfg(test)]
                Some(Command::Raw(target, packet)) => { let _ = send(&sessions, &target, *packet); }
                Some(Command::Approve(key, accept)) => { if let Some(approved) = approvals.remove(&key) { let _: Result<(), bool> = approved.send(accept); } }
                Some(Command::Action(action)) => {
                    if let Err(error) = local_action(*action, &own, &mut state, &sessions, &notify, &identity, &internal, &mut receipts) { notify.event(Event::Error(error.to_string())); }
                }
            },
            _ = status_commands.changed.notified() => {
                let latest = status_commands.latest.lock().unwrap().take();
                if let Some((node,native,ready)) = latest {
                    match state.update_node(&own, *node.clone(), native, ready) {
                        Ok(()) => if state.hub != own && let Some(hub) = sessions.get(&state.hub) { hub.snapshot.send_replace(Some(Arc::new(Packet::Node(node,native,ready)))); },
                        Err(error) => notify.event(Event::Error(error.to_string())),
                    }
                }
            }
            _ = clipboard_commands.changed.notified() => {
                let latest = clipboard_commands.latest.lock().unwrap().take();
                if let Some(action) = latest
                    && let Err(error) = local_action(action, &own, &mut state, &sessions, &notify, &identity, &internal, &mut receipts)
                { notify.event(Event::Error(error.to_string())); }
            }
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
                    let key = member.node.key.clone();
                    if sessions.get(&key).is_some_and(|session:&Session| session.generation <= candidate.connection) { continue; }
                    let result = (|| {
                        let mut next = state.clone();
                        let mut local_address = candidate.stream.local_addr()?;
                        if let SocketAddr::V6(address) = local_address && let Some(ip) = address.ip().to_ipv4_mapped() {local_address = SocketAddr::new(ip.into(), address.port());}
                        local_address.set_port(listener.local_addr()?.port());
                        next.members.get_mut(&own).unwrap().address = local_address;
                        if controller {
                            ensure!(state.hub == own, "Add computers through the workspace controller");
                            let mut member = *member;
                            if let Some(saved) = state.members.get(&key) { member.preferences = saved.preferences.clone(); }
                            next.members.insert(key.clone(), member);
                            next.revision = state.revision.checked_add(1).filter(|revision|*revision<u64::MAX).context("Workspace revision exhausted")?;
                        } else {
                            ensure!(state.members.len() <= 1 || state.hub == key, "Leave the existing workspace before joining another");
                            next.hub = key.clone(); next.members.insert(key.clone(), *member);
                        }
                        next.validate()?;
                        let connection = start_session(candidate, internal.clone());
                        if controller { connection.snapshot.send_replace(Some(Arc::new(Packet::State(Box::new(next.clone()))))); }
                        else { let local=&next.members[&own];connection.snapshot.send_replace(Some(Arc::new(Packet::Node(Box::new(local.node.clone()),local.native_output,local.input_ready)))); }
                        sessions.insert(key, connection);
                        state = next;
                        connected_once = true;
                        Ok::<(), anyhow::Error>(())
                    })();
                    if let Err(error) = result { notify.event(Event::Error(error.to_string())); }
                }
                Incoming::Packet(key, generation, packet) => {
                    if !sessions.get(&key).is_some_and(|session| session.generation == generation) { continue; }
                    if let Err(error) = receive_packet(&key, packet, &own, &mut state, &sessions, &notify, &identity, &internal, &mut receipts) {
                        let _ = send(&sessions, &key, Packet::Error(workspace::management::status_text(error.to_string(),1024)));
                    }
                }
                Incoming::Closed(key, generation, error) => {
                    if generation.is_some() && sessions.get(&key).is_some_and(|session| Some(session.generation) == generation) {
                        sessions.remove(&key);
                        receipts.closed(&key, &state.hub);
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
            if (state.revision != before.revision || state.hub != before.hub)
                && let Err(error) = save(&dir, &state)
            {
                notify.event(Event::Error(format!(
                    "Workspace settings could not be saved: {error}"
                )));
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

    #[test]
    fn update_receipts_require_the_original_destination_and_exact_message() {
        let mut receipts = UpdateReceipts::default();
        let message = workspace::update::Message::Handoff(42);
        receipts.remember("destination".into(), message.clone());
        receipts.remember("destination".into(), message.clone());
        assert_eq!(receipts.0.len(), 1);
        assert!(!receipts.confirm("another PC", &message));
        assert!(!receipts.confirm("destination", &workspace::update::Message::Commit(42)));
        assert!(receipts.confirm("destination", &message));
        assert!(!receipts.confirm("destination", &message));
        for id in 1..=128 {
            receipts.remember(
                "destination".into(),
                workspace::update::Message::Handoff(id),
            );
        }
        assert!(
            receipts
                .permit("destination", &workspace::update::Message::Handoff(129))
                .is_err()
        );
        assert!(
            receipts
                .permit("destination", &workspace::update::Message::Handoff(1))
                .is_ok()
        );
        receipts.closed("destination", "controller");
        assert!(receipts.0.is_empty());
    }

    #[test]
    fn maximum_binary_clipboards_fit_the_wire_limit_and_local_copies_coalesce() {
        use sha2::{Digest, Sha256};
        let bytes = vec![255; 8 * 1024 * 1024];
        let digest = format!("{:x}", Sha256::digest(&bytes));
        for packet in [
            Packet::Action(Action::Clipboard {
                mime: "image/png".into(),
                bytes: bytes.clone(),
                digest: digest.clone(),
            }),
            Packet::Clipboard {
                source: "source".into(),
                mime: "image/png".into(),
                bytes,
                digest,
            },
        ] {
            let encoded = serde_json::to_vec(&packet).unwrap();
            assert!(encoded.len() < MAX_PACKET);
            let decoded: Packet = serde_json::from_slice(&encoded).unwrap();
            let (mime, bytes, digest) = match decoded {
                Packet::Action(Action::Clipboard {
                    mime,
                    bytes,
                    digest,
                })
                | Packet::Clipboard {
                    mime,
                    bytes,
                    digest,
                    ..
                } => (mime, bytes, digest),
                _ => panic!("Unexpected clipboard round trip"),
            };
            assert_eq!(bytes.len(), 8 * 1024 * 1024);
            clipboard_data(&mime, &bytes, &digest).unwrap();
        }

        let (commands, mut receive) = queue::channel(1);
        let (_, events) = mpsc::channel();
        let service = Service {
            commands,
            events,
            snapshot: Default::default(),
            clipboard: Default::default(),
            clipboard_commands: Default::default(),
            status_commands: Default::default(),
        };
        service.action(Action::ReturnInput).unwrap();
        for index in 0..100u8 {
            service
                .action(Action::Clipboard {
                    mime: "text/plain".into(),
                    bytes: vec![index; 1024],
                    digest: String::new(),
                })
                .unwrap();
        }
        assert!(
            matches!(receive.try_recv().unwrap(),Command::Action(action) if matches!(*action,Action::ReturnInput))
        );
        assert!(receive.try_recv().is_err());
        assert!(
            matches!(service.clipboard_commands.latest.lock().unwrap().as_ref(),Some(Action::Clipboard{bytes,..}) if bytes==&vec![99;1024])
        );
    }

    #[test]
    fn terminal_monitor_status_survives_full_control_queues_and_snapshot_coalescing() {
        let (commands, mut receive) = queue::channel(1);
        let (_, events) = mpsc::channel();
        let service = Service {
            commands,
            events,
            snapshot: Default::default(),
            clipboard: Default::default(),
            clipboard_commands: Default::default(),
            status_commands: Default::default(),
        };
        service.action(Action::ReturnInput).unwrap();
        assert!(service.action(Action::ReturnInput).is_err());
        let mut state = model::tests::fixture(2);
        let own = state.hub.clone();
        service.publish(state.members[&own].node.clone(), None, 1);
        let mut completed = state.members[&own].node.clone();
        completed.name = "Terminal monitor acknowledgement".into();
        service.publish(completed.clone(), None, 2);
        let (latest, _, ready) = service
            .status_commands
            .latest
            .lock()
            .unwrap()
            .take()
            .unwrap();
        assert_eq!(*latest, completed);
        assert_eq!(ready, 2);
        assert!(matches!(receive.try_recv().unwrap(), Command::Action(_)));
        assert!(receive.try_recv().is_err());

        let (commands, mut control) = queue::channel(1);
        commands.try_send(Packet::Ping).ok().unwrap();
        let (clipboard, _clips) = tokio::sync::watch::channel(None);
        let (snapshot, snapshots) = tokio::sync::watch::channel(None);
        let mut sessions = BTreeMap::new();
        sessions.insert(
            "peer".into(),
            Session {
                generation: [1; 32],
                commands,
                clipboard,
                snapshot,
            },
        );
        broadcast(&sessions, &state);
        state.activation = 2;
        state.members.get_mut(&own).unwrap().input_ready = 2;
        broadcast(&sessions, &state);
        assert!(
            matches!(&**snapshots.borrow().as_ref().unwrap(), Packet::State(latest) if latest.members[&own].input_ready == 2)
        );
        assert!(matches!(control.try_recv().unwrap(), Packet::Ping));
        assert!(control.try_recv().is_err());
    }

    #[test]
    fn migration_preserves_existing_identity_device_choices_and_levels() {
        let root = std::env::temp_dir().join(format!(
            "desk-switch-group-migration-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let identity = config::create_identity(&root).unwrap();
        let own = config::hex(&identity.public);
        let fixture = model::tests::fixture(2);
        let peer = fixture.members.keys().next().unwrap().clone();
        config::pair(&root, &peer).unwrap();
        std::fs::write(
            root.join("desktop.toml"),
            "[workspace]\nlast_address='[::1]:45878'\npeer_name='Ubuntu'\n",
        )
        .unwrap();
        let identity_bytes = std::fs::read(root.join("identity")).unwrap();
        let peer_bytes = std::fs::read(root.join("peer")).unwrap();
        let mut local = fixture.members.values().next().unwrap().clone();
        local.node.key = own.clone();
        local.node.shared.audio_levels.insert(
            own.clone(),
            workspace::AudioLevel {
                volume: 75,
                muted: false,
            },
        );
        local.node.shared.audio_levels.insert(
            peer.clone(),
            workspace::AudioLevel {
                volume: 25,
                muted: true,
            },
        );
        local.node.shared.devices.insert(
            peer.clone(),
            workspace::devices::Choices {
                speaker: Some("saved output".into()),
                ..Default::default()
            },
        );
        local.node.shared.devices.insert(
            own.clone(),
            workspace::devices::Choices {
                keyboard: Some("saved keyboard".into()),
                ..Default::default()
            },
        );
        let state = load(&root, local.clone()).unwrap();
        assert_eq!(state.members[&own].preferences.audio.volume, 75);
        assert_eq!(state.members[&peer].preferences.audio.volume, 25);
        assert!(state.members[&peer].preferences.audio.muted);
        assert_eq!(
            state.members[&peer].preferences.devices.speaker.as_deref(),
            Some("saved output")
        );
        assert_eq!(
            state.members[&own].preferences.devices.keyboard.as_deref(),
            Some("saved keyboard")
        );
        assert_eq!(
            std::fs::read(root.join("identity")).unwrap(),
            identity_bytes
        );
        assert_eq!(std::fs::read(root.join("peer")).unwrap(), peer_bytes);
        save(&root, &state).unwrap();
        local.node.shared = Default::default();
        let reloaded = load(&root, local).unwrap();
        assert_eq!(reloaded.settings, state.settings);
        assert_eq!(
            reloaded.members[&own].preferences,
            state.members[&own].preferences
        );
        assert_eq!(
            reloaded.members[&peer].preferences,
            state.members[&peer].preferences
        );
        std::fs::remove_dir_all(root).unwrap();
    }

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
        until(&mut computers, &mut forwarded, |computers, _| {
            computers.iter().all(|computer| computer.state.is_some())
        });
        for index in 1..4 {
            let address = computers[index].state.as_ref().unwrap().members[&keys[index]].address;
            computers[0]
                .service
                .action(Action::Invite {
                    address,
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

        // A malformed authenticated peer report must fail only that message;
        // the controller and every other link remain usable afterwards.
        let mut invalid = computers[1].member.node.clone();
        invalid.hardware.audio_score = 101;
        computers[1]
            .service
            .commands
            .try_send(Command::Raw(
                keys[0].clone(),
                Box::new(Packet::Node(Box::new(invalid), None, 0)),
            ))
            .unwrap();
        until(&mut computers, &mut forwarded, |_, events| {
            events.iter().any(|(index,event)|*index==1 && matches!(event,Event::Error(error) if error.contains("invalid audio information")))
        });

        for (sender, target, id) in [(3, 2, 43), (0, 2, 44), (2, 0, 45)] {
            let message = workspace::update::Message::Handoff(id);
            computers[sender]
                .service
                .action(Action::Update {
                    target: keys[target].clone(),
                    message: message.clone(),
                })
                .unwrap();
            until(&mut computers, &mut forwarded, |_, events| {
                events.iter().any(|(index,event)|*index==target && matches!(event,Event::Update {source,message:received} if source==&keys[sender] && received==&message))
            });
            until(&mut computers, &mut forwarded, |_, events| {
                events.iter().any(|(index, event)| {
                    *index == sender
                        && matches!(event,Event::UpdateSent(received) if received==&message)
                })
            });
            assert!(!forwarded.iter().any(|(index, event)| *index != sender
                && matches!(event,Event::UpdateSent(received) if received==&message)));
        }
        use sha2::{Digest, Sha256};
        let bytes = b"Shared clipboard across four authenticated PCs".to_vec();
        let digest = format!("{:x}", Sha256::digest(&bytes));
        computers[3]
            .service
            .action(Action::Clipboard {
                mime: "text/plain".into(),
                bytes: bytes.clone(),
                digest: digest.clone(),
            })
            .unwrap();
        until(&mut computers, &mut forwarded, |_, events| {
            (0..3).all(|receiver|events.iter().any(|(index,event)|*index==receiver && matches!(event,Event::Clipboard {source,bytes:received,digest:checksum,..} if source==&keys[3] && received==&bytes && checksum==&digest)))
        });
        assert!(
            !forwarded.iter().any(|(index, event)| *index == 3
                && matches!(event,Event::Clipboard {digest:checksum,..} if checksum==&digest)),
            "Do not echo the source's own clipboard back to it"
        );
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
