//! User-authorized file exports over independent trusted-member connections.
//! Original paths stay local; peers can request only a verified export's indices.
mod lazy;
pub mod protocol;
mod source_trash;
mod storage;
#[cfg(all(test, windows))]
pub(crate) mod hosted_source_move {
    pub(crate) use super::source_trash::with_hosted_native_trash;
    pub(crate) use super::storage::SourceSelection;
    pub(crate) use super::storage::source_move::{SourceMoveBinding, SourceMoveState};
}
#[cfg(test)]
mod tests;
mod transport;

use crate::{config, group::clipboard::Clipboard};
use anyhow::{Context, Result, ensure};
use eframe::egui;
use protocol::{CHUNK, Kind, Manifest, Packet, Stamp};
pub use protocol::{Operation, Origin};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Semaphore, mpsc, watch},
};

const PORT: u16 = 45881;
const MAX_JOBS: usize = 128;
const ACTIVE: usize = 4;
const RETRY: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Direction {
    Sending,
    Receiving,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Phase {
    Preparing,
    Connecting,
    Transferring,
    Paused,
    Verifying,
    Completed,
    Cancelled,
    Failed,
}
impl Phase {
    fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed)
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PasteStage {
    Preparing,
    Receiving,
    Paused,
    Verifying,
    Delivered,
    NativeCompleted,
    RemovingOriginals,
    OriginalsRetained,
    Moved,
    Cancelled,
    Failed,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PauseReason {
    User,
    Peer,
    Connection,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PasteView {
    pub operation: Operation,
    pub stage: PasteStage,
    pub destination: Option<String>,
    pub reports_completion: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileEvent {
    /// Emitted once after the locally requested operation exists in snapshot().
    PasteStarted {
        id: String,
    },
    SendStarted {
        id: String,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Transfer {
    pub id: String,
    pub peer: String,
    pub direction: Direction,
    pub origin: Origin,
    pub name: String,
    pub phase: Phase,
    pub bytes: u64,
    pub total: u64,
    pub files: u32,
    pub error: Option<String>,
    pub paths: Vec<PathBuf>,
    pub clipboard_ready: bool,
    pub clipboard_pending: bool,
    pub clipboard_skipped: bool,
    #[serde(default)]
    pub paste: Option<PasteView>,
    #[serde(default)]
    pub pause_reason: Option<PauseReason>,
    #[serde(default)]
    pub current_name: Option<String>,
    #[serde(default)]
    pub recovery_paths: Vec<PathBuf>,
}
#[derive(Clone, Debug)]
pub struct Peer {
    pub address: SocketAddr,
    pub online: bool,
}
#[derive(Clone, Debug, Default)]
pub struct TrustSnapshot {
    pub workspace_id: String,
    pub own: String,
    pub peers: BTreeMap<String, Peer>,
    pub clipboard_enabled: bool,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Record {
    workspace: String,
    export: String,
    stamp: Option<Stamp>,
    created: u64,
    manifest: Option<Manifest>,
    view: Transfer,
    #[serde(default)]
    cancel_notified: bool,
    #[serde(default)]
    user_paused: bool,
    #[serde(default)]
    peer_paused: bool,
    #[serde(default)]
    offer: Option<String>,
    #[serde(default)]
    operation: Operation,
    #[serde(default)]
    native_move: Option<lazy::MoveReceipt>,
    #[serde(default)]
    move_binding: Option<storage::source_move::SourceMoveBinding>,
    #[serde(default)]
    move_outcome: Option<protocol::MoveOutcome>,
    /// Captured only by an actual demand while the original native Cut is live.
    #[serde(default)]
    move_grant: Option<lazy::MoveGrant>,
}
struct Job {
    record: Mutex<Record>,
    persist: Mutex<()>,
    running: AtomicBool,
    cancel: AtomicBool,
    cancelled: tokio::sync::Notify,
    retry: Mutex<Instant>,
    last_wake: Mutex<Instant>,
    cleaned: AtomicBool,
    cleaning: AtomicBool,
    reserved: AtomicBool,
    control_running: AtomicBool,
    paused: Mutex<bool>,
    pause_changed: Condvar,
    move_finalizer: tokio::sync::Mutex<()>,
}
impl Job {
    fn new(record: Record) -> Self {
        let cancelled = record.view.phase == Phase::Cancelled;
        let paused = record.user_paused || record.peer_paused;
        Self {
            record: Mutex::new(record),
            persist: Mutex::new(()),
            running: AtomicBool::new(false),
            cancel: AtomicBool::new(cancelled),
            cancelled: Default::default(),
            retry: Mutex::new(Instant::now()),
            last_wake: Mutex::new(Instant::now()),
            cleaned: AtomicBool::new(false),
            cleaning: AtomicBool::new(false),
            reserved: AtomicBool::new(false),
            control_running: AtomicBool::new(false),
            paused: Mutex::new(paused),
            pause_changed: Condvar::new(),
            move_finalizer: tokio::sync::Mutex::new(()),
        }
    }
}
struct Running(Arc<Job>);
impl Drop for Running {
    fn drop(&mut self) {
        self.0.running.store(false, Ordering::Release);
    }
}
enum ClipboardEvent {
    Begin(String),
    Ready(String, Vec<PathBuf>),
    Cancel(String),
    Offer(crate::group::clipboard_files::provider::RemoteOffer),
}
struct Core {
    dir: PathBuf,
    downloads: PathBuf,
    identity: config::Identity,
    trust: Mutex<TrustSnapshot>,
    jobs: Mutex<BTreeMap<String, Arc<Job>>>,
    clipboard: Mutex<VecDeque<ClipboardEvent>>,
    latest: Mutex<Option<Stamp>>,
    ctx: egui::Context,
    slots: Arc<Semaphore>,
    prepare: Arc<Semaphore>,
    stopping: AtomicBool,
    error: Mutex<Option<String>>,
    collected: Mutex<BTreeSet<String>>,
    garbage: Mutex<Vec<(String, String)>>,
    reservations: Mutex<BTreeMap<String, u64>>,
    events: Mutex<VecDeque<FileEvent>>,
    lazy: Mutex<lazy::State>,
    commands: mpsc::Sender<Command>,
}
enum Command {
    Send {
        id: String,
        target: String,
        paths: Vec<PathBuf>,
    },
    Copied(Vec<PathBuf>),
    CopiedSelection(Option<crate::group::clipboard_files::Selection>),
    NativeBegin(Arc<lazy::Fetch>),
    NativeResult(
        Arc<lazy::Session>,
        crate::group::clipboard_files::provider::NativeResult,
    ),
    AcquireLease {
        offer: String,
        peer: String,
        lease: String,
    },
    ReleaseLease {
        offer: String,
        peer: String,
        lease: String,
    },
    Cancel(String),
    Pause(String),
    Resume(String),
    ClipboardReady(String, bool),
}
pub struct Service {
    core: Arc<Core>,
    commands: mpsc::Sender<Command>,
    trust: watch::Sender<TrustSnapshot>,
    stop: watch::Sender<bool>,
}
fn millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
fn new_id() -> Result<String> {
    let keys = snow::Builder::new(config::NOISE.parse()?).generate_keypair()?;
    Ok(config::hex(&keys.private[..16]))
}
fn job_id(export: &str, peer: &str) -> String {
    format!("{export}.{peer}")
}
fn validate_job_id(id: &str) -> Result<()> {
    let (export, peer) = id
        .split_once('.')
        .context("Invalid file transfer identity")?;
    protocol::valid_id(export)?;
    crate::group::model::valid_key(peer)
}
fn listener(port: u16) -> Result<std::net::TcpListener> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV6,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )
    .and_then(|socket| {
        socket.set_only_v6(false)?;
        #[cfg(unix)]
        socket.set_reuse_address(true)?;
        socket.bind(&SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], port)).into())?;
        Ok(socket)
    })
    .or_else(|error| {
        if !crate::listen::ipv6_unavailable(&error) {
            return Err(error);
        }
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )?;
        // Windows must not share a native service port: two profiles binding
        // the same file-transfer port would split authenticated connections.
        #[cfg(unix)]
        socket.set_reuse_address(true)?;
        socket.bind(&SocketAddr::from(([0, 0, 0, 0], port)).into())?;
        Ok::<_, std::io::Error>(socket)
    })?;
    socket.listen(8)?;
    socket.set_nonblocking(true)?;
    Ok(socket.into())
}
impl Service {
    pub fn start(dir: PathBuf, ctx: egui::Context) -> Result<Self> {
        let downloads = dirs::download_dir()
            .context("The Downloads folder is unavailable")?
            .join("Desk Switch");
        Self::start_at(dir, downloads, PORT, ctx)
    }
    fn start_at(dir: PathBuf, downloads: PathBuf, port: u16, ctx: egui::Context) -> Result<Self> {
        let listener = listener(port).context("Could not open the file transfer connection")?;
        let identity = config::load_identity(&dir)?;
        let (commands, receiver) = mpsc::channel(64);
        let core = Arc::new(Core {
            dir,
            downloads,
            identity,
            trust: Mutex::new(TrustSnapshot::default()),
            jobs: Mutex::new(BTreeMap::new()),
            clipboard: Mutex::new(VecDeque::new()),
            latest: Mutex::new(None),
            ctx,
            slots: Arc::new(Semaphore::new(ACTIVE)),
            prepare: Arc::new(Semaphore::new(1)),
            stopping: AtomicBool::new(false),
            error: Mutex::new(None),
            collected: Mutex::new(BTreeSet::new()),
            garbage: Mutex::new(Vec::new()),
            reservations: Mutex::new(BTreeMap::new()),
            events: Mutex::new(VecDeque::new()),
            lazy: Mutex::new(lazy::State::default()),
            commands: commands.clone(),
        });
        let (trust, updates) = watch::channel(TrustSnapshot::default());
        let (stop, stopping) = watch::channel(false);
        let worker = core.clone();
        std::thread::Builder::new()
            .name("Desk Switch file transfers".into())
            .spawn(move || {
                let result = crate::runtime().and_then(|runtime| {
                    runtime.block_on(run(worker.clone(), listener, receiver, updates, stopping))
                });
                if let Err(error) = result {
                    *worker.error.lock().unwrap() =
                        Some(format!("File transfers stopped: {error:#}"));
                    worker.wake();
                }
            })?;
        Ok(Self {
            core,
            commands,
            trust,
            stop,
        })
    }
    /// Supply only the majority-committed workspace roster, never pending invites.
    pub fn update(&self, mut trust: TrustSnapshot) {
        let valid = trust.own == config::hex(&self.core.identity.public)
            && (16..=128).contains(&trust.workspace_id.len())
            && trust
                .workspace_id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b'-')
            && trust.peers.len() <= 128
            && trust.peers.iter().all(|(key, peer)| {
                key != &trust.own
                    && crate::group::model::valid_key(key).is_ok()
                    && peer.address.port() != 0
                    && !peer.address.ip().is_unspecified()
                    && !peer.address.ip().is_multicast()
            });
        if !valid {
            trust = TrustSnapshot::default();
        }
        self.trust.send_if_modified(|current| {
            let same = current.workspace_id == trust.workspace_id
                && current.own == trust.own
                && current.clipboard_enabled == trust.clipboard_enabled
                && current.peers.len() == trust.peers.len()
                && current.peers.iter().all(|(key, value)| {
                    trust
                        .peers
                        .get(key)
                        .is_some_and(|v| v.address == value.address && v.online == value.online)
                });
            if same {
                false
            } else {
                *current = trust;
                true
            }
        });
    }
    pub fn send(&self, target: String, paths: Vec<PathBuf>) -> Result<String> {
        let trust = self.trust.borrow();
        ensure!(
            trust.peers.contains_key(&target),
            "Choose a trusted workspace computer before sending files"
        );
        ensure!(
            !paths.is_empty()
                && paths.len() <= protocol::MAX_ENTRIES
                && paths.iter().all(|p| p.is_absolute()),
            "Choose files or folders to send"
        );
        {
            let jobs = self.core.jobs.lock().unwrap();
            ensure!(
                jobs.len() < MAX_JOBS
                    || jobs.values().any(|job| {
                        matches!(
                            job.record.lock().unwrap().view.phase,
                            Phase::Completed | Phase::Cancelled
                        ) && !job.running.load(Ordering::Acquire)
                    }),
                "The file transfer queue is full"
            );
        }
        let id = new_id()?;
        let handle = job_id(&id, &target);
        self.commands
            .try_send(Command::Send { id, target, paths })
            .context("The file transfer queue is full")?;
        Ok(handle)
    }
    pub fn cancel(&self, id: &str) -> Result<()> {
        validate_job_id(id)?;
        ensure!(
            !lazy::cleanup_started(&self.core, id),
            "The native Move has finished; its originals are being finalized"
        );
        if let Some(job) = self.core.jobs.lock().unwrap().get(id) {
            let r = job.record.lock().unwrap();
            ensure!(
                r.native_move.is_none() || r.move_outcome.is_some(),
                "The native Move has finished; its originals are being finalized"
            );
            if r.view.phase != Phase::Completed
                || (r.view.direction == Direction::Receiving && r.view.origin == Origin::Clipboard)
            {
                job.cancel.store(true, Ordering::Release);
                job.cancelled.notify_waiters();
                job.pause_changed.notify_all();
            }
        }
        self.commands
            .try_send(Command::Cancel(id.into()))
            .context("The file transfer queue is full")?;
        Ok(())
    }
    pub fn pause(&self, id: &str) -> Result<()> {
        validate_job_id(id)?;
        if let Some(job) = self.core.jobs.lock().unwrap().get(id) {
            let r = job.record.lock().unwrap();
            ensure!(
                !r.view.phase.terminal(),
                "The file transfer is no longer active"
            );
        }
        self.commands
            .try_send(Command::Pause(id.into()))
            .context("The file transfer queue is full")?;
        Ok(())
    }
    pub fn resume(&self, id: &str) -> Result<()> {
        validate_job_id(id)?;
        if let Some(job) = self.core.jobs.lock().unwrap().get(id) {
            let r = job.record.lock().unwrap();
            ensure!(
                r.manifest.is_some() || (r.offer.is_some() && r.view.paste.is_some()),
                "Choose the files or folders again to retry preparation"
            );
            ensure!(
                r.view.phase != Phase::Cancelled,
                "A cancelled transfer cannot be resumed; send the files again"
            );
            ensure!(
                !job.cancel.load(Ordering::Acquire),
                "Paste or send the files again to retry this cancelled native operation"
            );
        }
        self.commands
            .try_send(Command::Resume(id.into()))
            .context("The file transfer queue is full")?;
        Ok(())
    }
    pub fn snapshot(&self) -> Vec<Transfer> {
        let jobs = self.core.jobs.lock().unwrap();
        let mut views = jobs
            .values()
            .map(|job| {
                let r = job.record.lock().unwrap();
                (r.created, r.view.clone())
            })
            .collect::<Vec<_>>();
        views.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
        views.into_iter().map(|(_, view)| view).collect()
    }
    pub fn take_events(&self) -> Vec<FileEvent> {
        self.core.events.lock().unwrap().drain(..).collect()
    }
    pub fn can_resume(&self, id: &str) -> bool {
        self.core.jobs.lock().unwrap().get(id).is_some_and(|job| {
            let r = job.record.lock().unwrap();
            (r.manifest.is_some() || (r.offer.is_some() && r.view.paste.is_some()))
                && !job.cancel.load(Ordering::Acquire)
                && matches!(r.view.phase, Phase::Paused | Phase::Failed)
        })
    }
    pub fn error(&self) -> Option<String> {
        self.core.error.lock().unwrap().clone()
    }
    /// Called by the existing clipboard tick. Native publication is acknowledged
    /// only after all files verify and the clipboard worker accepts the reservation.
    pub fn clipboard(&self, clipboard: &Clipboard) {
        for selection in clipboard.poll_files() {
            let _ = self.commands.try_send(Command::CopiedSelection(selection));
        }
        let events = self
            .core
            .clipboard
            .lock()
            .unwrap()
            .drain(..)
            .collect::<Vec<_>>();
        for event in events {
            match event {
                ClipboardEvent::Begin(id) => clipboard.begin_files(id),
                ClipboardEvent::Ready(id, paths) => clipboard.receive_files(id, paths),
                ClipboardEvent::Cancel(id) => clipboard.cancel_files(id),
                ClipboardEvent::Offer(offer) => clipboard.receive_offer(offer),
            }
        }
        for (id, ready) in clipboard.files_ready() {
            let _ = self.commands.try_send(Command::ClipboardReady(id, ready));
        }
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.core.stopping.store(true, Ordering::Release);
        lazy::stop(&self.core);
        self.stop.send_replace(true);
    }
}
struct IncomingReservation {
    core: Arc<Core>,
    id: String,
}
impl Drop for IncomingReservation {
    fn drop(&mut self) {
        self.core.reservations.lock().unwrap().remove(&self.id);
    }
}
fn reserve_incoming(core: &Arc<Core>, id: &str, total: u64) -> Result<Option<IncomingReservation>> {
    let mut pending = core.reservations.lock().unwrap();
    if pending.contains_key(id) {
        return Err(RetryLater("Waiting for another file transfer to finish").into());
    }
    let mut sizes = {
        let jobs = core.jobs.lock().unwrap();
        if jobs.get(id).is_some_and(|job| {
            matches!(
                job.record.lock().unwrap().view.phase,
                Phase::Completed | Phase::Cancelled
            )
        }) {
            return Ok(None);
        }
        jobs.iter()
            .filter_map(|(key, job)| {
                let r = job.record.lock().unwrap();
                (r.view.direction == Direction::Receiving
                    && key != id
                    && !matches!(r.view.phase, Phase::Completed | Phase::Cancelled))
                .then_some((key.clone(), r.view.total))
            })
            .collect::<BTreeMap<_, _>>()
    };
    for (key, size) in pending.iter() {
        sizes
            .entry(key.clone())
            .and_modify(|v| *v = (*v).max(*size))
            .or_insert(*size);
    }
    let used = sizes
        .values()
        .try_fold(0_u64, |sum, value| sum.checked_add(*value))
        .unwrap_or(u64::MAX);
    ensure!(
        used.checked_add(total)
            .is_some_and(|sum| sum <= protocol::MAX_TOTAL),
        "The incoming file transfers exceed the 64 GiB staging limit"
    );
    pending.insert(id.into(), total);
    Ok(Some(IncomingReservation {
        core: core.clone(),
        id: id.into(),
    }))
}

