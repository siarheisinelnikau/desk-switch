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
    collections::{BTreeMap, BTreeSet, VecDeque},
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

#[path = "net/coordination.rs"]
mod coordination;

fn canonical_address(address: SocketAddr) -> SocketAddr {
    if let SocketAddr::V6(value) = address
        && let Some(ip) = value.ip().to_ipv4_mapped()
    {
        SocketAddr::new(ip.into(), value.port())
    } else {
        // A genuine IPv6 endpoint keeps its flow information and interface scope.
        address
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Action {
    SetLayout {
        expected: u64,
        layout: super::layout::Layout,
    },
    ActivateDisplay {
        target: super::layout::DisplayTarget,
        entry: Option<crate::edge::Entry>,
    },
    CrossEdge {
        crossing: crate::edge::Crossing,
    },
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
    EdgePause(bool),
    MonitorResult {
        id: u64,
        error: Option<String>,
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
    Reconnecting {
        key: String,
        reason: String,
    },
    Coordination {
        leader: Option<String>,
        writable: bool,
        online: usize,
        quorum: usize,
    },
    Error(String),
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
enum Intent {
    Invite,
    Join,
    Mesh,
}

#[derive(Deserialize, Serialize)]
enum Packet {
    Hello {
        protocol: u16,
        intent: Intent,
        member: Box<Member>,
        hub: String,
        count: usize,
        workspace_id: String,
    },
    Decision(bool),
    Bootstrap(Box<super::consensus::HardState>),
    Consensus(super::consensus::Rpc),
    Observation(Box<Member>),
    Directory(BTreeMap<String, SocketAddr>),
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
    #[cfg(test)]
    FailCommitPersistence(mpsc::Sender<()>),
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
        mesh: bool,
        workspace_id: String,
        dial: Option<DialPermit>,
    },
    Packet(String, [u8; 32], Packet),
    DialFailed(String, anyhow::Error),
    Closed(String, Option<[u8; 32]>, Option<anyhow::Error>),
}

#[derive(Clone)]
struct Internal {
    sender: queue::Sender<Incoming>,
    dialing: Arc<Mutex<BTreeSet<String>>>,
}
impl Internal {
    async fn send(&self, event: Incoming) -> Result<()> {
        self.sender
            .send(event)
            .await
            .map_err(|_| anyhow::anyhow!("Workspace event loop stopped"))
    }
    fn reserve_dial(&self, peer: &str, address: SocketAddr) -> Option<DialPermit> {
        let key = if peer.is_empty() {
            format!("address:{}", canonical_address(address))
        } else {
            format!("peer:{peer}")
        };
        self.dialing
            .lock()
            .unwrap()
            .insert(key.clone())
            .then(|| DialPermit {
                key,
                dialing: self.dialing.clone(),
            })
    }
}
struct DialPermit {
    key: String,
    dialing: Arc<Mutex<BTreeSet<String>>>,
}
impl Drop for DialPermit {
    fn drop(&mut self) {
        self.dialing.lock().unwrap().remove(&self.key);
    }
}

fn retryable_connection_error(error: &anyhow::Error) -> bool {
    // A malformed authenticated packet remains a hard failure even when a
    // wrapper also carries an I/O-shaped error. Never classify by message text.
    let protocol_cause = |cause: &(dyn std::error::Error + 'static)| {
        cause.is::<serde_json::Error>() || cause.is::<snow::Error>()
    };
    if error.chain().any(|cause| {
        protocol_cause(cause)
            || cause
                .downcast_ref::<std::io::Error>()
                .and_then(std::io::Error::get_ref)
                .is_some_and(|inner| protocol_cause(inner))
    }) {
        return false;
    }
    error.chain().any(|cause| {
        cause.is::<tokio::time::error::Elapsed>()
            || cause.downcast_ref::<std::io::Error>().is_some_and(|error| {
                matches!(
                    error.kind(),
                    std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::NotConnected
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::ConnectionRefused
                        | std::io::ErrorKind::NetworkDown
                        | std::io::ErrorKind::NetworkUnreachable
                        | std::io::ErrorKind::HostUnreachable
                        | std::io::ErrorKind::AddrNotAvailable
                        | std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::WriteZero
                )
            })
    })
}

fn closure_event(key: String, known: bool, error: anyhow::Error) -> Event {
    let reason = format!("{error:#}");
    if known && retryable_connection_error(&error) {
        Event::Reconnecting { key, reason }
    } else {
        Event::Error(reason)
    }
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
impl Service {
    pub fn start(dir: PathBuf, mut local: Member, port: u16, ctx: egui::Context) -> Result<Self> {
        let listener = crate::listen::wildcard_tcp(port, 16)?;
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
            authority: None,
            revision: 0,
            settings: Settings::from(&local.node.shared),
            members: BTreeMap::new(),
            active_input: None,
            input_intent: None,
            input_request: 0,
            active_input_entry: None,
            activation: 0,
            active_monitor: None,
            confirmed_monitor: None,
            monitor_command: None,
            layout: Default::default(),
            active_display: None,
            display_entry: None,
            edge_paused: false,
            edge_policy_revision: 0,
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
        local.input_ready = saved.input_ready;
        local.node.monitor_error = saved.node.monitor_error.clone();
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

/// A greeting reports hardware, not authority over the controller's activation
/// counter, device preferences or monitor completion. Its sender can have a
/// newer cached workspace, or come from an unrelated singleton workspace.
fn hello_observation(state: &Workspace, mut member: Member) -> Member {
    if let Some(saved) = state.members.get(&member.node.key) {
        member.preferences = saved.preferences.clone();
        member.input_ready = saved.input_ready;
        member.node.monitor_error = saved.node.monitor_error.clone();
    } else {
        member.input_ready = 0;
        member.node.monitor_error = None;
    }
    member
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
    internal: Internal,
    dial: Option<DialPermit>,
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
            workspace_id: state.workspace_id().to_owned(),
        },
    )
    .await?;
    let Packet::Hello {
        protocol,
        intent: remote_intent,
        mut member,
        hub: remote_hub,
        count,
        workspace_id,
    } = timeout(AUTH, read_packet(&mut candidate.stream, &noise)).await??
    else {
        bail!("Missing workspace handshake")
    };
    ensure!(
        protocol == model::PROTOCOL && member.node.key == peer,
        "Incompatible workspace handshake"
    );
    member.address = canonical_address(candidate.address);
    member.online = true;
    member.validate(&remote_hub)?;
    model::valid_key(&workspace_id)?;
    let requested = if outgoing { intent } else { remote_intent };
    let mesh = trusted && workspace_id == state.workspace_id();
    ensure!(
        !matches!(requested, Intent::Mesh) || mesh,
        "The saved computer belongs to a different workspace; invite it explicitly"
    );
    let controller = matches!(
        (outgoing, requested),
        (true, Intent::Invite) | (false, Intent::Join)
    );
    if !mesh && controller {
        ensure!(
            state.hub == local.node.key,
            "Add computers through the workspace controller"
        );
    } else if !mesh {
        ensure!(remote_hub == peer, "Connect to the workspace controller");
        ensure!(
            state.members.len() <= 1 || state.hub == peer,
            "Leave the existing workspace before joining another"
        );
    }
    if !mesh && controller && matches!(requested, Intent::Invite) {
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
            mesh,
            workspace_id,
            dial,
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
    internal: Internal,
) {
    let Some(permit) = internal.reserve_dial(&expected, address) else {
        return;
    };
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
            setup(
                candidate,
                local,
                state,
                intent,
                internal.clone(),
                Some(permit),
            )
            .await
        }
        .await;
        if let Err(error) = result {
            let _ = internal.send(Incoming::DialFailed(expected, error)).await;
        }
    });
}

/// Writes one packet with a bounded wait, so a stalled peer cannot keep the
/// session open forever. The limit is a parameter because a keep-alive ping
/// uses a shorter budget than a real packet.
async fn write_bounded(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    noise: &Arc<Mutex<snow::TransportState>>,
    packet: &Packet,
    limit: Duration,
) -> Result<()> {
    match timeout(limit, write_packet(writer, noise, packet)).await {
        Ok(result) => result,
        Err(error) => Err(anyhow::Error::from(error)),
    }
}

fn start_session(candidate: Candidate, internal: Internal) -> Session {
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
                    if let Err(error) = write_bounded(&mut write, &noise, &packet, Duration::from_secs(30)).await { break Err(error); }
                }
                changed=clips.changed()=>{
                    if changed.is_err(){break Ok(());}
                    let latest=clips.borrow_and_update().clone();
                    if let Some(latest)=latest && let Err(error)=write_bounded(&mut write,&noise,&latest,Duration::from_secs(30)).await{break Err(error);}
                }
                changed=snapshots.changed()=>{
                    if changed.is_err(){break Ok(());}
                    let latest=snapshots.borrow_and_update().clone();
                    if let Some(latest)=latest && let Err(error)=write_bounded(&mut write,&noise,&latest,Duration::from_secs(30)).await{break Err(error);}
                }
                _ = tokio::time::sleep(Duration::from_secs(2)) => {
                    if let Err(error) = write_bounded(&mut write, &noise, &Packet::Ping, AUTH).await { break Err(error); }
                }
            }
        };
        reader.abort();
        let _ = internal
            .send(Incoming::Closed(key, Some(generation), result.err()))
            .await;
    });
    Session {
        generation,
        commands: send,
        clipboard,
        snapshot,
        directory_queued: false,
    }
}

struct Session {
    generation: [u8; 32],
    commands: queue::Sender<Packet>,
    clipboard: tokio::sync::watch::Sender<Option<Arc<Packet>>>,
    snapshot: tokio::sync::watch::Sender<Option<Arc<Packet>>>,
    directory_queued: bool,
}
type Sessions = BTreeMap<String, Session>;

const MAX_RETIRED_PER_PEER: usize = 4;
#[derive(Default)]
struct Links {
    active: Sessions,
    retired: BTreeMap<String, Vec<Session>>,
}
impl std::ops::Deref for Links {
    type Target = Sessions;
    fn deref(&self) -> &Self::Target {
        &self.active
    }
}
impl Links {
    fn accepts(&self, key: &str, generation: [u8; 32]) -> Result<bool> {
        if self
            .active
            .get(key)
            .is_some_and(|session| session.generation <= generation)
        {
            return Ok(false);
        }
        ensure!(
            self.retired
                .get(key)
                .is_none_or(|sessions| sessions.len() < MAX_RETIRED_PER_PEER),
            "Too many unconfirmed workspace handovers for one computer"
        );
        Ok(true)
    }
    fn install(&mut self, key: String, session: Session) -> Result<bool> {
        if !self.accepts(&key, session.generation)? {
            return Ok(false);
        }
        if let Some(previous) = self.active.insert(key.clone(), session) {
            // Retain all senders until a packet on the replacement proves the
            // peer has activated it. Closing the old socket earlier can offline
            // the peer before it processes its replacement Ready event.
            self.retired.entry(key).or_default().push(previous);
        }
        Ok(true)
    }
    fn packet(&mut self, key: &str, generation: [u8; 32]) -> bool {
        if !self
            .active
            .get(key)
            .is_some_and(|session| session.generation == generation)
        {
            return false;
        }
        self.retired.remove(key);
        true
    }
    fn closed(&mut self, key: &str, generation: [u8; 32]) -> bool {
        if self
            .active
            .get(key)
            .is_some_and(|session| session.generation == generation)
        {
            self.active.remove(key);
            self.retired.remove(key);
            return true;
        }
        if let Some(retired) = self.retired.get_mut(key) {
            retired.retain(|session| session.generation != generation);
            if retired.is_empty() {
                self.retired.remove(key);
            }
        }
        false
    }
    fn shutdown(&mut self) {
        self.active.clear();
        self.retired.clear();
    }
}