impl Core {
    fn receive_base(&self, record: &Record) -> PathBuf {
        if record.offer.is_some() && record.view.paste.is_some() {
            self.dir.join("file-transfer/native-cache")
        } else {
            self.downloads.clone()
        }
    }
    fn wake(&self) {
        self.ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
    fn allowed(&self, workspace: &str, peer: &str, origin: Origin) -> bool {
        let trust = self.trust.lock().unwrap();
        !self.stopping.load(Ordering::Acquire)
            && trust.workspace_id == workspace
            && trust.peers.contains_key(peer)
            && (origin != Origin::Clipboard || trust.clipboard_enabled)
    }
    fn check(&self, job: &Job) -> Result<()> {
        ensure!(
            !job.cancel.load(Ordering::Acquire),
            "The file transfer was cancelled"
        );
        let record = job.record.lock().unwrap();
        if record.user_paused || record.peer_paused {
            return Err(RetryLater("The file transfer is paused").into());
        }
        if self.stopping.load(Ordering::Acquire)
            || self.trust.lock().unwrap().workspace_id.is_empty()
        {
            return Err(RetryLater("The workspace membership is temporarily unavailable").into());
        }
        ensure!(
            self.allowed(
                &record.workspace,
                &record.view.peer,
                if record.offer.is_some() && record.view.paste.is_some() {
                    Origin::Send
                } else {
                    record.view.origin
                }
            ),
            "The computer is no longer authorized for this file transfer"
        );
        Ok(())
    }
    fn phase(&self, job: &Job, phase: Phase, error: Option<String>) {
        let mut r = job.record.lock().unwrap();
        let phase =
            if (r.user_paused || r.peer_paused) && !phase.terminal() && r.native_move.is_none() {
                Phase::Paused
            } else {
                phase
            };
        r.view.phase = phase;
        r.view.error = error;
        r.view.pause_reason = if phase == Phase::Paused {
            Some(if r.user_paused {
                PauseReason::User
            } else if r.peer_paused {
                PauseReason::Peer
            } else {
                PauseReason::Connection
            })
        } else {
            None
        };
        if let Some(paste) = &mut r.view.paste {
            paste.stage = match phase {
                Phase::Preparing => PasteStage::Preparing,
                Phase::Connecting | Phase::Transferring => PasteStage::Receiving,
                Phase::Paused => PasteStage::Paused,
                Phase::Verifying => PasteStage::Verifying,
                Phase::Completed => paste.stage,
                Phase::Cancelled => PasteStage::Cancelled,
                Phase::Failed => PasteStage::Failed,
            };
        }
        let id = r.view.id.clone();
        drop(r);
        if matches!(phase, Phase::Cancelled | Phase::Failed) {
            lazy::failed(
                self,
                &id,
                job.record
                    .lock()
                    .unwrap()
                    .view
                    .error
                    .clone()
                    .unwrap_or_else(|| "The file transfer was cancelled".into()),
                phase == Phase::Cancelled,
            );
        }
        self.wake();
    }
    fn progress(&self, job: &Job, bytes: u64) {
        job.record.lock().unwrap().view.bytes = bytes;
        let mut last = job.last_wake.lock().unwrap();
        if last.elapsed() >= Duration::from_millis(250) {
            *last = Instant::now();
            self.wake();
        }
    }
    fn clipboard_event(&self, event: ClipboardEvent) {
        let mut queue = self.clipboard.lock().unwrap();
        if queue.len() >= 64 {
            queue.pop_front();
        }
        queue.push_back(event);
        drop(queue);
        self.wake();
    }
    fn insert(&self, record: Record) -> Result<Arc<Job>> {
        self.prune();
        let mut jobs = self.jobs.lock().unwrap();
        ensure!(jobs.len() < MAX_JOBS, "The file transfer queue is full");
        ensure!(
            !jobs.contains_key(&record.view.id),
            "This file transfer is already registered"
        );
        let job = Arc::new(Job::new(record));
        let id = job.record.lock().unwrap().view.id.clone();
        jobs.insert(id, job.clone());
        drop(jobs);
        self.wake();
        Ok(job)
    }
    fn prune(&self) {
        let protected = lazy::protected_jobs(self);
        let removed = {
            let mut jobs = self.jobs.lock().unwrap();
            let mut terminal = jobs
                .iter()
                .filter_map(|(id, job)| {
                    let r = job.record.lock().unwrap();
                    ((matches!(r.view.phase, Phase::Completed | Phase::Cancelled)
                        || (r.view.phase == Phase::Failed
                            && r.manifest.is_none()
                            && r.cancel_notified))
                        && !r.view.clipboard_pending
                        && !protected.contains(id)
                        && !(r.native_move.is_some() && r.move_outcome.is_none())
                        && !(r.view.phase == Phase::Cancelled
                            && (!r.cancel_notified
                                || (r.view.direction == Direction::Receiving
                                    && !job.cleaned.load(Ordering::Acquire))))
                        && !job.running.load(Ordering::Acquire))
                    .then_some((r.created, id.clone()))
                })
                .collect::<Vec<_>>();
            terminal.sort();
            let keep = 32_usize;
            let count = terminal.len().saturating_sub(keep);
            terminal
                .into_iter()
                .take(count)
                .filter_map(|(_, id)| jobs.remove(&id).map(|job| (id, job)))
                .collect::<Vec<_>>()
        };
        for (id, job) in removed {
            let export = job.record.lock().unwrap().export.clone();
            self.garbage.lock().unwrap().push((id, export));
        }
    }
    fn collect_export(&self, id: &str) {
        let removable = {
            let jobs = self.jobs.lock().unwrap();
            !jobs.values().any(|job| {
                let r = job.record.lock().unwrap();
                r.export == id
                    && r.view.direction == Direction::Sending
                    && (!matches!(r.view.phase, Phase::Completed | Phase::Cancelled)
                        || (r.native_move.is_some() && r.move_outcome.is_none())
                        || job.running.load(Ordering::Acquire))
            })
        };
        let collect = removable && self.collected.lock().unwrap().insert(id.into());
        if collect && storage::Export::discard(&self.dir, id).is_err() {
            self.collected.lock().unwrap().remove(id);
        }
    }
    fn reap(&self) {
        self.prune();
        let garbage = self.garbage.lock().unwrap().drain(..).collect::<Vec<_>>();
        for (id, export) in garbage {
            self.collect_export(&export);
            if validate_job_id(&id).is_ok() {
                let _ = std::fs::remove_file(
                    self.dir
                        .join("file-transfer/jobs")
                        .join(format!("{id}.json")),
                );
            }
            let referenced = self
                .jobs
                .lock()
                .unwrap()
                .values()
                .any(|job| job.record.lock().unwrap().export == export);
            if !referenced {
                self.collected.lock().unwrap().remove(&export);
            }
        }
    }
    fn save(&self, job: &Job) -> Result<()> {
        let _guard = job.persist.lock().unwrap();
        let record = job.record.lock().unwrap().clone();
        validate_job_id(&record.view.id)?;
        config::write_atomic(
            &self
                .dir
                .join("file-transfer/jobs")
                .join(format!("{}.json", record.view.id)),
            &serde_json::to_string(&record)?,
        )
    }
    fn address(&self, peer: &str) -> Result<SocketAddr> {
        let mut address = self
            .trust
            .lock()
            .unwrap()
            .peers
            .get(peer)
            .context("The computer is no longer a trusted workspace member")?
            .address;
        address.set_port(
            address
                .port()
                .checked_add(1)
                .context("Invalid file transfer port")?,
        );
        Ok(address)
    }
    fn stamp(&self, author: &str) -> Stamp {
        let mut latest = self.latest.lock().unwrap();
        let stamp = Stamp {
            counter: millis().max(latest.as_ref().map_or(0, |s| s.counter.saturating_add(1))),
            author: author.into(),
        };
        *latest = Some(stamp.clone());
        stamp
    }
}
async fn save(core: Arc<Core>, job: Arc<Job>) -> Result<()> {
    tokio::task::spawn_blocking(move || core.save(&job)).await?
}
/// Cancel persists its marker before looking up a job; registration inserts the
/// job before checking again. Whichever side runs first therefore observes the
/// other, including an Offer that was already in flight before cancellation.
async fn reject_cancelled_registration(core: Arc<Core>, job: Arc<Job>) -> Result<bool> {
    let record = job.record.lock().unwrap().clone();
    let c = core.clone();
    let cancelled = tokio::task::spawn_blocking(move || {
        storage::was_cancelled(&c.dir, &record.workspace, &record.view.peer, &record.export)
    })
    .await??;
    if cancelled
        || job.cancel.load(Ordering::Acquire)
        || job.record.lock().unwrap().view.phase == Phase::Cancelled
    {
        if cancelled {
            job.record.lock().unwrap().cancel_notified = true;
        }
        cancel_job(core, job, false).await?;
        return Ok(true);
    }
    core.check(&job)?;
    Ok(false)
}
#[derive(Debug)]
struct RetryLater(&'static str);
impl std::fmt::Display for RetryLater {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for RetryLater {}
fn retryable(error: &anyhow::Error) -> bool {
    if error.downcast_ref::<RetryLater>().is_some() {
        return true;
    }
    !error.chain().any(|e| {
        e.is::<snow::Error>() || e.is::<serde_json::Error>() || e.is::<Box<bincode::ErrorKind>>()
    }) && error.chain().any(|e| {
        e.is::<tokio::time::error::Elapsed>()
            || e.downcast_ref::<std::io::Error>().is_some_and(|e| {
                matches!(
                    e.kind(),
                    std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::NotConnected
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::ConnectionRefused
                        | std::io::ErrorKind::NetworkUnreachable
                        | std::io::ErrorKind::HostUnreachable
                        | std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::WouldBlock
                )
            })
    })
}
async fn load(core: Arc<Core>) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let root = core.dir.join("file-transfer/jobs");
        if !root.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(root)?.take(MAX_JOBS * 8) {
            let entry = entry?;
            let metadata = entry.file_type()?;
            if !metadata.is_file() || entry.metadata()?.len() > 8 * 1024 * 1024 {
                continue;
            }
            let Ok(mut record) = serde_json::from_slice::<Record>(&std::fs::read(entry.path())?)
            else {
                continue;
            };
            if validate_job_id(&record.view.id).is_err()
                || entry.file_name() != std::ffi::OsString::from(format!("{}.json", record.view.id))
                || protocol::valid_id(&record.export).is_err()
                || record.view.id != job_id(&record.export, &record.view.peer)
                || record
                    .manifest
                    .as_ref()
                    .is_some_and(|m| m.id != record.export || m.validate().is_err())
            {
                continue;
            }
            record.view.clipboard_ready = false;
            record.view.clipboard_pending = false;
            record.view.clipboard_skipped = record.view.phase == Phase::Completed
                && record.view.origin == Origin::Clipboard
                && record.view.direction == Direction::Receiving;
            if !record.view.phase.terminal() {
                record.view.phase =
                    if record.view.direction == Direction::Sending && record.manifest.is_none() {
                        Phase::Failed
                    } else {
                        Phase::Paused
                    };
                record.view.error =
                    Some("The transfer will resume when the other computer reconnects".into());
                if record.view.origin == Origin::Clipboard && record.offer.is_none() {
                    // 0.7 queued clipboard exports were created merely by Copy.
                    // Upgrading must not turn that old metadata into new intent.
                    record.user_paused = true;
                    record.view.pause_reason = Some(PauseReason::User);
                    record.view.error = Some(
                        "Choose Resume to continue this saved transfer, or paste the files again"
                            .into(),
                    );
                }
            }
            if record.view.direction == Direction::Sending {
                record.view.paths.clear();
                if record.manifest.is_none() {
                    let _ = storage::Export::discard(&core.dir, &record.export);
                    record.view.error = Some(
                        "Source preparation was interrupted. Choose the files or folders again"
                            .into(),
                    );
                }
            }
            let _ = core.insert(record);
        }
        Ok::<_, anyhow::Error>(())
    })
    .await?
}
async fn run(
    core: Arc<Core>,
    listener: std::net::TcpListener,
    mut commands: mpsc::Receiver<Command>,
    mut trust: watch::Receiver<TrustSnapshot>,
    mut stopping: watch::Receiver<bool>,
) -> Result<()> {
    load(core.clone()).await?;
    let listener = TcpListener::from_std(listener)?;
    let incoming = Arc::new(Semaphore::new(8));
    let mut retry = tokio::time::interval(Duration::from_secs(1));
    retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _=stopping.changed()=>{break;},
            changed=trust.changed()=>{
                if changed.is_err(){break;}
                let value=trust.borrow().clone();
                *core.trust.lock().unwrap()=value.clone();
                if !value.clipboard_enabled {core.clipboard.lock().unwrap().clear();lazy::disable(&core);}
                let jobs=core.jobs.lock().unwrap().values().cloned().collect::<Vec<_>>();
                for job in jobs {let r=job.record.lock().unwrap();
                    let invalidate_clipboard=!value.clipboard_enabled&&r.view.phase==Phase::Completed&&r.view.clipboard_pending;
                    let discard_notice=!value.workspace_id.is_empty()&&r.view.phase==Phase::Cancelled&&!core.allowed(&r.workspace,&r.view.peer,Origin::Send);
                    let forbidden=!value.workspace_id.is_empty() && !r.view.phase.terminal() && !core.allowed(&r.workspace,&r.view.peer,if r.offer.is_some() && r.view.paste.is_some(){Origin::Send}else{r.view.origin});drop(r);
                    if invalidate_clipboard {let mut r=job.record.lock().unwrap();r.view.clipboard_pending=false;r.view.clipboard_skipped=true;drop(r);core.wake();let c=core.clone();let j=job.clone();tokio::spawn(async move{let _=save(c,j).await;});}
                    if discard_notice {job.record.lock().unwrap().cancel_notified=true;let c=core.clone();let j=job.clone();tokio::spawn(async move{let _=save(c,j).await;});}
                    if forbidden {job.record.lock().unwrap().cancel_notified=true;job.cancel.store(true,Ordering::Release);job.cancelled.notify_waiters();job.pause_changed.notify_all();let c=core.clone();tokio::spawn(async move{let _=cancel_job(c,job,false).await;});}
                }
            },
            Some(command)=commands.recv()=>{match command {
                Command::Send{id,target,paths}=>{let c=core.clone();tokio::spawn(async move{let _=prepare(c,id,vec![target],paths,Origin::Send).await;});},
                Command::Copied(paths)=>{let generation=lazy::begin_capture(&core);let c=core.clone();tokio::spawn(async move{let _=lazy::capture(c,Some(crate::group::clipboard_files::Selection{sequence:0,paths,operation:Operation::Copy}),generation).await;});},
                Command::CopiedSelection(selection)=>{let generation=lazy::begin_capture(&core);let c=core.clone();tokio::spawn(async move{let _=lazy::capture(c,selection,generation).await;});},
                Command::NativeBegin(fetch)=>{let c=core.clone();tokio::spawn(async move{lazy::request(c,fetch).await;});},
                Command::NativeResult(session,result)=>{let c=core.clone();tokio::spawn(async move{let _=lazy::native_result(c,session,result).await;});},
                Command::AcquireLease{offer,peer,lease}=>{let c=core.clone();tokio::spawn(async move{let _=lazy::lease_remote(c,peer,offer,lease,false).await;});},
                Command::ReleaseLease{offer,peer,lease}=>{let c=core.clone();tokio::spawn(async move{let _=lazy::lease_remote(c,peer,offer,lease,true).await;});},
                Command::Cancel(id)=>{let job=core.jobs.lock().unwrap().get(&id).cloned();if let Some(job)=job{let c=core.clone();tokio::spawn(async move{let _=cancel_job(c,job,true).await;});}},
                Command::Pause(id)=>{
                    let job=core.jobs.lock().unwrap().get(&id).cloned();
                    if let Some(job)=job {
                        lazy::set_user_pause(&core,&job,true);
                        core.phase(&job,Phase::Paused,None);
                        let c=core.clone();
                        tokio::spawn(async move {
                            let _=save(c.clone(),job.clone()).await;
                            let _=lazy::pause_peer(c,job).await;
                        });
                    }
                },
                Command::Resume(id)=>{
                    let job=core.jobs.lock().unwrap().get(&id).cloned();
                    if let Some(job)=job {
                        let record=job.record.lock().unwrap().clone();
                        if record.view.phase==Phase::Completed && record.offer.is_none()
                            && record.view.direction==Direction::Receiving {
                            let c=core.clone();tokio::spawn(async move{let _=publish_saved(c,job).await;});
                        } else if !matches!(record.view.phase,Phase::Completed|Phase::Cancelled) {
                            lazy::resume(core.clone(),job);
                        }
                    }
                },
                Command::ClipboardReady(id,ready)=>{lazy::published(&core,&id,ready);let job=core.jobs.lock().unwrap().get(&id).cloned();if let Some(job)=job{
                    {let mut r=job.record.lock().unwrap();r.view.clipboard_ready=ready;r.view.clipboard_pending=false;r.view.clipboard_skipped = !ready;}core.wake();let c=core.clone();tokio::spawn(async move{let _=save(c,job).await;});
                }},
            }},
            accepted=listener.accept()=>{if let Ok((stream,_))=accepted && let Ok(permit)=incoming.clone().try_acquire_owned(){let c=core.clone();tokio::spawn(async move{let _permit=permit;let _=incoming_connection(c,stream).await;});}},
            _=retry.tick()=>{
                lazy::maintain(&core);
                let jobs=core.jobs.lock().unwrap().values().cloned().collect::<Vec<_>>();
                for job in jobs {
                    if job.record.lock().unwrap().view.phase==Phase::Cancelled && !job.running.load(Ordering::Acquire) && !job.cleaned.load(Ordering::Acquire)&&!job.cleaning.swap(true,Ordering::AcqRel) {
                        let c=core.clone();let j=job.clone();tokio::spawn(async move{let _=cancel_job(c,j,false).await;});
                    }
                    let (eligible,peer)={let r=job.record.lock().unwrap();(!r.user_paused&&!r.peer_paused&&r.view.direction==Direction::Sending&&r.manifest.is_some()&&matches!(r.view.phase,Phase::Paused|Phase::Connecting),r.view.peer.clone())};
                    let online=core.trust.lock().unwrap().peers.get(&peer).is_some_and(|p|p.online);
                    let pending_move={let r=job.record.lock().unwrap();r.view.direction==Direction::Receiving&&r.native_move.is_some()&&r.move_outcome.is_none()};
                    if pending_move && online && Instant::now()>=*job.retry.lock().unwrap() {
                        lazy::retry_move(core.clone(),job.clone());
                        continue;
                    }
                    let retry_demand={let r=job.record.lock().unwrap();r.view.direction==Direction::Receiving&&r.offer.is_some()
                        && !r.user_paused && !r.peer_paused && r.view.phase==Phase::Paused && r.native_move.is_none()};
                    if retry_demand && online && !job.control_running.load(Ordering::Acquire)
                        && Instant::now()>=*job.retry.lock().unwrap() {
                        if let Some(fetch)=lazy::fetch(&core,&job.record.lock().unwrap().view.id) {
                            let c=core.clone();tokio::spawn(async move{lazy::request(c,fetch).await;});
                        } else {queue_control(core.clone(),job.clone(),false);}
                    }
                    let retry_cancel={let r=job.record.lock().unwrap();(r.view.phase==Phase::Cancelled||(r.view.phase==Phase::Failed&&r.manifest.is_none()&&r.view.origin==Origin::Clipboard))&&!r.cancel_notified
                        && core.allowed(&r.workspace,&r.view.peer,Origin::Send)};
                    if retry_cancel&&online&&Instant::now()>=*job.retry.lock().unwrap(){queue_control(core.clone(),job.clone(),true);}
                    if eligible && online && !job.cancel.load(Ordering::Acquire) && Instant::now()>=*job.retry.lock().unwrap()
                        && !job.running.swap(true,Ordering::AcqRel) {let c=core.clone();tokio::spawn(async move{send_job(c,job).await;});}
                }
                let c=core.clone();tokio::task::spawn_blocking(move||{
                    let ids={let jobs=c.jobs.lock().unwrap();jobs.values().filter_map(|job|{let r=job.record.lock().unwrap();
                        (r.view.direction==Direction::Sending&&matches!(r.view.phase,Phase::Completed|Phase::Cancelled)).then_some(r.export.clone())}).collect::<BTreeSet<_>>()};
                    for id in ids {c.collect_export(&id);}c.reap();
                });
            },
        }
    }
    core.stopping.store(true, Ordering::Release);
    lazy::stop(&core);
    for job in core.jobs.lock().unwrap().values() {
        job.cancelled.notify_waiters();
        job.pause_changed.notify_all();
    }
    Ok(())
}

async fn prepare(
    core: Arc<Core>,
    id: String,
    targets: Vec<String>,
    paths: Vec<PathBuf>,
    origin: Origin,
) -> Result<()> {
    let result = prepare_inner(core.clone(), id.clone(), targets, paths, origin).await;
    if let Err(error) = &result {
        let jobs = core
            .jobs
            .lock()
            .unwrap()
            .values()
            .filter(|job| job.record.lock().unwrap().export == id)
            .cloned()
            .collect::<Vec<_>>();
        for job in jobs {
            if !job.record.lock().unwrap().view.phase.terminal() {
                if origin == Origin::Send {
                    job.record.lock().unwrap().cancel_notified = true;
                }
                core.phase(&job, Phase::Failed, Some(format!("{error:#}")));
                let _ = save(core.clone(), job.clone()).await;
                if origin == Origin::Clipboard {
                    queue_control(core.clone(), job, true);
                }
            }
        }
    }
    result
}
async fn prepare_inner(
    core: Arc<Core>,
    id: String,
    targets: Vec<String>,
    paths: Vec<PathBuf>,
    origin: Origin,
) -> Result<()> {
    let trust = core.trust.lock().unwrap().clone();
    let stamp = (origin == Origin::Clipboard).then(|| core.stamp(&trust.own));
    let mut jobs = Vec::new();
    for target in targets {
        if !core.allowed(&trust.workspace_id, &target, origin) {
            continue;
        }
        let record = Record {
            workspace: trust.workspace_id.clone(),
            export: id.clone(),
            stamp: stamp.clone(),
            created: millis(),
            manifest: None,
            cancel_notified: false,
            user_paused: false,
            peer_paused: false,
            offer: None,
            operation: Operation::Copy,
            native_move: None,
            move_binding: None,
            move_outcome: None,
            move_grant: None,
            view: Transfer {
                id: job_id(&id, &target),
                peer: target,
                direction: Direction::Sending,
                origin,
                name: paths
                    .first()
                    .and_then(|p| p.file_name())
                    .map(|v| v.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Files and folders".into()),
                phase: Phase::Preparing,
                bytes: 0,
                total: 0,
                files: 0,
                error: None,
                paths: vec![],
                clipboard_ready: false,
                clipboard_pending: false,
                clipboard_skipped: false,
                paste: None,
                pause_reason: None,
                current_name: None,
                recovery_paths: vec![],
            },
        };
        if let Ok(job) = core.insert(record) {
            if origin == Origin::Send {
                let id = job.record.lock().unwrap().view.id.clone();
                core.events
                    .lock()
                    .unwrap()
                    .push_back(FileEvent::SendStarted { id });
                core.wake();
            }
            jobs.push(job);
        }
    }
    if jobs.is_empty() {
        return Ok(());
    }
    let _preparing = jobs
        .iter()
        .map(|job| {
            job.running.store(true, Ordering::Release);
            Running(job.clone())
        })
        .collect::<Vec<_>>();
    for job in &jobs {
        save(core.clone(), job.clone()).await?;
    }
    if origin == Origin::Clipboard {
        let mut announcing = tokio::task::JoinSet::new();
        for job in &jobs {
            let peer = job.record.lock().unwrap().view.peer.clone();
            if trust.peers.get(&peer).is_some_and(|peer| peer.online) {
                let c = core.clone();
                let j = job.clone();
                announcing.spawn(async move {
                    let _ = announce(c, j).await;
                });
            }
        }
        while announcing.join_next().await.is_some() {}
    }
    let _prepare = core.prepare.clone().acquire_owned().await?;
    let used = {
        let all = core.jobs.lock().unwrap();
        let mut seen = BTreeSet::new();
        all.values()
            .filter_map(|job| {
                let r = job.record.lock().unwrap();
                (r.view.direction == Direction::Sending
                    && r.manifest.is_some()
                    && !matches!(r.view.phase, Phase::Completed | Phase::Cancelled)
                    && seen.insert(r.export.clone()))
                .then_some(r.view.total)
            })
            .sum::<u64>()
    };
    let budget = protocol::MAX_TOTAL.saturating_sub(used);
    let c = core.clone();
    let preparation_jobs = jobs.clone();
    let export_id = id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let cancel = AtomicBool::new(
            preparation_jobs
                .iter()
                .all(|job| job.cancel.load(Ordering::Acquire)),
        );
        storage::Export::prepare_with_limit(&c.dir, &export_id, &paths, budget, &cancel, |bytes| {
            cancel.store(
                preparation_jobs
                    .iter()
                    .all(|job| job.cancel.load(Ordering::Acquire))
                    || c.stopping.load(Ordering::Acquire),
                Ordering::Release,
            );
            for job in &preparation_jobs {
                c.progress(job, bytes);
            }
        })
    })
    .await?;
    match result {
        Ok(export) => {
            let manifest = export.manifest.clone();
            for job in jobs {
                if job.cancel.load(Ordering::Acquire) {
                    continue;
                }
                {
                    let mut r = job.record.lock().unwrap();
                    r.view.name = manifest.name.clone();
                    r.view.total = manifest.total;
                    r.view.files = manifest
                        .entries
                        .iter()
                        .filter(|e| matches!(e.kind, Kind::File { .. }))
                        .count() as u32;
                    r.view.bytes = 0;
                    r.manifest = Some(manifest.clone());
                }
                core.phase(&job, Phase::Paused, None);
                save(core.clone(), job).await?;
            }
        }
        Err(error) => {
            for job in jobs {
                if origin == Origin::Send {
                    job.record.lock().unwrap().cancel_notified = true;
                }
                let phase = if job.cancel.load(Ordering::Acquire) {
                    Phase::Cancelled
                } else {
                    Phase::Failed
                };
                core.phase(&job, phase, Some(format!("{error:#}")));
                let _ = save(core.clone(), job.clone()).await;
                if origin == Origin::Clipboard {
                    queue_control(core.clone(), job, true);
                }
            }
        }
    }
    Ok(())
}
async fn announce(core: Arc<Core>, job: Arc<Job>) -> Result<()> {
    let _slot = core.slots.clone().acquire_owned().await?;
    core.check(&job)?;
    let record = job.record.lock().unwrap().clone();
    let address = core.address(&record.view.peer)?;
    let mut pipe = transport::connect(
        address,
        &core.identity,
        &record.view.peer,
        &record.workspace,
    )
    .await?;
    pipe.send(&Packet::Announce {
        id: record.export,
        name: record.view.name,
        stamp: record.stamp.context("Invalid file clipboard author")?,
    })
    .await?;
    ensure!(
        matches!(pipe.receive().await?, Packet::Queued),
        "The other computer could not prepare its file clipboard"
    );
    Ok(())
}