fn close_session(
    links: &mut Links,
    state: &mut Workspace,
    receipts: &mut UpdateReceipts,
    _own: &str,
    key: &str,
    generation: [u8; 32],
) -> bool {
    if !links.closed(key, generation) {
        return false;
    }
    receipts.closed(key, &state.hub);
    if let Some(member) = state.members.get_mut(key) {
        member.online = false;
    }
    true
}
fn send(sessions: &Sessions, key: &str, packet: Packet) -> Result<()> {
    sessions
        .get(key)
        .context("The computer is offline")?
        .commands
        .try_send(packet)
        .context("The computer's command queue is busy")?;
    Ok(())
}
#[cfg(test)]
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
    fn closed(&mut self, peer: &str, _hub: &str) {
        self.0.retain(|(target, _)| target != peer);
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
    internal: &Internal,
) -> Result<()> {
    ensure!(
        state.members.contains_key(source),
        "Unknown workspace member"
    );
    match action {
        Action::SetLayout { expected, layout } => state.set_layout(expected, layout)?,
        Action::ActivateDisplay { target, entry } => {
            state.activate_display(source, target, entry)?
        }
        Action::CrossEdge { crossing } => {
            if let Some(activation) = super::layout::resolve_crossing(state, source, crossing)? {
                state.activate_display(source, activation.target, Some(activation.entry))?;
            }
        }
        Action::Settings { expected, value } => {
            state.set_settings(expected, value)?;
            state.bind_microphone();
        }
        Action::Member {
            key,
            expected,
            value,
        } => state.set_member(&key, &expected, value)?,
        Action::Activate { key } => state.activate(source, &key, None)?,
        Action::ActivateEdge { key, position } => state.activate(source, &key, Some(position))?,
        Action::ReturnInput => state.return_input()?,
        Action::EdgePause(paused) => state.pause_edges(paused)?,
        Action::MonitorResult { id, error } => state.monitor_result(source, id, error)?,
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

async fn run(
    listener: std::net::TcpListener,
    dir: PathBuf,
    state: Workspace,
    commands: queue::Receiver<Command>,
    status_commands: Arc<StatusCommand>,
    clipboard_commands: Arc<ClipboardCommand>,
    notify: Notify,
) -> Result<()> {
    coordination::run(
        listener,
        dir,
        state,
        commands,
        status_commands,
        clipboard_commands,
        notify,
    )
    .await
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    include!("net/reconnect_tests.rs");
    include!("net/failover_tests.rs");

    fn remove_test_root(root: &Path) {
        // Service shutdown is asynchronous on Windows; a late handle can keep
        // the temp directory non-empty briefly.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match std::fs::remove_dir_all(root) {
                Ok(()) => break,
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(error) => {
                    eprintln!("could not remove {}: {error}", root.display());
                    break;
                }
            }
        }
    }

    pub(super) fn synthetic_session(generation: u8) -> (Session, queue::Receiver<Packet>) {
        let (commands, receive) = queue::channel(1);
        let (clipboard, _clips) = tokio::sync::watch::channel(None);
        let (snapshot, _snapshots) = tokio::sync::watch::channel(None);
        (
            Session {
                generation: [generation; 32],
                commands,
                clipboard,
                snapshot,
                directory_queued: false,
            },
            receive,
        )
    }

    #[test]
    fn handover_keeps_the_old_link_until_the_peer_activates_the_replacement_in_either_event_order()
    {
        for old_closed_first in [false, true] {
            let mut state = model::tests::fixture(3);
            let own = state.hub.clone();
            let peer = state
                .members
                .keys()
                .find(|key| *key != &own)
                .unwrap()
                .clone();
            let mut links = Links::default();
            let mut receipts = UpdateReceipts::default();
            let (old, old_receive) = synthetic_session(3);
            assert!(links.install(peer.clone(), old).unwrap());
            let (new, new_receive) = synthetic_session(2);
            assert!(links.install(peer.clone(), new).unwrap());
            // The remote end can still be on generation3 while its Ready(2)
            // waits in the event queue. Keeping every old sender prevents a FIN.
            assert!(!old_receive.is_closed());
            if old_closed_first {
                assert!(!close_session(
                    &mut links,
                    &mut state,
                    &mut receipts,
                    &own,
                    &peer,
                    [3; 32]
                ));
            }
            assert!(links.packet(&peer, [2; 32]));
            if !old_closed_first {
                assert!(!close_session(
                    &mut links,
                    &mut state,
                    &mut receipts,
                    &own,
                    &peer,
                    [3; 32]
                ));
            }
            assert!(old_receive.is_closed());
            assert!(!new_receive.is_closed());
            assert!(state.members[&peer].online);
            assert_eq!(links[&peer].generation, [2; 32]);
            assert!(!links.packet(&peer, [3; 32]));
        }
    }

    #[test]
    fn a_real_current_link_failure_is_immediately_offline_and_drops_all_retirees() {
        let mut state = model::tests::fixture(3);
        let own = state.hub.clone();
        let peer = state
            .members
            .keys()
            .find(|key| *key != &own)
            .unwrap()
            .clone();
        let mut links = Links::default();
        let mut receipts = UpdateReceipts::default();
        let (old, old_receive) = synthetic_session(3);
        links.install(peer.clone(), old).unwrap();
        let (new, new_receive) = synthetic_session(2);
        links.install(peer.clone(), new).unwrap();
        assert!(close_session(
            &mut links,
            &mut state,
            &mut receipts,
            &own,
            &peer,
            [2; 32]
        ));
        assert!(!state.members[&peer].online);
        assert!(
            state
                .members
                .iter()
                .filter(|(key, _)| *key != &peer)
                .all(|(_, member)| member.online)
        );
        assert!(old_receive.is_closed() && new_receive.is_closed());
        assert!(!links.contains_key(&peer) && !links.retired.contains_key(&peer));
    }

    #[test]
    fn handover_capacity_preserves_protected_links_and_shutdown_releases_them() {
        let mut links = Links::default();
        let mut receives = Vec::new();
        for generation in (6..=10).rev() {
            let (session, receive) = synthetic_session(generation);
            links.install("peer".into(), session).unwrap();
            receives.push(receive);
        }
        let (overflow, overflow_receive) = synthetic_session(5);
        assert!(links.install("peer".into(), overflow).is_err());
        assert_eq!(links.retired["peer"].len(), MAX_RETIRED_PER_PEER);
        assert!(receives.iter().all(|receive| !receive.is_closed()));
        assert!(overflow_receive.is_closed());
        let (other, other_receive) = synthetic_session(1);
        assert!(links.install("other".into(), other).unwrap());
        links.shutdown();
        assert!(receives.iter().all(queue::Receiver::is_closed) && other_receive.is_closed());
        assert!(links.active.is_empty() && links.retired.is_empty());
    }

    #[test]
    fn single_flight_dial_permit_survives_queueing_and_canonicalizes_unknown_manual_addresses() {
        let (sender, _incoming) = queue::channel(1);
        let internal = Internal {
            sender,
            dialing: Default::default(),
        };
        let address = "192.0.2.1:45880".parse().unwrap();
        let permit = internal.reserve_dial("peer", address).unwrap();
        assert!(
            internal
                .reserve_dial("peer", "192.0.2.2:45880".parse().unwrap())
                .is_none()
        );
        // Ready owns this same permit while queued; finishing authentication
        // must not free the slot before the event loop accepts the candidate.
        let (queued, receive) = mpsc::channel();
        assert!(queued.send(permit).is_ok());
        assert!(internal.reserve_dial("peer", address).is_none());
        drop(receive.recv().unwrap());
        assert!(internal.reserve_dial("peer", address).is_some());
        let manual = internal
            .reserve_dial("", "[::ffff:192.0.2.1]:45880".parse().unwrap())
            .unwrap();
        assert!(internal.reserve_dial("", address).is_none());
        drop(manual);
        assert!(internal.reserve_dial("", address).is_some());
    }

    #[test]
    fn known_peer_socket_closures_reconnect_without_turning_protocol_errors_into_transport_errors()
    {
        use std::io::{Error, ErrorKind};
        for kind in [
            ErrorKind::UnexpectedEof,
            ErrorKind::ConnectionReset,
            ErrorKind::ConnectionAborted,
            ErrorKind::BrokenPipe,
            ErrorKind::TimedOut,
            ErrorKind::ConnectionRefused,
            ErrorKind::NetworkUnreachable,
        ] {
            let error =
                anyhow::Error::new(Error::from(kind)).context("Reading the pinned peer's socket");
            assert!(
                matches!(closure_event("peer".into(),true,error),Event::Reconnecting{key,reason} if key=="peer" && !reason.is_empty())
            );
        }
        #[cfg(windows)]
        {
            let reset = Error::from_raw_os_error(10054);
            assert_eq!(reset.kind(), ErrorKind::ConnectionReset);
            assert!(retryable_connection_error(&reset.into()));
        }
        assert!(matches!(
            closure_event(
                "new-peer".into(),
                false,
                Error::from(ErrorKind::ConnectionRefused).into()
            ),
            Event::Error(_)
        ));
        for error in [
            anyhow::Error::new(serde_json::from_slice::<serde_json::Value>(b"{").unwrap_err())
                .context("Reading a complete authenticated packet"),
            anyhow::Error::new(snow::Error::Decrypt).context("Authenticating an encrypted packet"),
            anyhow::anyhow!("Invalid monitor settings"),
            anyhow::anyhow!("unexpected end of file"),
            Error::from(ErrorKind::InvalidData).into(),
            Error::from(ErrorKind::PermissionDenied).into(),
            Error::new(
                ErrorKind::ConnectionReset,
                serde_json::from_slice::<serde_json::Value>(b"{").unwrap_err(),
            )
            .into(),
        ] {
            assert!(
                !retryable_connection_error(&error),
                "A protocol or settings failure was hidden as a reconnect: {error:#}"
            );
            assert!(matches!(
                closure_event("peer".into(), true, error),
                Event::Error(_)
            ));
        }
    }

    #[tokio::test]
    async fn known_peer_timeout_uses_its_typed_elapsed_cause() {
        let elapsed = timeout(Duration::ZERO, std::future::pending::<()>())
            .await
            .unwrap_err();
        let error = anyhow::Error::new(elapsed).context("Waiting for an authenticated peer packet");
        assert!(matches!(
            closure_event("peer".into(), true, error),
            Event::Reconnecting { .. }
        ));
    }

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
                directory_queued: false,
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
        remove_test_root(&root);
    }

    #[test]
    fn mapped_ipv4_members_are_canonical_and_real_ipv6_keeps_its_scope() {
        let mapped: SocketAddr = "[::ffff:127.0.0.1]:45880".parse().unwrap();
        assert_eq!(
            canonical_address(mapped),
            "127.0.0.1:45880".parse().unwrap()
        );
        let scoped = SocketAddr::V6(std::net::SocketAddrV6::new(
            "fe80::1234".parse().unwrap(),
            45880,
            13,
            17,
        ));
        assert_eq!(canonical_address(scoped), scoped);
    }

    #[test]
    fn a_saved_workspace_with_auto_connect_disabled_reconnects_its_pinned_controller_by_ip() {
        let root = std::env::temp_dir().join(format!(
            "desk-switch-manual-controller-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let fixture = model::tests::fixture(3);
        let mut members = Vec::new();
        let mut directories = Vec::new();
        for (index, template) in fixture.members.values().enumerate() {
            let dir = root.join(index.to_string());
            std::fs::create_dir_all(&dir).unwrap();
            let identity = config::create_identity(&dir).unwrap();
            let mut member = template.clone();
            member.node.key = config::hex(&identity.public);
            member.node.management = Some(workspace::management::State {
                application: workspace::management::Application {
                    theme: "dark".into(),
                    language: "en".into(),
                    autostart: false,
                    auto_connect: false,
                    auto_update: false,
                    show_remote_cursor: true,
                },
                update: Default::default(),
            });
            member.preferences.audio.volume = if index == 0 { 41 } else { 23 };
            member.address = "127.0.0.1:9".parse().unwrap();
            member.online = false;
            members.push(member);
            directories.push(dir);
        }
        let keys = members
            .iter()
            .map(|member| member.node.key.clone())
            .collect::<Vec<_>>();
        let mut stored = fixture;
        stored.hub = keys[0].clone();
        stored.members = members[..2]
            .iter()
            .map(|member| (member.node.key.clone(), member.clone()))
            .collect();
        stored.settings.clipboard_enabled = true;
        stored.revision = 7;
        let settings = stored.settings.clone();
        for index in 0..2 {
            save(&directories[index], &stored).unwrap();
            config::pair(&directories[index], &keys[1 - index]).unwrap();
        }
        let identity_files = directories[..2]
            .iter()
            .map(|dir| std::fs::read(dir.join("identity")).unwrap())
            .collect::<Vec<_>>();
        let pin_files = directories[..2]
            .iter()
            .map(|dir| std::fs::read(dir.join("peer")).unwrap())
            .collect::<Vec<_>>();
        let mut computers = members
            .into_iter()
            .zip(directories)
            .map(|(member, dir)| Computer {
                service: Service::start(dir.clone(), member.clone(), 0, egui::Context::default())
                    .unwrap(),
                state: None,
                dir,
                member,
            })
            .collect::<Vec<_>>();
        let mut forwarded = Vec::new();
        until(&mut computers, &mut forwarded, |computers, _| {
            computers.iter().all(|computer| computer.state.is_some())
        });
        std::thread::sleep(Duration::from_millis(50));
        pump(&mut computers, &mut forwarded);
        assert!(!computers[1].state.as_ref().unwrap().members[&keys[0]].online);
        assert!(!computers[0].state.as_ref().unwrap().members[&keys[1]].online);
        let other = computers[2].state.as_ref().unwrap().members[&keys[2]].address;

        let before = forwarded.len();
        computers[1]
            .service
            .action(Action::Invite {
                address: other,
                key: keys[2].clone(),
            })
            .unwrap();
        until(&mut computers, &mut forwarded, |_, events| {
            events[before..].iter().any(|(index, event)| {
                *index == 1 && matches!(event,Event::Error(error) if error.contains("majority"))
            })
        });
        let before = forwarded.len();
        computers[1]
            .service
            .action(Action::Invite {
                address: other,
                key: String::new(),
            })
            .unwrap();
        until(&mut computers, &mut forwarded, |_, events| {
            events[before..]
                .iter()
                .any(|(index, event)| *index == 1 && matches!(event, Event::Error(_)))
        });
        assert_eq!(computers[1].state.as_ref().unwrap().hub, keys[0]);
        assert_eq!(computers[1].state.as_ref().unwrap().members.len(), 2);
        assert_eq!(computers[2].state.as_ref().unwrap().members.len(), 1);

        let controller = computers[0].state.as_ref().unwrap().members[&keys[0]].address;
        computers[1]
            .service
            .action(Action::Invite {
                address: controller,
                key: String::new(),
            })
            .unwrap();
        until(&mut computers, &mut forwarded, |computers, _| {
            computers[..2].iter().all(|computer| {
                computer.state.as_ref().is_some_and(|state| {
                    state.members.len() == 2 && state.members.values().all(|member| member.online)
                })
            })
        });
        for (index, computer) in computers[..2].iter().enumerate() {
            let state = computer.state.as_ref().unwrap();
            assert_eq!(state.workspace_id(), keys[0]);
            assert_eq!(state.settings, settings);
            for key in &keys[..2] {
                assert_eq!(
                    state.members[key].preferences,
                    stored.members[key].preferences
                );
                assert!(state.members[key].address.is_ipv4());
            }
            assert!(
                !state.members[&keys[index]]
                    .node
                    .management
                    .as_ref()
                    .unwrap()
                    .application
                    .auto_connect
            );
            assert_eq!(
                std::fs::read(computer.dir.join("identity")).unwrap(),
                identity_files[index]
            );
            assert_eq!(
                std::fs::read(computer.dir.join("peer")).unwrap(),
                pin_files[index]
            );
        }
        for computer in &computers {
            computer.service.stop();
        }
        drop(computers);
        std::thread::sleep(Duration::from_millis(100));
        remove_test_root(&root);
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
    #[track_caller]
    fn until(
        computers: &mut [Computer],
        forwarded: &mut Vec<(usize, Event)>,
        condition: impl Fn(&[Computer], &[(usize, Event)]) -> bool,
    ) {
        let since = Instant::now();
        let caller = std::panic::Location::caller();
        loop {
            pump(computers, forwarded);
            if condition(computers, forwarded) {
                return;
            }
            assert!(
                since.elapsed() < Duration::from_secs(20),
                "Timed out at {caller}: states=(hub,members,online,authority,revision,activation,source,active,intent,intent-health,coordination)={:?}, events={forwarded:?}",
                computers
                    .iter()
                    .enumerate()
                    .map(|(index, computer)| computer.state.as_ref().map(|state| (
                        &state.hub,
                        state.members.len(),
                        state
                            .members
                            .values()
                            .filter(|member| member.online)
                            .count(),
                        &state.authority,
                        state.revision,
                        state.activation,
                        state.plan().input,
                        &state.active_input,
                        &state.input_intent,
                        state.input_intent.as_ref().map(|key| (
                            state.members[key].compatible(),
                            &state.members[key].node.monitor_error,
                        )),
                        failover_coordination(forwarded, index),
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
        let ready = |computers: &[Computer], events: &[(usize, Event)], count: usize| {
            let Some(first) = computers[0].state.as_ref() else {
                return false;
            };
            let Some(leader) = keys[..count].iter().position(|key| key == &first.hub) else {
                return false;
            };
            failover_coordination(events, leader)
                .is_some_and(|(hub, writable, _, _)| writable && hub.as_ref() == Some(&first.hub))
                && computers[..count].iter().all(|computer| {
                    computer.state.as_ref().is_some_and(|state| {
                        let mut reconciled = state.clone();
                        reconciled.reconcile_input();
                        state.authority.is_some()
                            && state.authority == first.authority
                            && state.revision == first.revision
                            && state.activation == first.activation
                            && state.workspace_id() == keys[0]
                            && state.hub == first.hub
                            && state.members.len() == count
                            && state.members.values().all(|member| member.online)
                            && state.active_input == reconciled.active_input
                            && state.input_intent == reconciled.input_intent
                            && state.activation == reconciled.activation
                    })
                })
        };
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
            until(&mut computers, &mut forwarded, |computers, events| {
                ready(computers, events, index + 1)
            });
        }
        until(&mut computers, &mut forwarded, |computers, events| {
            ready(computers, events, 4)
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
        let restart_events = forwarded.len();
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
        assert!(
            forwarded[restart_events..]
                .iter()
                .any(|(index, event)| *index == 0
                    && matches!(event,Event::Reconnecting {key,..} if key==&keys[3])),
            "A known peer's stop must be reported as reconnecting"
        );
        assert!(
            !forwarded[restart_events..]
                .iter()
                .any(|(_, event)| matches!(event, Event::Error(_))),
            "Ordinary stop/restart recovery produced a hard error: {:?}",
            &forwarded[restart_events..]
        );
        for computer in &computers {
            computer.service.stop();
        }
        drop(computers);
        std::thread::sleep(Duration::from_millis(100));
        remove_test_root(&root);
    }
}