async fn receive_packet(
    pipe: &mut transport::Connection,
    core: &Core,
    job: &Job,
) -> Result<Packet> {
    loop {
        core.check(job)?;
        let packet = tokio::select! {biased;
            _=job.cancelled.notified()=>{core.check(job)?;return Err(RetryLater("The file transfer was interrupted").into());},
            result=pipe.receive()=>result?,
        };
        if matches!(packet, Packet::Working) {
            continue;
        }
        if matches!(packet, Packet::Busy) {
            return Err(RetryLater("Waiting for another file transfer to finish").into());
        }
        return Ok(packet);
    }
}
/// Verification can take longer than one socket deadline. Heartbeats are sent
/// only while real storage work runs, and its token is cancelled before reuse.
async fn work<T: Send + 'static>(
    core: &Arc<Core>,
    job: &Arc<Job>,
    pipe: &mut transport::Connection,
    operation: impl FnOnce(Arc<AtomicBool>) -> Result<T> + Send + 'static,
) -> Result<T> {
    let cancelled = Arc::new(AtomicBool::new(false));
    struct AbortOnDrop(Arc<AtomicBool>);
    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let _abort = AbortOnDrop(cancelled.clone());
    let token = cancelled.clone();
    let mut task = tokio::task::spawn_blocking(move || operation(token));
    let mut heartbeat = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(2),
        Duration::from_secs(2),
    );
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {biased;
            result=&mut task=>return result?,
            _=job.cancelled.notified()=>{
                if let Err(error)=core.check(job){cancelled.store(true,Ordering::Release);let _=(&mut task).await;return Err(error);}
            },
            _=heartbeat.tick()=>{
                let result=match core.check(job) {Ok(())=>pipe.send(&Packet::Working).await,Err(error)=>Err(error)};
                if let Err(error)=result {cancelled.store(true,Ordering::Release);let _=(&mut task).await;return Err(error);}
            },
        }
    }
}
async fn send_job(core: Arc<Core>, job: Arc<Job>) {
    let _running = Running(job.clone());
    let Ok(_slot) = core.slots.clone().acquire_owned().await else {
        return;
    };
    let result = send_inner(core.clone(), job.clone()).await;
    if let Err(error) = result {
        if job.cancel.load(Ordering::Acquire) {
            let _ = cancel_job(core.clone(), job.clone(), false).await;
        } else {
            let phase = if retryable(&error) {
                Phase::Paused
            } else {
                Phase::Failed
            };
            core.phase(&job, phase, Some(format!("{error:#}")));
            *job.retry.lock().unwrap() = Instant::now() + RETRY;
            let _ = save(core.clone(), job.clone()).await;
        }
    }
}
async fn send_inner(core: Arc<Core>, job: Arc<Job>) -> Result<()> {
    core.check(&job)?;
    let r = job.record.lock().unwrap().clone();
    let address = core.address(&r.view.peer)?;
    core.phase(&job, Phase::Connecting, None);
    let c = core.clone();
    let id = r.export.clone();
    let export =
        Arc::new(tokio::task::spawn_blocking(move || storage::Export::load(&c.dir, &id)).await??);
    ensure!(
        r.manifest.as_ref() == Some(&export.manifest),
        "The saved source export no longer matches this transfer"
    );
    core.check(&job)?;
    let mut pipe = transport::connect(address, &core.identity, &r.view.peer, &r.workspace).await?;
    pipe.send(&Packet::Offer {
        manifest: export.manifest.clone(),
        origin: r.view.origin,
        stamp: r.stamp.clone(),
    })
    .await?;
    match receive_packet(&mut pipe, &core, &job).await? {
        Packet::Accepted => {}
        Packet::Complete => {
            sent(&core, &job);
            core.progress(&job, r.view.total);
            save(core.clone(), job.clone()).await?;
            return Ok(());
        }
        Packet::Cancel => {
            job.record.lock().unwrap().cancel_notified = true;
            job.cancel.store(true, Ordering::Release);
            anyhow::bail!("The file transfer was cancelled");
        }
        Packet::Error(error) => {
            anyhow::bail!("{}", crate::workspace::management::status_text(error, 512))
        }
        _ => anyhow::bail!("The receiver did not accept this file transfer"),
    }
    core.phase(&job, Phase::Transferring, None);
    let mut complete = 0_u64;
    for (index, entry) in export.manifest.entries.iter().enumerate() {
        let Kind::File { size, .. } = &entry.kind else {
            continue;
        };
        job.record.lock().unwrap().view.current_name = entry.path.last().cloned();
        let reply = receive_packet(&mut pipe, &core, &job).await?;
        let (mut offset, prefix) = match reply {
            Packet::Ready {
                index: i,
                offset,
                prefix,
            } if i == index as u32 && offset <= *size && protocol::valid_hash(&prefix) => {
                (offset, prefix)
            }
            Packet::Complete => {
                core.phase(&job, Phase::Completed, None);
                core.progress(&job, r.view.total);
                save(core.clone(), job.clone()).await?;
                return Ok(());
            }
            Packet::Cancel => {
                job.record.lock().unwrap().cancel_notified = true;
                job.cancel.store(true, Ordering::Release);
                anyhow::bail!("The file transfer was cancelled");
            }
            Packet::Error(error) => {
                anyhow::bail!("{}", crate::workspace::management::status_text(error, 512))
            }
            _ => anyhow::bail!("Invalid file transfer resume position"),
        };
        let e = export.clone();
        let expected = work(&core, &job, &mut pipe, move |cancel| {
            e.prefix_with_cancel(index as u32, offset, &cancel)
        })
        .await?;
        ensure!(
            expected == prefix,
            "The receiver's saved file prefix failed verification"
        );
        core.progress(&job, complete + offset);
        while offset < *size {
            core.check(&job)?;
            let e = export.clone();
            let position = offset;
            let bytes = work(&core, &job, &mut pipe, move |_| {
                e.read(index as u32, position, CHUNK)
            })
            .await?;
            ensure!(
                !bytes.is_empty() && bytes.len() <= CHUNK,
                "The saved source export is incomplete"
            );
            let next = offset + bytes.len() as u64;
            let digest = protocol::hash(&bytes);
            pipe.send(&Packet::Chunk {
                index: index as u32,
                offset,
                bytes,
                digest,
            })
            .await?;
            match receive_packet(&mut pipe, &core, &job).await? {
                Packet::Ack {
                    index: i,
                    offset: n,
                } if i == index as u32 && n == next => {}
                Packet::Cancel => {
                    job.record.lock().unwrap().cancel_notified = true;
                    job.cancel.store(true, Ordering::Release);
                    anyhow::bail!("The file transfer was cancelled");
                }
                Packet::Error(error) => {
                    anyhow::bail!("{}", crate::workspace::management::status_text(error, 512))
                }
                _ => anyhow::bail!("Invalid file transfer acknowledgement"),
            }
            offset = next;
            core.progress(&job, complete + offset);
        }
        complete += *size;
    }
    core.phase(&job, Phase::Verifying, None);
    pipe.send(&Packet::Finish).await?;
    match receive_packet(&mut pipe, &core, &job).await? {
        Packet::Complete => {}
        Packet::Error(error) => {
            anyhow::bail!("{}", crate::workspace::management::status_text(error, 512))
        }
        _ => anyhow::bail!("The receiver did not confirm the completed files"),
    }
    sent(&core, &job);
    core.progress(&job, export.manifest.total);
    save(core.clone(), job).await?;
    Ok(())
}

fn sent(core: &Core, job: &Job) {
    let mut record = job.record.lock().unwrap();
    record.view.current_name = None;
    record.view.error = None;
    record.view.phase = if record.offer.is_some() && record.operation == Operation::Move {
        Phase::Verifying
    } else {
        Phase::Completed
    };
    if let Some(paste) = &mut record.view.paste {
        paste.stage = PasteStage::Delivered;
    }
    drop(record);
    core.wake();
}

async fn incoming_connection(core: Arc<Core>, stream: TcpStream) -> Result<()> {
    let trust = core.trust.lock().unwrap().clone();
    ensure!(
        !trust.workspace_id.is_empty(),
        "The workspace membership is not ready"
    );
    let peers = trust.peers.keys().cloned().collect();
    let (peer, mut pipe) =
        transport::accept(stream, &core.identity, &trust.workspace_id, &peers).await?;
    let first = pipe.receive().await?;
    match first {
        Packet::ClipboardOffer {
            id,
            description,
            stamp,
            operation,
        } => lazy::incoming_offer(core, peer, id, description, stamp, operation, &mut pipe).await,
        Packet::RetireOffer { id } => {
            protocol::valid_id(&id)?;
            lazy::retired_remote(&core, &peer, &id);
            pipe.send(&Packet::Queued).await
        }
        Packet::LeaseOffer { offer, lease } => {
            lazy::local_lease(&core, &peer, &offer, &lease, false)?;
            pipe.send(&Packet::Queued).await
        }
        Packet::ReleaseOfferLease { offer, lease } => {
            lazy::local_lease(&core, &peer, &offer, &lease, true)?;
            pipe.send(&Packet::Queued).await
        }
        Packet::PasteDemand {
            offer,
            id,
            operation,
            lease,
        } => {
            if let Err(error) =
                lazy::demand(core, peer, offer, id, operation, lease, &mut pipe).await
            {
                pipe.send(&Packet::Error(
                    format!("{error:#}").chars().take(2048).collect(),
                ))
                .await?;
            }
            Ok(())
        }
        Packet::PauseTransfer { id } => {
            protocol::valid_id(&id)?;
            let job = core.jobs.lock().unwrap().get(&job_id(&id, &peer)).cloned();
            if let Some(job) = job {
                lazy::set_peer_pause(&core, &job, true);
                core.phase(&job, Phase::Paused, None);
                save(core.clone(), job).await?;
            }
            pipe.send(&Packet::Queued).await
        }
        Packet::PasteFailed {
            id,
            error,
            retryable,
        } => {
            protocol::valid_id(&id)?;
            let job = core.jobs.lock().unwrap().get(&job_id(&id, &peer)).cloned();
            if let Some(job) = job {
                ensure!(
                    job.record.lock().unwrap().view.direction == Direction::Receiving,
                    "Only the sender can report preparation failure"
                );
                core.phase(
                    &job,
                    if retryable {
                        Phase::Paused
                    } else {
                        Phase::Failed
                    },
                    Some(error.chars().take(2048).collect()),
                );
                save(core.clone(), job).await?;
            }
            pipe.send(&Packet::Queued).await
        }
        Packet::NativeMoveCommitted {
            offer,
            id,
            manifest,
            paste,
            consumer,
            entries,
        } => {
            if let Err(error) = lazy::source_move(
                core, peer, offer, id, manifest, paste, consumer, entries, &mut pipe,
            )
            .await
            {
                pipe.send(&Packet::Error(
                    format!("{error:#}").chars().take(2048).collect(),
                ))
                .await?;
            }
            Ok(())
        }
        Packet::Announce { .. } => {
            // Wire v2 accepts metadata offers only. A v1-style announcement
            // cannot create transfer intent, reserve a clipboard or read files.
            pipe.send(&Packet::Error(
                "Update Desk Switch on both computers to paste shared files".into(),
            ))
            .await
        }
        control_packet @ (Packet::Resume { .. } | Packet::CancelTransfer { .. }) => {
            let (id, cancel) = match control_packet {
                Packet::Resume { id } => (id, false),
                Packet::CancelTransfer { id } => (id, true),
                _ => unreachable!(),
            };
            protocol::valid_id(&id)?;
            ensure!(
                core.allowed(&trust.workspace_id, &peer, Origin::Send),
                "The computer is no longer authorized for this file transfer"
            );
            if cancel {
                let c = core.clone();
                let workspace = trust.workspace_id.clone();
                let source = peer.clone();
                let export = id.clone();
                tokio::task::spawn_blocking(move || {
                    storage::remember_cancel(&c.dir, &workspace, &source, &export)
                })
                .await??;
                let job = core.jobs.lock().unwrap().get(&job_id(&id, &peer)).cloned();
                if let Some(job) = job {
                    job.record.lock().unwrap().cancel_notified = true;
                    let _ = cancel_job(core.clone(), job, false).await;
                }
                pipe.send(&Packet::Queued).await?;
                return Ok(());
            }
            let job = core
                .jobs
                .lock()
                .unwrap()
                .get(&job_id(&id, &peer))
                .cloned()
                .context("The requested file export is unavailable")?;
            {
                let r = job.record.lock().unwrap();
                ensure!(
                    r.workspace == trust.workspace_id && r.view.peer == peer,
                    "The file transfer belongs to another workspace"
                );
                ensure!(
                    core.allowed(
                        &r.workspace,
                        &r.view.peer,
                        if r.offer.is_some() || cancel {
                            Origin::Send
                        } else {
                            r.view.origin
                        }
                    ),
                    "The computer is no longer authorized for this file transfer"
                );
            }
            let native = {
                let r = job.record.lock().unwrap();
                ensure!(
                    (r.view.direction == Direction::Sending || r.offer.is_some())
                        && (r.manifest.is_some() || r.offer.is_some())
                        && !matches!(r.view.phase, Phase::Cancelled | Phase::Completed),
                    "The requested file export cannot be resumed"
                );
                r.offer.is_some()
            };
            if native {
                lazy::set_user_pause(&core, &job, false);
                lazy::resume_after_peer(core.clone(), job.clone());
            } else {
                job.cancel.store(false, Ordering::Release);
                *job.retry.lock().unwrap() = Instant::now();
                core.phase(&job, Phase::Paused, None);
            }
            pipe.send(&Packet::Queued).await?;
            Ok(())
        }
        Packet::Offer {
            manifest,
            origin,
            stamp,
        } => {
            manifest.validate()?;
            let id = job_id(&manifest.id, &peer);
            let existing = core.jobs.lock().unwrap().get(&id).cloned();
            let native_demand = existing.as_ref().is_some_and(|job| {
                let r = job.record.lock().unwrap();
                r.offer.is_some()
                    && r.view.paste.is_some()
                    && r.view.direction == Direction::Receiving
            });
            ensure!(
                core.allowed(
                    &trust.workspace_id,
                    &peer,
                    if native_demand { Origin::Send } else { origin }
                ),
                "Shared file clipboard is disabled or this computer is no longer trusted"
            );
            ensure!(
                origin != Origin::Clipboard || existing.is_some(),
                "Paste the copied files before receiving their contents"
            );
            ensure!(
                (origin == Origin::Clipboard) == stamp.is_some(),
                "Invalid file clipboard offer"
            );
            if let Some(stamp) = &stamp {
                ensure!(
                    stamp.author == peer && stamp.counter > 0,
                    "Invalid file clipboard author"
                );
            }
            let c = core.clone();
            let workspace = trust.workspace_id.clone();
            let source = peer.clone();
            let export = manifest.id.clone();
            if tokio::task::spawn_blocking(move || {
                storage::was_cancelled(&c.dir, &workspace, &source, &export)
            })
            .await??
            {
                pipe.send(&Packet::Cancel).await?;
                return Ok(());
            }
            let _reservation = match reserve_incoming(&core, &id, manifest.total) {
                Ok(reservation) => reservation,
                Err(error) => {
                    let reply = if error.downcast_ref::<RetryLater>().is_some() {
                        Packet::Busy
                    } else {
                        Packet::Error(crate::workspace::management::status_text(
                            error.to_string(),
                            512,
                        ))
                    };
                    pipe.send(&reply).await?;
                    return Ok(());
                }
            };
            let job = if let Some(job) = existing {
                lazy::expected_manifest(&core, &job, &manifest)?;
                let phase = {
                    let mut r = job.record.lock().unwrap();
                    ensure!(
                        r.workspace == trust.workspace_id
                            && r.view.direction == Direction::Receiving
                            && (r.manifest.as_ref() == Some(&manifest)
                                || (r.manifest.is_none() && origin == Origin::Clipboard))
                            && r.view.origin == origin
                            && r.stamp == stamp,
                        "This transfer identity was reused with different files"
                    );
                    r.manifest = Some(manifest.clone());
                    r.view.name = manifest.name.clone();
                    r.view.total = manifest.total;
                    r.view.files = manifest
                        .entries
                        .iter()
                        .filter(|e| matches!(e.kind, Kind::File { .. }))
                        .count() as u32;
                    r.view.phase
                };
                if phase == Phase::Completed {
                    pipe.send(&Packet::Complete).await?;
                    return Ok(());
                }
                if phase == Phase::Cancelled {
                    pipe.send(&Packet::Cancel).await?;
                    job.record.lock().unwrap().cancel_notified = true;
                    save(core.clone(), job.clone()).await?;
                    return Ok(());
                }
                job
            } else {
                core.insert(Record {
                    workspace: trust.workspace_id.clone(),
                    export: manifest.id.clone(),
                    stamp: stamp.clone(),
                    created: millis(),
                    manifest: Some(manifest.clone()),
                    cancel_notified: false,
                    user_paused: false,
                    peer_paused: false,
                    offer: None,
                    operation: Operation::Copy,
                    native_move: None,
                    move_binding: None,
                    move_outcome: None,
                    move_grant: None,
                    view: Transfer {
                        id: id.clone(),
                        peer: peer.clone(),
                        direction: Direction::Receiving,
                        origin,
                        name: manifest.name.clone(),
                        phase: Phase::Connecting,
                        bytes: 0,
                        total: manifest.total,
                        files: manifest
                            .entries
                            .iter()
                            .filter(|e| matches!(e.kind, Kind::File { .. }))
                            .count() as u32,
                        error: None,
                        paths: vec![],
                        clipboard_ready: false,
                        clipboard_pending: false,
                        clipboard_skipped: origin == Origin::Clipboard,
                        paste: None,
                        pause_reason: None,
                        current_name: None,
                        recovery_paths: vec![],
                    },
                })?
            };
            if reject_cancelled_registration(core.clone(), job.clone()).await? {
                pipe.send(&Packet::Cancel).await?;
                return Ok(());
            }
            ensure!(
                !job.running.swap(true, Ordering::AcqRel),
                "This file transfer is already active"
            );
            let _running = Running(job.clone());
            let Ok(_slot) = core.slots.clone().try_acquire_owned() else {
                core.phase(
                    &job,
                    Phase::Paused,
                    Some("Waiting for another file transfer to finish".into()),
                );
                pipe.send(&Packet::Busy).await?;
                return Ok(());
            };
            let result = async {
                save(core.clone(), job.clone()).await?;
                receive_inner(core.clone(), job.clone(), &mut pipe, manifest).await
            }
            .await;
            if let Err(error) = &result {
                if job.record.lock().unwrap().view.phase == Phase::Completed {
                    return result;
                }
                let cancelled = job.cancel.load(Ordering::Acquire);
                let phase = if cancelled {
                    Phase::Cancelled
                } else if retryable(error) {
                    Phase::Paused
                } else {
                    Phase::Failed
                };
                core.phase(&job, phase, Some(format!("{error:#}")));
                let reply = if cancelled {
                    Packet::Cancel
                } else {
                    Packet::Error(crate::workspace::management::status_text(
                        format!("{error:#}"),
                        512,
                    ))
                };
                let acknowledged = pipe.send(&reply).await.is_ok();
                if cancelled && acknowledged {
                    job.record.lock().unwrap().cancel_notified = true;
                }
                if cancelled {
                    let _ = cancel_job(core.clone(), job.clone(), false).await;
                }
                let _ = save(core.clone(), job).await;
            }
            result
        }
        _ => anyhow::bail!("The file transfer did not begin with an authorized export"),
    }
}
async fn receive_inner(
    core: Arc<Core>,
    job: Arc<Job>,
    pipe: &mut transport::Connection,
    manifest: Manifest,
) -> Result<()> {
    core.check(&job)?;
    let r = job.record.lock().unwrap().clone();
    let m = manifest.clone();
    let peer = r.view.peer.clone();
    let workspace = r.workspace.clone();
    let base = core.receive_base(&r);
    let receive = work(&core, &job, pipe, move |_| {
        storage::Receive::open(&base, m, &peer, &workspace)
    })
    .await?;
    let receive = Arc::new(Mutex::new(receive));
    if receive.lock().unwrap().is_complete() {
        let receiver = receive.clone();
        let paths = work(&core, &job, pipe, move |cancel| {
            receiver.lock().unwrap().finish_with_cancel(&cancel)
        })
        .await?;
        return complete_receive(core, job, pipe, paths, manifest.total, receive).await;
    }
    pipe.send(&Packet::Accepted).await?;
    core.phase(&job, Phase::Transferring, None);
    let mut complete = 0_u64;
    for (index, entry) in manifest.entries.iter().enumerate() {
        let Kind::File { size, .. } = &entry.kind else {
            continue;
        };
        job.record.lock().unwrap().view.current_name = entry.path.last().cloned();
        let receiver = receive.clone();
        let (mut offset, prefix) = work(&core, &job, pipe, move |cancel| {
            receiver
                .lock()
                .unwrap()
                .position_with_cancel(index as u32, &cancel)
        })
        .await?;
        ensure!(offset <= *size, "Invalid saved file transfer offset");
        core.progress(&job, complete + offset);
        pipe.send(&Packet::Ready {
            index: index as u32,
            offset,
            prefix,
        })
        .await?;
        while offset < *size {
            match receive_packet(pipe, &core, &job).await? {
                Packet::Chunk {
                    index: i,
                    offset: position,
                    bytes,
                    digest,
                } if i == index as u32
                    && position == offset
                    && !bytes.is_empty()
                    && bytes.len() <= CHUNK
                    && offset + bytes.len() as u64 <= *size =>
                {
                    let receiver = receive.clone();
                    let next = work(&core, &job, pipe, move |_| {
                        receiver.lock().unwrap().write(i, position, &bytes, &digest)
                    })
                    .await?;
                    offset = next;
                    pipe.send(&Packet::Ack { index: i, offset }).await?;
                    core.progress(&job, complete + offset);
                }
                Packet::Cancel => {
                    job.record.lock().unwrap().cancel_notified = true;
                    job.cancel.store(true, Ordering::Release);
                    anyhow::bail!("The file transfer was cancelled");
                }
                _ => anyhow::bail!("Invalid file transfer chunk"),
            }
        }
        complete += *size;
    }
    ensure!(
        matches!(receive_packet(pipe, &core, &job).await?, Packet::Finish),
        "The file transfer did not finish cleanly"
    );
    core.phase(&job, Phase::Verifying, None);
    core.check(&job)?;
    let receiver = receive.clone();
    let paths = work(&core, &job, pipe, move |cancel| {
        receiver.lock().unwrap().finish_with_cancel(&cancel)
    })
    .await?;
    complete_receive(core, job, pipe, paths, manifest.total, receive).await
}
async fn complete_receive(
    core: Arc<Core>,
    job: Arc<Job>,
    pipe: &mut transport::Connection,
    paths: Vec<PathBuf>,
    total: u64,
    receive: Arc<Mutex<storage::Receive>>,
) -> Result<()> {
    let r = job.record.lock().unwrap().clone();
    let native = r.offer.is_some() && r.view.paste.is_some();
    let publish = !native
        && r.view.origin == Origin::Clipboard
        && !job.cancel.load(Ordering::Acquire)
        && job.reserved.load(Ordering::Acquire)
        && !r.view.clipboard_skipped
        && core.allowed(&r.workspace, &r.view.peer, r.view.origin)
        && r.stamp.as_ref() == core.latest.lock().unwrap().as_ref();
    {
        let mut record = job.record.lock().unwrap();
        record.view.paths = if native { vec![] } else { paths.clone() };
        record.view.bytes = total;
        record.view.current_name = None;
        record.view.clipboard_pending = publish;
        record.view.clipboard_skipped = r.view.origin == Origin::Clipboard && !publish;
    }
    if native {
        {
            let mut record = job.record.lock().unwrap();
            record.view.phase = if record
                .view
                .paste
                .as_ref()
                .is_some_and(|paste| paste.reports_completion)
            {
                Phase::Verifying
            } else {
                Phase::Completed
            };
            record.view.clipboard_skipped = false;
            if let Some(paste) = &mut record.view.paste {
                paste.stage = PasteStage::Delivered;
            }
        }
        lazy::delivered(&core, &r.view.id, receive);
        core.wake();
    } else {
        core.phase(&job, Phase::Completed, None);
    }
    save(core.clone(), job.clone()).await?;
    if publish && !job.cancel.load(Ordering::Acquire) {
        core.clipboard_event(ClipboardEvent::Ready(r.view.id, paths));
    } else if publish {
        {
            let mut record = job.record.lock().unwrap();
            record.view.clipboard_pending = false;
            record.view.clipboard_skipped = true;
        }
        save(core.clone(), job).await?;
    }
    pipe.send(&Packet::Complete).await?;
    Ok(())
}
async fn cancel_job(core: Arc<Core>, job: Arc<Job>, tell_peer: bool) -> Result<()> {
    let _finalizer = job.move_finalizer.lock().await;
    let record = job.record.lock().unwrap().clone();
    ensure!(
        !lazy::cleanup_started(&core, &record.view.id)
            && (record.native_move.is_none() || record.move_outcome.is_some()),
        "The native Move has finished; its originals are being finalized"
    );
    if record.view.phase == Phase::Completed && record.offer.is_none() {
        if record.view.origin == Origin::Clipboard && record.view.direction == Direction::Receiving
        {
            job.cancel.store(true, Ordering::Release);
            {
                let mut r = job.record.lock().unwrap();
                r.view.clipboard_pending = false;
                r.view.clipboard_skipped = true;
            }
            core.clipboard_event(ClipboardEvent::Cancel(record.view.id));
            save(core.clone(), job.clone()).await?;
        }
        return Ok(());
    }
    job.cancel.store(true, Ordering::Release);
    job.cancelled.notify_waiters();
    job.pause_changed.notify_all();
    lazy::release_move_claim(&core, &record);
    job.record.lock().unwrap().move_grant = None;
    if record.view.direction == Direction::Receiving && record.view.origin == Origin::Clipboard {
        core.clipboard_event(ClipboardEvent::Cancel(record.view.id.clone()));
    }
    core.phase(&job, Phase::Cancelled, None);
    save(core.clone(), job.clone()).await?;
    if record.view.direction == Direction::Receiving {
        let c = core.clone();
        let workspace = record.workspace.clone();
        let source = record.view.peer.clone();
        let export = record.export.clone();
        tokio::task::spawn_blocking(move || {
            storage::remember_cancel(&c.dir, &workspace, &source, &export)
        })
        .await??;
    }
    if !job.running.load(Ordering::Acquire)
        && record.view.direction == Direction::Receiving
        && let Some(manifest) = record.manifest.clone()
    {
        let base = core.receive_base(&record);
        let peer = record.view.peer.clone();
        let workspace = record.workspace.clone();
        let cleaned = tokio::task::spawn_blocking(move || {
            match storage::Receive::open(&base, manifest.clone(), &peer, &workspace) {
                Ok(mut receive) => receive.cancel(),
                Err(error) => {
                    storage::Receive::cleanup_cancelled(&base, &manifest, &peer, &workspace)
                        .or(Err(error))
                }
            }
        })
        .await;
        if matches!(cleaned, Ok(Ok(()))) {
            job.cleaned.store(true, Ordering::Release);
        }
    } else if !job.running.load(Ordering::Acquire) {
        job.cleaned.store(true, Ordering::Release);
    }
    job.cleaning.store(false, Ordering::Release);
    if tell_peer {
        queue_control(core.clone(), job.clone(), true);
    }
    Ok(())
}
async fn publish_saved(core: Arc<Core>, job: Arc<Job>) -> Result<()> {
    let r = job.record.lock().unwrap().clone();
    ensure!(
        r.view.phase == Phase::Completed
            && r.view.direction == Direction::Receiving
            && r.view.origin == Origin::Clipboard,
        "Only received clipboard files can be copied again"
    );
    ensure!(
        core.allowed(&r.workspace, &r.view.peer, r.view.origin),
        "Enable shared clipboard before copying received files"
    );
    job.cancel.store(false, Ordering::Release);
    job.reserved.store(true, Ordering::Release);
    {
        let mut record = job.record.lock().unwrap();
        record.view.clipboard_pending = true;
        record.view.clipboard_skipped = false;
        record.view.error = None;
    }
    core.clipboard_event(ClipboardEvent::Begin(r.view.id.clone()));
    let c = core.clone();
    let j = job.clone();
    let result = tokio::task::spawn_blocking(move || {
        let manifest = r
            .manifest
            .context("The received file manifest is unavailable")?;
        let mut receive =
            storage::Receive::open(&c.downloads, manifest, &r.view.peer, &r.workspace)?;
        receive.finish_with_cancel(&j.cancel)
    })
    .await?;
    match result {
        Ok(paths) if !job.cancel.load(Ordering::Acquire) => {
            let id = job.record.lock().unwrap().view.id.clone();
            core.clipboard_event(ClipboardEvent::Ready(id, paths));
        }
        result => {
            let mut r = job.record.lock().unwrap();
            r.view.clipboard_pending = false;
            r.view.clipboard_skipped = true;
            if let Err(error) = result {
                r.view.error = Some(format!("{error:#}"));
            }
            drop(r);
            core.wake();
        }
    }
    save(core, job).await
}
async fn control(core: Arc<Core>, job: Arc<Job>, cancel: bool) -> Result<()> {
    let r = job.record.lock().unwrap().clone();
    ensure!(
        core.allowed(
            &r.workspace,
            &r.view.peer,
            if cancel { Origin::Send } else { r.view.origin }
        ),
        "The computer is no longer authorized for this file transfer"
    );
    let address = core.address(&r.view.peer)?;
    let mut pipe = transport::connect(address, &core.identity, &r.view.peer, &r.workspace).await?;
    pipe.send(&if cancel {
        Packet::CancelTransfer { id: r.export }
    } else {
        Packet::Resume { id: r.export }
    })
    .await?;
    ensure!(
        matches!(pipe.receive().await?, Packet::Queued),
        "The other computer could not resume the file transfer"
    );
    if cancel {
        job.record.lock().unwrap().cancel_notified = true;
        save(core.clone(), job).await?;
    }
    Ok(())
}
fn queue_control(core: Arc<Core>, job: Arc<Job>, cancel: bool) {
    if job.control_running.swap(true, Ordering::AcqRel) {
        return;
    }
    tokio::spawn(async move {
        let result = control(core.clone(), job.clone(), cancel).await;
        *job.retry.lock().unwrap() = Instant::now() + RETRY;
        job.control_running.store(false, Ordering::Release);
        if let Err(error) = result
            && !cancel
        {
            core.phase(&job, Phase::Paused, Some(format!("{error:#}")));
            let _ = save(core, job).await;
        }
    });
}
