//! Metadata-only clipboard offers and demand-owned native content leases.
use super::*;
use crate::group::clipboard_files::{
    Selection,
    provider::{
        CompletionSupport, DemandProvider, NativeConsumer, NativeResult, PasteIntent, PasteSession,
        RemoteOffer,
    },
};
use protocol::{Description, DescriptorKind};
use std::{
    fs::File,
    sync::{Condvar, Weak},
};

const RETIRED_OFFERS: usize = 8;
const OFFER_GRACE: Duration = Duration::from_secs(120);
const LEASE_LIFETIME: Duration = Duration::from_secs(90);
const LEASE_REFRESH: Duration = Duration::from_secs(30);

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct State {
    generation: u64,
    local: Option<String>,
    locals: BTreeMap<String, Arc<LocalOffer>>,
    remote: Option<String>,
    remotes: BTreeMap<String, Arc<RemoteState>>,
    fetches: BTreeMap<String, Weak<Fetch>>,
    leases: BTreeMap<String, Weak<Lease>>,
}

struct LocalOffer {
    id: String,
    stamp: Stamp,
    operation: Operation,
    description: Description,
    selection: Mutex<Option<storage::SourceSelection>>,
    active: AtomicBool,
    retired: Mutex<Option<Instant>>,
    cut_grant: Option<String>,
    leases: Mutex<BTreeMap<(String, String), Instant>>,
    sequence: u64,
    paths: Vec<PathBuf>,
    move_claim: Mutex<Option<(String, String)>>,
}

struct RemoteState {
    id: String,
    workspace: String,
    peer: String,
    stamp: Stamp,
    operation: Operation,
    description: Description,
    active: AtomicBool,
    retired: Mutex<Option<Instant>>,
    fetch: Mutex<Option<Arc<Fetch>>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct MoveReceipt {
    paste: String,
    consumer: NativeConsumer,
    entries: Vec<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct MoveGrant {
    cut_grant: String,
    cut_roots: String,
}

struct FetchState {
    verified: Option<Arc<Mutex<storage::Receive>>>,
    error: Option<String>,
    paused: bool,
    cancelled: bool,
    cleanup_started: bool,
    cleanup_consumer: Option<NativeConsumer>,
}

pub(super) struct Fetch {
    id: String,
    wire: String,
    core: Weak<Core>,
    remote: Weak<RemoteState>,
    state: Mutex<FetchState>,
    changed: Condvar,
    sessions: Mutex<BTreeMap<String, Weak<Session>>>,
    lease: String,
}

pub(super) struct Session {
    id: String,
    fetch: Arc<Fetch>,
    intent: PasteIntent,
    cancelled: AtomicBool,
    reported: AtomicBool,
    reporting: AtomicBool,
    _lease: Option<Arc<Lease>>,
}

struct Provider {
    core: Weak<Core>,
    remote: Arc<RemoteState>,
    lease: Option<Arc<Lease>>,
}
struct Lease {
    id: String,
    core: Weak<Core>,
    remote: Arc<RemoteState>,
    refreshed: Mutex<Instant>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(core) = self.core.upgrade() {
            let _ = core.commands.try_send(Command::ReleaseLease {
                offer: self.remote.id.clone(),
                peer: self.remote.peer.clone(),
                lease: self.id.clone(),
            });
        }
    }
}

impl DemandProvider for Provider {
    fn lease(&self) -> Result<Arc<dyn DemandProvider>> {
        let core = self.core.upgrade().context("File sharing has stopped")?;
        ensure!(
            core.allowed(&self.remote.workspace, &self.remote.peer, Origin::Clipboard),
            "This computer is no longer trusted for shared files"
        );
        if let Some(lease) = &self.lease {
            return Ok(Arc::new(Self {
                core: self.core.clone(),
                remote: self.remote.clone(),
                lease: Some(lease.clone()),
            }));
        }
        let within_grace = self
            .remote
            .retired
            .lock()
            .unwrap()
            .is_some_and(|retired| retired.elapsed() < OFFER_GRACE);
        ensure!(
            self.remote.active.load(Ordering::Acquire) || within_grace,
            "These copied files are no longer available. Copy them again."
        );
        let lease = Arc::new(Lease {
            id: new_id()?,
            core: self.core.clone(),
            remote: self.remote.clone(),
            refreshed: Mutex::new(Instant::now()),
        });
        core.commands
            .try_send(Command::AcquireLease {
                offer: self.remote.id.clone(),
                peer: self.remote.peer.clone(),
                lease: lease.id.clone(),
            })
            .context("The file transfer queue is full")?;
        core.lazy
            .lock()
            .unwrap()
            .leases
            .insert(lease.id.clone(), Arc::downgrade(&lease));
        Ok(Arc::new(Self {
            core: self.core.clone(),
            remote: self.remote.clone(),
            lease: Some(lease),
        }))
    }

    fn begin(&self, intent: PasteIntent) -> Result<Arc<dyn PasteSession>> {
        let core = self.core.upgrade().context("File sharing has stopped")?;
        ensure!(
            core.allowed(&self.remote.workspace, &self.remote.peer, Origin::Clipboard),
            "This computer is no longer trusted for shared files"
        );
        ensure!(
            intent.operation == self.remote.operation,
            "The copied file operation has changed"
        );
        ensure!(
            self.lease.is_some() || self.remote.active.load(Ordering::Acquire),
            "These copied files are no longer available. Copy them again."
        );
        if intent.operation == Operation::Move {
            intent
                .consumer
                .as_ref()
                .context("The native Move consumer is unavailable")?
                .validate()?;
        }
        let mut current = self.remote.fetch.lock().unwrap();
        let reusable = current
            .as_ref()
            .filter(|fetch| {
                let state = fetch.state.lock().unwrap();
                !state.cancelled && state.error.is_none()
            })
            .cloned();
        let (fetch, start) = if let Some(fetch) = reusable {
            (fetch, false)
        } else {
            let wire = new_id()?;
            let id = job_id(&wire, &self.remote.peer);
            let reports_completion = intent.operation == Operation::Move
                || intent.completion == CompletionSupport::Reported;
            let record = Record {
                workspace: self.remote.workspace.clone(),
                export: wire.clone(),
                stamp: Some(self.remote.stamp.clone()),
                created: millis(),
                manifest: None,
                cancel_notified: false,
                user_paused: false,
                peer_paused: false,
                offer: Some(self.remote.id.clone()),
                operation: self.remote.operation,
                native_move: None,
                move_binding: None,
                move_outcome: None,
                move_grant: None,
                view: Transfer {
                    id: id.clone(),
                    peer: self.remote.peer.clone(),
                    direction: Direction::Receiving,
                    origin: Origin::Clipboard,
                    name: self.remote.description.name.clone(),
                    phase: Phase::Preparing,
                    bytes: 0,
                    total: self.remote.description.total,
                    files: self.remote.description.entries.len() as u32,
                    error: None,
                    paths: vec![],
                    clipboard_ready: false,
                    clipboard_pending: false,
                    clipboard_skipped: false,
                    paste: Some(PasteView {
                        operation: intent.operation,
                        stage: PasteStage::Preparing,
                        destination: intent.destination.clone(),
                        reports_completion,
                    }),
                    pause_reason: None,
                    current_name: None,
                    recovery_paths: vec![],
                },
            };
            core.insert(record)?;
            let fetch = Arc::new(Fetch {
                id: id.clone(),
                wire,
                core: Arc::downgrade(&core),
                remote: Arc::downgrade(&self.remote),
                state: Mutex::new(FetchState {
                    verified: None,
                    error: None,
                    paused: false,
                    cancelled: false,
                    cleanup_started: false,
                    cleanup_consumer: None,
                }),
                changed: Condvar::new(),
                sessions: Mutex::new(BTreeMap::new()),
                lease: self
                    .lease
                    .as_ref()
                    .map_or_else(|| new_id().unwrap_or_default(), |lease| lease.id.clone()),
            });
            {
                let mut state = core.lazy.lock().unwrap();
                state.fetches.retain(|_, fetch| fetch.strong_count() > 0);
                state.fetches.insert(id.clone(), Arc::downgrade(&fetch));
            }
            *current = Some(fetch.clone());
            // Registration precedes the event, so GUI snapshot lookup is safe.
            core.events
                .lock()
                .unwrap()
                .push_back(FileEvent::PasteStarted { id });
            core.wake();
            (fetch, true)
        };
        let session = Arc::new(Session {
            id: new_id()?,
            fetch: fetch.clone(),
            intent,
            cancelled: AtomicBool::new(false),
            reported: AtomicBool::new(false),
            reporting: AtomicBool::new(false),
            _lease: self.lease.clone(),
        });
        {
            let mut sessions = fetch.sessions.lock().unwrap();
            sessions.retain(|_, session| session.strong_count() > 0);
            ensure!(
                sessions.len() < 256,
                "Too many copied file readers are open"
            );
            sessions.insert(session.id.clone(), Arc::downgrade(&session));
        }
        if start && let Err(error) = core.commands.try_send(Command::NativeBegin(fetch.clone())) {
            failed(
                &core,
                &fetch.id,
                "The file transfer queue is full".into(),
                false,
            );
            let job = core.jobs.lock().unwrap().get(&fetch.id).cloned();
            if let Some(job) = job {
                core.phase(
                    &job,
                    Phase::Failed,
                    Some("The file transfer queue is full".into()),
                );
            }
            return Err(error).context("The file transfer queue is full");
        }
        Ok(session)
    }
}

impl Session {
    fn wait(&self, ready: bool) -> Result<Option<Arc<Mutex<storage::Receive>>>> {
        self.wait_cancellable(ready, None)
    }
    fn wait_cancellable(
        &self,
        ready: bool,
        request: Option<&AtomicBool>,
    ) -> Result<Option<Arc<Mutex<storage::Receive>>>> {
        let mut state = self.fetch.state.lock().unwrap();
        loop {
            ensure!(
                !request.is_some_and(|cancel| cancel.load(Ordering::Acquire)),
                "The native file request was cancelled"
            );
            ensure!(
                !self.cancelled.load(Ordering::Acquire) && !state.cancelled,
                "The file transfer was cancelled"
            );
            if let Some(error) = &state.error {
                anyhow::bail!("{error}");
            }
            let core = self
                .fetch
                .core
                .upgrade()
                .context("File sharing has stopped")?;
            ensure!(
                !core.stopping.load(Ordering::Acquire),
                "File sharing has stopped"
            );
            if !state.paused && (!ready || state.verified.is_some()) {
                return Ok(state.verified.clone());
            }
            state = if request.is_some() {
                // Only an active native I/O request polls its external OS
                // cancellation flag. This never schedules UI work.
                self.fetch
                    .changed
                    .wait_timeout(state, Duration::from_millis(100))
                    .unwrap()
                    .0
            } else {
                self.fetch.changed.wait(state).unwrap()
            };
        }
    }
}

impl PasteSession for Session {
    fn id(&self) -> &str {
        &self.id
    }
    fn open_verified(&self, index: usize) -> Result<File> {
        let ready = self.wait(true)?.context("The copied files are not ready")?;
        let file = ready.lock().unwrap().open_verified(index)?;
        Ok(file)
    }
    fn wait_verified(&self) -> Result<()> {
        self.wait(true).map(|_| ())
    }
    fn open_verified_cancellable(&self, index: usize, cancel: &AtomicBool) -> Result<File> {
        let ready = self
            .wait_cancellable(true, Some(cancel))?
            .context("The copied files are not ready")?;
        let file = ready
            .lock()
            .unwrap()
            .open_verified_cancellable(index, cancel)?;
        Ok(file)
    }
    fn wait_verified_cancellable(&self, cancel: &AtomicBool) -> Result<()> {
        self.wait_cancellable(true, Some(cancel)).map(|_| ())
    }
    fn wait_unpaused_cancellable(&self, cancel: &AtomicBool) -> Result<()> {
        self.wait_cancellable(false, Some(cancel)).map(|_| ())
    }
    fn wait_unpaused(&self) -> Result<()> {
        self.wait(false).map(|_| ())
    }
    fn report_read(&self, index: usize, offset: u64, count: usize) -> Result<()> {
        // These bytes have already been supplied to the native consumer. A
        // subsequent Pause must not block recording their actual coverage.
        let remote = self
            .fetch
            .remote
            .upgrade()
            .context("The copied file metadata is unavailable")?;
        let entry = remote
            .description
            .entries
            .get(index)
            .context("Unknown copied file")?;
        let DescriptorKind::File { size } = entry.kind else {
            anyhow::bail!("The copied entry is a folder");
        };
        ensure!(
            offset
                .checked_add(count as u64)
                .is_some_and(|end| end <= size),
            "A native file read exceeded the copied file"
        );
        Ok(())
    }
    fn is_terminal(&self) -> bool {
        self.cancelled.load(Ordering::Acquire) || self.reported.load(Ordering::Acquire) || {
            let state = self.fetch.state.lock().unwrap();
            state.cancelled || state.error.is_some()
        }
    }
    fn can_retry_move(&self) -> bool {
        let state = self.fetch.state.lock().unwrap();
        self.intent.operation == Operation::Move
            && (self.cancelled.load(Ordering::Acquire) || state.cancelled || state.error.is_some())
            && !state.cleanup_started
            && self
                .fetch
                .remote
                .upgrade()
                .is_some_and(|remote| remote.active.load(Ordering::Acquire))
    }
    fn native_complete(&self, result: NativeResult) -> Result<()> {
        if self.reported.load(Ordering::Acquire) {
            return Ok(());
        }
        ensure!(
            !self.reporting.swap(true, Ordering::AcqRel),
            "The native operation result is already being queued"
        );
        struct Reporting<'a>(&'a AtomicBool);
        impl Drop for Reporting<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _reporting = Reporting(&self.reporting);
        let moving = matches!(result, NativeResult::MoveCommitted { .. });
        if let NativeResult::MoveCommitted { consumer, entries } = &result {
            ensure!(
                self.intent.operation == Operation::Move,
                "Only an explicitly cut selection can move originals"
            );
            consumer.validate()?;
            ensure!(
                self.intent.consumer.as_ref() == Some(consumer),
                "The native Move consumer changed"
            );
            let remote = self
                .fetch
                .remote
                .upgrade()
                .context("The copied file metadata is unavailable")?;
            let indices = entries.iter().copied().collect::<BTreeSet<_>>();
            ensure!(
                indices.len() == entries.len()
                    && indices.into_iter().eq(0..remote.description.entries.len()),
                "A Move must confirm every copied file and folder"
            );
            let state = self.fetch.state.lock().unwrap();
            ensure!(
                state.verified.is_some() && !state.cancelled && state.error.is_none(),
                "The moved files have not been verified"
            );
        }
        ensure!(
            !matches!(result, NativeResult::Succeeded) || self.intent.operation == Operation::Copy,
            "Moving originals requires a complete native Move receipt"
        );
        if matches!(result, NativeResult::Cancelled | NativeResult::Failed(_)) {
            self.cancelled.store(true, Ordering::Release);
            self.fetch.changed.notify_all();
        }
        let core = self
            .fetch
            .core
            .upgrade()
            .context("File sharing has stopped")?;
        let session = self
            .fetch
            .sessions
            .lock()
            .unwrap()
            .get(&self.id)
            .and_then(Weak::upgrade)
            .context("The native file operation has ended")?;
        let mut state = self.fetch.state.lock().unwrap();
        let already_cleaning = state.cleanup_started;
        if moving {
            ensure!(
                state.cleanup_consumer.as_ref().is_none_or(|consumer| self
                    .intent
                    .consumer
                    .as_ref()
                    == Some(consumer)),
                "Another native consumer already owns this Move"
            );
            state.cleanup_started = true;
            state.cleanup_consumer = self.intent.consumer.clone();
        }
        if let Err(error) = core
            .commands
            .try_send(Command::NativeResult(session, result))
        {
            if moving && !already_cleaning {
                state.cleanup_started = false;
                state.cleanup_consumer = None;
            }
            return Err(error).context("The file transfer queue is full");
        }
        self.reported.store(true, Ordering::Release);
        Ok(())
    }
}

pub(super) fn fetch(core: &Core, id: &str) -> Option<Arc<Fetch>> {
    core.lazy
        .lock()
        .unwrap()
        .fetches
        .get(id)
        .and_then(Weak::upgrade)
}
pub(super) fn cleanup_started(core: &Core, id: &str) -> bool {
    fetch(core, id).is_some_and(|fetch| fetch.state.lock().unwrap().cleanup_started)
}

pub(super) fn release_move_claim(core: &Core, record: &Record) {
    if record.native_move.is_some() {
        return;
    }
    let offer = record
        .offer
        .as_ref()
        .and_then(|id| core.lazy.lock().unwrap().locals.get(id).cloned());
    if let Some(offer) = offer {
        let mut claim = offer.move_claim.lock().unwrap();
        if claim
            .as_ref()
            .is_some_and(|(peer, wire)| peer == &record.view.peer && wire == &record.export)
        {
            *claim = None;
        }
    }
}
pub(super) fn paused(core: &Core, id: &str, value: bool) {
    if let Some(fetch) = fetch(core, id) {
        fetch.state.lock().unwrap().paused = value;
        fetch.changed.notify_all();
    }
}
pub(super) fn failed(core: &Core, id: &str, error: String, cancelled: bool) {
    if let Some(fetch) = fetch(core, id) {
        let mut state = fetch.state.lock().unwrap();
        state.error = Some(error);
        state.cancelled = cancelled;
        drop(state);
        fetch.changed.notify_all();
        if cancelled {
            for session in fetch
                .sessions
                .lock()
                .unwrap()
                .values()
                .filter_map(Weak::upgrade)
            {
                session.cancelled.store(true, Ordering::Release);
            }
        }
    }
}
pub(super) fn delivered(core: &Core, id: &str, receive: Arc<Mutex<storage::Receive>>) {
    if let Some(fetch) = fetch(core, id) {
        fetch.state.lock().unwrap().verified = Some(receive);
        fetch.changed.notify_all();
    }
}
pub(super) fn stop(core: &Core) {
    let fetches = core
        .lazy
        .lock()
        .unwrap()
        .fetches
        .values()
        .filter_map(Weak::upgrade)
        .collect::<Vec<_>>();
    for fetch in fetches {
        fetch.state.lock().unwrap().cancelled = true;
        fetch.changed.notify_all();
    }
}

/// A native handle/cache and an unfinished Move receipt outlive network delivery.
pub(super) fn protected_jobs(core: &Core) -> BTreeSet<String> {
    let mut state = core.lazy.lock().unwrap();
    state.fetches.retain(|_, fetch| fetch.strong_count() > 0);
    state.fetches.keys().cloned().collect()
}

/// Leases are bounded metadata capabilities. A lost release message expires;
/// an actual started transfer is retained independently by its persisted job.
pub(super) fn maintain(core: &Arc<Core>) {
    let (refresh, locals) = {
        let mut state = core.lazy.lock().unwrap();
        state.fetches.retain(|_, fetch| fetch.strong_count() > 0);
        state.leases.retain(|_, lease| lease.strong_count() > 0);
        let refresh = state
            .leases
            .values()
            .filter_map(Weak::upgrade)
            .filter(|lease| {
                let mut last = lease.refreshed.lock().unwrap();
                if last.elapsed() >= LEASE_REFRESH {
                    *last = Instant::now();
                    true
                } else {
                    false
                }
            })
            .collect::<Vec<_>>();
        (refresh, state.locals.values().cloned().collect::<Vec<_>>())
    };
    for offer in locals {
        offer
            .leases
            .lock()
            .unwrap()
            .retain(|_, deadline| *deadline > Instant::now());
    }
    for lease in refresh {
        let core = core.clone();
        tokio::spawn(async move {
            let _ = lease_remote(
                core,
                lease.remote.peer.clone(),
                lease.remote.id.clone(),
                lease.id.clone(),
                false,
            )
            .await;
        });
    }
}

pub(super) fn set_user_pause(core: &Core, job: &Job, value: bool) {
    let is_paused = {
        let mut record = job.record.lock().unwrap();
        record.user_paused = value;
        if !value {
            record.peer_paused = false;
        }
        record.user_paused || record.peer_paused
    };
    *job.paused.lock().unwrap() = is_paused;
    job.pause_changed.notify_all();
    job.cancelled.notify_waiters();
    let id = job.record.lock().unwrap().view.id.clone();
    if let Some(fetch) = fetch(core, &id) {
        let mut state = fetch.state.lock().unwrap();
        state.paused = is_paused;
        if !value && !state.cleanup_started {
            state.error = None;
        }
        drop(state);
        fetch.changed.notify_all();
    }
}

pub(super) fn resume(core: Arc<Core>, job: Arc<Job>) {
    resume_inner(core, job, true);
}

pub(super) fn resume_after_peer(core: Arc<Core>, job: Arc<Job>) {
    resume_inner(core, job, false);
}

fn resume_inner(core: Arc<Core>, job: Arc<Job>, notify_peer: bool) {
    set_user_pause(&core, &job, false);
    let record = job.record.lock().unwrap().clone();
    job.cancel.store(false, Ordering::Release);
    *job.retry.lock().unwrap() = Instant::now();
    if record.native_move.is_some()
        && record.move_outcome.is_none()
        && record.view.direction == Direction::Receiving
    {
        retry_move(core, job);
        return;
    }
    if record.view.direction == Direction::Receiving && record.offer.is_some() {
        core.phase(&job, Phase::Connecting, None);
        if let Some(fetch) = fetch(&core, &record.view.id) {
            tokio::spawn(async move {
                request(core, fetch).await;
            });
        } else {
            queue_control(core, job, false);
        }
    } else if record.view.direction == Direction::Sending && record.manifest.is_none() {
        let offer = record
            .offer
            .as_ref()
            .and_then(|id| core.lazy.lock().unwrap().locals.get(id).cloned());
        if let Some(offer) = offer {
            if !job.running.swap(true, Ordering::AcqRel) {
                core.phase(&job, Phase::Preparing, None);
                tokio::spawn(async move {
                    prepare_demand(core, job, offer).await;
                });
            }
        } else {
            core.phase(
                &job,
                Phase::Failed,
                Some("The copied files are unavailable. Copy them again.".into()),
            );
        }
    } else {
        core.phase(&job, Phase::Paused, None);
        if notify_peer {
            queue_control(core, job, false);
        }
    }
}

async fn pipe(core: &Core, peer: &str, workspace: &str) -> Result<transport::Connection> {
    ensure!(
        core.allowed(workspace, peer, Origin::Send),
        "The computer is no longer trusted"
    );
    transport::connect(core.address(peer)?, &core.identity, peer, workspace).await
}

fn retire_local(core: &Arc<Core>) {
    let retired = {
        let mut lazy = core.lazy.lock().unwrap();
        lazy.generation = lazy.generation.wrapping_add(1);
        lazy.local
            .take()
            .and_then(|id| lazy.locals.get(&id).cloned())
    };
    if let Some(offer) = retired {
        offer.active.store(false, Ordering::Release);
        *offer.retired.lock().unwrap() = Some(Instant::now());
        let trust = core.trust.lock().unwrap().clone();
        for peer in trust.peers.keys().cloned().collect::<Vec<_>>() {
            let core = core.clone();
            let id = offer.id.clone();
            let workspace = trust.workspace_id.clone();
            tokio::spawn(async move {
                if let Ok(mut pipe) = pipe(&core, &peer, &workspace).await {
                    let _ = pipe.send(&Packet::RetireOffer { id }).await;
                }
            });
        }
    }
}

pub(super) fn begin_capture(core: &Arc<Core>) -> u64 {
    retire_local(core);
    let mut state = core.lazy.lock().unwrap();
    if let Some(old) = state
        .remote
        .take()
        .and_then(|id| state.remotes.get(&id).cloned())
    {
        old.active.store(false, Ordering::Release);
        *old.retired.lock().unwrap() = Some(Instant::now());
    }
    state.generation
}

pub(super) fn disable(core: &Arc<Core>) {
    retire_local(core);
    let mut state = core.lazy.lock().unwrap();
    state.remote = None;
    for offer in state.remotes.values() {
        offer.active.store(false, Ordering::Release);
        *offer.retired.lock().unwrap() = Some(Instant::now());
    }
}

pub(super) async fn capture(
    core: Arc<Core>,
    selected: Option<Selection>,
    generation: u64,
) -> Result<()> {
    if core.lazy.lock().unwrap().generation != generation {
        return Ok(());
    }
    let Some(selected) = selected.filter(|selection| !selection.paths.is_empty()) else {
        return Ok(());
    };
    let trust = core.trust.lock().unwrap().clone();
    if !trust.clipboard_enabled || trust.workspace_id.is_empty() {
        return Ok(());
    }
    let stamp = core.stamp(&trust.own);
    let paths = selected.paths.clone();
    let inventory = tokio::task::spawn_blocking(move || {
        storage::SourceSelection::capture(&paths, &AtomicBool::new(false))
    })
    .await??;
    let cut_grant = if selected.operation == Operation::Move {
        inventory.cut_root_digest()?;
        Some(new_id()?)
    } else {
        None
    };
    let offer = Arc::new(LocalOffer {
        id: new_id()?,
        stamp,
        operation: selected.operation,
        description: inventory.description.clone(),
        selection: Mutex::new(Some(inventory)),
        active: AtomicBool::new(true),
        retired: Mutex::new(None),
        cut_grant,
        leases: Mutex::new(BTreeMap::new()),
        sequence: selected.sequence,
        paths: selected.paths,
        move_claim: Mutex::new(None),
    });
    {
        let mut state = core.lazy.lock().unwrap();
        if state.generation != generation || !core.trust.lock().unwrap().clipboard_enabled {
            return Ok(());
        }
        let mut retired = state
            .locals
            .values()
            .filter(|offer| {
                !offer.active.load(Ordering::Acquire)
                    && offer.leases.lock().unwrap().is_empty()
                    && offer.move_claim.lock().unwrap().is_none()
            })
            .map(|offer| (offer.stamp.clone(), offer.id.clone()))
            .collect::<Vec<_>>();
        retired.sort();
        for (_, id) in retired.into_iter().rev().skip(RETIRED_OFFERS) {
            state.locals.remove(&id);
        }
        ensure!(
            state.locals.len() < MAX_JOBS,
            "Finish active file operations before sharing another selection"
        );
        state.local = Some(offer.id.clone());
        state.locals.insert(offer.id.clone(), offer.clone());
    }
    *core.error.lock().unwrap() = None;
    for peer in trust.peers.keys().cloned().collect::<Vec<_>>() {
        let c = core.clone();
        let offer = offer.clone();
        let workspace = trust.workspace_id.clone();
        tokio::spawn(async move {
            let result = async {
                let mut pipe = pipe(&c, &peer, &workspace).await?;
                pipe.send(&Packet::ClipboardOffer {
                    id: offer.id.clone(),
                    description: offer.description.clone(),
                    stamp: offer.stamp.clone(),
                    operation: offer.operation,
                })
                .await?;
                ensure!(
                    matches!(pipe.receive().await?, Packet::Queued),
                    "The other computer could not offer copied files"
                );
                Ok::<_, anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                *c.error.lock().unwrap() = Some(format!("Shared files: {error:#}"));
                c.wake();
            }
        });
    }
    Ok(())
}

pub(super) async fn incoming_offer(
    core: Arc<Core>,
    peer: String,
    id: String,
    description: Description,
    stamp: Stamp,
    operation: Operation,
    pipe: &mut transport::Connection,
) -> Result<()> {
    protocol::valid_id(&id)?;
    description.validate()?;
    ensure!(
        stamp.author == peer && stamp.counter > 0,
        "Invalid copied file author"
    );
    let trust = core.trust.lock().unwrap().clone();
    ensure!(
        core.allowed(&trust.workspace_id, &peer, Origin::Clipboard),
        "Shared file clipboard is disabled"
    );
    let stale = {
        let mut latest = core.latest.lock().unwrap();
        if latest.as_ref().is_some_and(|current| current > &stamp) {
            true
        } else {
            *latest = Some(stamp.clone());
            false
        }
    };
    if stale {
        pipe.send(&Packet::Queued).await?;
        return Ok(());
    }
    let handle = job_id(&id, &peer);
    let existing = core.lazy.lock().unwrap().remotes.get(&handle).cloned();
    if let Some(existing) = existing {
        ensure!(
            existing.stamp == stamp
                && existing.description == description
                && existing.operation == operation,
            "A copied file offer was reused"
        );
        pipe.send(&Packet::Queued).await?;
        return Ok(());
    }
    let offer = {
        let mut state = core.lazy.lock().unwrap();
        ensure!(
            !state.remotes.contains_key(&handle),
            "The copied offer is already being registered"
        );
        if let Some(previous) = state
            .remote
            .take()
            .and_then(|old| state.remotes.get(&old).cloned())
        {
            previous.active.store(false, Ordering::Release);
            *previous.retired.lock().unwrap() = Some(Instant::now());
        }
        state.remotes.retain(|_, value| {
            value.active.load(Ordering::Acquire)
                || value
                    .retired
                    .lock()
                    .unwrap()
                    .is_none_or(|retired| retired.elapsed() < OFFER_GRACE)
                || Arc::strong_count(value) > 1
        });
        ensure!(
            state.remotes.len() < MAX_JOBS,
            "Too many copied file providers are still in use"
        );
        let remote = Arc::new(RemoteState {
            id,
            workspace: trust.workspace_id.clone(),
            peer,
            stamp,
            operation,
            description,
            active: AtomicBool::new(true),
            retired: Mutex::new(None),
            fetch: Mutex::new(None),
        });
        state.remote = Some(handle.clone());
        state.remotes.insert(handle.clone(), remote.clone());
        remote
    };
    let provider = Arc::new(Provider {
        core: Arc::downgrade(&core),
        remote: offer.clone(),
        lease: None,
    });
    core.clipboard_event(ClipboardEvent::Offer(RemoteOffer {
        id: handle,
        description: offer.description.clone(),
        operation,
        provider,
    }));
    pipe.send(&Packet::Queued).await?;
    Ok(())
}

pub(super) fn retired_remote(core: &Core, peer: &str, id: &str) {
    let mut state = core.lazy.lock().unwrap();
    let handle = job_id(id, peer);
    if let Some(offer) = state.remotes.get(&handle) {
        offer.active.store(false, Ordering::Release);
        *offer.retired.lock().unwrap() = Some(Instant::now());
    }
    if state.remote.as_ref() == Some(&handle) {
        state.remote = None;
    }
}

pub(super) fn published(core: &Arc<Core>, id: &str, accepted: bool) {
    let remote = core.lazy.lock().unwrap().remotes.get(id).cloned();
    if let Some(remote) = remote {
        if accepted {
            retire_local(core);
        } else {
            remote.active.store(false, Ordering::Release);
            *remote.retired.lock().unwrap() = Some(Instant::now());
        }
    }
}

pub(super) async fn lease_remote(
    core: Arc<Core>,
    peer: String,
    offer: String,
    lease: String,
    release: bool,
) -> Result<()> {
    let workspace = core.trust.lock().unwrap().workspace_id.clone();
    if !release {
        ensure!(
            core.allowed(&workspace, &peer, Origin::Clipboard),
            "Shared file clipboard is disabled"
        );
    }
    let mut pipe = pipe(&core, &peer, &workspace).await?;
    pipe.send(&if release {
        Packet::ReleaseOfferLease { offer, lease }
    } else {
        Packet::LeaseOffer { offer, lease }
    })
    .await?;
    ensure!(
        matches!(pipe.receive().await?, Packet::Queued),
        "The copied file lease is unavailable"
    );
    Ok(())
}

pub(super) fn local_lease(
    core: &Core,
    peer: &str,
    id: &str,
    lease: &str,
    release: bool,
) -> Result<()> {
    protocol::valid_id(lease)?;
    let offer = core
        .lazy
        .lock()
        .unwrap()
        .locals
        .get(id)
        .cloned()
        .context("These copied files are no longer available")?;
    let key = (peer.into(), lease.into());
    let mut leases = offer.leases.lock().unwrap();
    if release {
        leases.remove(&key);
        return Ok(());
    }
    leases.retain(|_, deadline| *deadline > Instant::now());
    let workspace = core.trust.lock().unwrap().workspace_id.clone();
    ensure!(
        core.allowed(&workspace, peer, Origin::Clipboard),
        "Shared file clipboard is disabled"
    );
    let grace = offer
        .retired
        .lock()
        .unwrap()
        .is_some_and(|retired| retired.elapsed() < OFFER_GRACE);
    ensure!(
        leases.contains_key(&key) || offer.active.load(Ordering::Acquire) || grace,
        "These copied files are no longer available. Copy them again."
    );
    ensure!(
        leases.len() < 256 || leases.contains_key(&key),
        "Too many copied file handles are open"
    );
    leases.insert(key, Instant::now() + LEASE_LIFETIME);
    Ok(())
}

pub(super) async fn request(core: Arc<Core>, fetch: Arc<Fetch>) {
    let job = core.jobs.lock().unwrap().get(&fetch.id).cloned();
    let Some(job) = job else {
        return;
    };
    if job.control_running.swap(true, Ordering::AcqRel) {
        return;
    }
    struct RequestGuard(Arc<Job>);
    impl Drop for RequestGuard {
        fn drop(&mut self) {
            self.0.control_running.store(false, Ordering::Release);
        }
    }
    let _request = RequestGuard(job.clone());
    let result = async {
        let remote = fetch
            .remote
            .upgrade()
            .context("The copied file offer is unavailable")?;
        let job = core
            .jobs
            .lock()
            .unwrap()
            .get(&fetch.id)
            .cloned()
            .context("The file transfer has ended")?;
        core.check(&job)?;
        save(core.clone(), job.clone()).await?;
        let mut pipe = pipe(&core, &remote.peer, &remote.workspace).await?;
        core.check(&job)?;
        pipe.send(&Packet::PasteDemand {
            offer: remote.id.clone(),
            id: fetch.wire.clone(),
            operation: remote.operation,
            lease: fetch.lease.clone(),
        })
        .await?;
        match pipe.receive().await? {
            Packet::Queued => {
                if job.record.lock().unwrap().manifest.is_none() {
                    core.phase(&job, Phase::Preparing, None);
                }
                Ok(())
            }
            Packet::Error(error) => anyhow::bail!("{error}"),
            _ => anyhow::bail!("The other computer could not start the pasted files"),
        }
    }
    .await;
    *job.retry.lock().unwrap() = Instant::now() + RETRY;
    if let Err(error) = result {
        let job = core.jobs.lock().unwrap().get(&fetch.id).cloned();
        if let Some(job) = job {
            let phase = if retryable(&error) {
                Phase::Paused
            } else {
                Phase::Failed
            };
            core.phase(&job, phase, Some(format!("{error:#}")));
            *job.retry.lock().unwrap() = Instant::now() + RETRY;
            let _ = save(core.clone(), job).await;
        }
    }
}

async fn grant_move(
    core: Arc<Core>,
    offer: Arc<LocalOffer>,
    peer: &str,
    wire: &str,
) -> Result<Option<MoveGrant>> {
    if offer.operation != Operation::Move || !offer.active.load(Ordering::Acquire) {
        return Ok(None);
    }
    let workspace = core.trust.lock().unwrap().workspace_id.clone();
    if !core.allowed(&workspace, peer, Origin::Clipboard) {
        return Ok(None);
    }
    let paths = offer.paths.clone();
    let sequence = offer.sequence;
    // This bounded worker reads native formats only; it never opens file data.
    // A late OPEN lease cannot renew an original Cut after another local Copy.
    let _slot = core.prepare.clone().acquire_owned().await?;
    let current = tokio::task::spawn_blocking(move || {
        crate::group::clipboard_files::read().map(|selection| {
            selection.is_some_and(|selection| {
                selection.sequence == sequence
                    && selection.paths == paths
                    && selection.operation == Operation::Move
            })
        })
    })
    .await?
    .unwrap_or(false);
    if !current
        || !offer.active.load(Ordering::Acquire)
        || !core.allowed(&workspace, peer, Origin::Clipboard)
    {
        return Ok(None);
    }
    let mut claim = offer.move_claim.lock().unwrap();
    if claim
        .as_ref()
        .is_some_and(|(owner, id)| owner != peer || id != wire)
    {
        return Ok(None);
    }
    let grant = MoveGrant {
        cut_grant: offer
            .cut_grant
            .clone()
            .context("The source selection was not cut")?,
        cut_roots: offer
            .selection
            .lock()
            .unwrap()
            .as_ref()
            .context("The source Cut has already been finalized")?
            .cut_root_digest()?,
    };
    *claim = Some((peer.to_owned(), wire.to_owned()));
    Ok(Some(grant))
}

pub(super) async fn demand(
    core: Arc<Core>,
    peer: String,
    offer_id: String,
    wire: String,
    operation: Operation,
    lease: String,
    pipe: &mut transport::Connection,
) -> Result<()> {
    protocol::valid_id(&wire)?;
    protocol::valid_id(&offer_id)?;
    protocol::valid_id(&lease)?;
    let id = job_id(&wire, &peer);
    let existing = core.jobs.lock().unwrap().get(&id).cloned();
    if let Some(job) = existing {
        let record = job.record.lock().unwrap().clone();
        ensure!(
            core.allowed(&record.workspace, &peer, Origin::Send)
                && record.offer.as_ref() == Some(&offer_id)
                && record.operation == operation
                && record.view.direction == Direction::Sending,
            "The pasted file identity was reused"
        );
        if record.view.phase == Phase::Cancelled {
            pipe.send(&Packet::Cancel).await?;
            return Ok(());
        }
        set_user_pause(&core, &job, false);
        if record.manifest.is_some() {
            pipe.send(&Packet::Queued).await?;
            if !job.running.swap(true, Ordering::AcqRel) {
                let c = core.clone();
                tokio::spawn(async move {
                    send_job(c, job).await;
                });
            }
        } else {
            let offer = core
                .lazy
                .lock()
                .unwrap()
                .locals
                .get(&offer_id)
                .cloned()
                .context("The source preparation cannot resume. Copy the files again.")?;
            pipe.send(&Packet::Queued).await?;
            if !job.running.swap(true, Ordering::AcqRel) {
                tokio::spawn(async move {
                    prepare_demand(core, job, offer).await;
                });
            }
        }
        return Ok(());
    }
    local_lease(&core, &peer, &offer_id, &lease, false)?;
    let offer = core
        .lazy
        .lock()
        .unwrap()
        .locals
        .get(&offer_id)
        .cloned()
        .context("These copied files are no longer available. Copy them again.")?;
    ensure!(
        offer.operation == operation,
        "The copied file operation changed"
    );
    let move_grant = if operation == Operation::Move {
        grant_move(core.clone(), offer.clone(), &peer, &wire).await?
    } else {
        None
    };
    let trust = core.trust.lock().unwrap().clone();
    ensure!(
        core.allowed(&trust.workspace_id, &peer, Origin::Clipboard),
        "Shared file clipboard is disabled"
    );
    let job = core.insert(Record {
        workspace: trust.workspace_id,
        export: wire,
        stamp: Some(offer.stamp.clone()),
        created: millis(),
        manifest: None,
        cancel_notified: false,
        user_paused: false,
        peer_paused: false,
        offer: Some(offer_id),
        operation,
        native_move: None,
        move_binding: None,
        move_outcome: None,
        move_grant,
        view: Transfer {
            id,
            peer,
            direction: Direction::Sending,
            origin: Origin::Clipboard,
            name: offer.description.name.clone(),
            phase: Phase::Preparing,
            bytes: 0,
            total: offer.description.total,
            files: offer.description.entries.len() as u32,
            error: None,
            paths: vec![],
            clipboard_ready: false,
            clipboard_pending: false,
            clipboard_skipped: false,
            paste: Some(PasteView {
                operation,
                stage: PasteStage::Preparing,
                destination: None,
                reports_completion: operation == Operation::Move,
            }),
            pause_reason: None,
            current_name: None,
            recovery_paths: vec![],
        },
    })?;
    if reject_cancelled_registration(core.clone(), job.clone()).await? {
        pipe.send(&Packet::Cancel).await?;
        return Ok(());
    }
    save(core.clone(), job.clone()).await?;
    pipe.send(&Packet::Queued).await?;
    job.running.store(true, Ordering::Release);
    tokio::spawn(async move {
        prepare_demand(core, job, offer).await;
    });
    Ok(())
}

async fn prepare_demand(core: Arc<Core>, job: Arc<Job>, offer: Arc<LocalOffer>) {
    let result = async {
        let _running = Running(job.clone());
        let _prepare = core.prepare.clone().acquire_owned().await?;
        core.check(&job)?;
        let used = {
            let jobs = core.jobs.lock().unwrap();
            let mut seen = BTreeSet::new();
            jobs.values()
                .filter_map(|candidate| {
                    let r = candidate.record.lock().unwrap();
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
        let j = job.clone();
        let source = offer.clone();
        let export_id = job.record.lock().unwrap().export.clone();
        let export = tokio::task::spawn_blocking(move || {
            let selected = source.selection.lock().unwrap();
            let selected = selected
                .as_ref()
                .context("The cut selection has already been finalized")?;
            let cancel = AtomicBool::new(false);
            selected.prepare(&c.dir, &export_id, budget, &cancel, |bytes| {
                let mut paused = j.paused.lock().unwrap();
                while *paused
                    && !j.cancel.load(Ordering::Acquire)
                    && !c.stopping.load(Ordering::Acquire)
                {
                    paused = j.pause_changed.wait(paused).unwrap();
                }
                cancel.store(
                    j.cancel.load(Ordering::Acquire) || c.stopping.load(Ordering::Acquire),
                    Ordering::Release,
                );
                c.progress(&j, bytes);
            })
        })
        .await??;
        ensure!(
            export.manifest.description() == offer.description,
            "The copied files changed. Copy them again before pasting."
        );
        {
            let mut record = job.record.lock().unwrap();
            record.manifest = Some(export.manifest.clone());
            record.view.total = export.manifest.total;
            record.view.files = export.manifest.entries.len() as u32;
        }
        core.progress(&job, 0);
        core.phase(&job, Phase::Connecting, None);
        save(core.clone(), job.clone()).await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if let Err(error) = result {
        let phase = if job.cancel.load(Ordering::Acquire) {
            Phase::Cancelled
        } else if retryable(&error) {
            Phase::Paused
        } else {
            Phase::Failed
        };
        core.phase(&job, phase, Some(format!("{error:#}")));
        let _ = save(core.clone(), job.clone()).await;
        let peer = job.record.lock().unwrap().view.peer.clone();
        let workspace = job.record.lock().unwrap().workspace.clone();
        if let Ok(mut pipe) = pipe(&core, &peer, &workspace).await {
            let id = job.record.lock().unwrap().export.clone();
            let _ = pipe
                .send(&Packet::PasteFailed {
                    id,
                    error: format!("{error:#}"),
                    retryable: retryable(&error),
                })
                .await;
        }
    }
}

pub(super) fn expected_manifest(core: &Core, job: &Job, manifest: &Manifest) -> Result<()> {
    let record = job.record.lock().unwrap();
    if let Some(saved) = &record.manifest {
        ensure!(saved == manifest, "The copied file manifest changed");
        return Ok(());
    }
    if let Some(offer) = &record.offer {
        let handle = job_id(offer, &record.view.peer);
        let remote = core
            .lazy
            .lock()
            .unwrap()
            .remotes
            .get(&handle)
            .cloned()
            .context("The copied file metadata is unavailable")?;
        ensure!(
            manifest.description() == remote.description,
            "The copied files changed. Copy them again before pasting."
        );
    }
    Ok(())
}

pub(super) async fn pause_peer(core: Arc<Core>, job: Arc<Job>) -> Result<()> {
    let record = job.record.lock().unwrap().clone();
    let mut pipe = pipe(&core, &record.view.peer, &record.workspace).await?;
    pipe.send(&Packet::PauseTransfer { id: record.export })
        .await?;
    ensure!(
        matches!(pipe.receive().await?, Packet::Queued),
        "The other computer could not pause the transfer"
    );
    Ok(())
}

pub(super) fn set_peer_pause(core: &Core, job: &Job, value: bool) {
    let is_paused = {
        let mut record = job.record.lock().unwrap();
        record.peer_paused = value;
        record.user_paused || value
    };
    *job.paused.lock().unwrap() = is_paused;
    job.pause_changed.notify_all();
    job.cancelled.notify_waiters();
    let id = job.record.lock().unwrap().view.id.clone();
    paused(core, &id, is_paused);
}

pub(super) async fn native_result(
    core: Arc<Core>,
    session: Arc<Session>,
    result: NativeResult,
) -> Result<()> {
    let job = core
        .jobs
        .lock()
        .unwrap()
        .get(&session.fetch.id)
        .cloned()
        .context("The native file transfer is no longer available")?;
    match result {
        NativeResult::Cancelled | NativeResult::Failed(_) => {
            if session.fetch.state.lock().unwrap().cleanup_started {
                return Ok(());
            }
            let another = session
                .fetch
                .sessions
                .lock()
                .unwrap()
                .values()
                .filter_map(Weak::upgrade)
                .any(|other| {
                    other.id != session.id
                        && !other.cancelled.load(Ordering::Acquire)
                        && !other.reported.load(Ordering::Acquire)
                });
            if !another {
                let failure = if let NativeResult::Failed(error) = result {
                    Some(error)
                } else {
                    None
                };
                cancel_job(core.clone(), job.clone(), true).await?;
                if let Some(error) = failure {
                    core.phase(
                        &job,
                        Phase::Failed,
                        Some(error.chars().take(2048).collect()),
                    );
                    save(core, job).await?;
                }
            }
        }
        NativeResult::Succeeded => {
            ensure!(
                session.intent.operation == Operation::Copy,
                "Move requires a complete native receipt"
            );
            {
                let mut r = job.record.lock().unwrap();
                if let Some(paste) = &mut r.view.paste {
                    paste.stage = PasteStage::NativeCompleted;
                }
                r.view.phase = Phase::Completed;
            }
            core.wake();
            save(core, job).await?;
        }
        NativeResult::MoveCommitted { consumer, entries } => {
            {
                let mut r = job.record.lock().unwrap();
                ensure!(
                    r.operation == Operation::Move && r.manifest.is_some(),
                    "The Move files have not been verified"
                );
                let entries = entries
                    .into_iter()
                    .map(u32::try_from)
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let receipt = if let Some(existing) = &r.native_move {
                    ensure!(
                        existing.consumer == consumer && existing.entries == entries,
                        "Another native consumer already owns this Move"
                    );
                    existing.clone()
                } else {
                    MoveReceipt {
                        paste: new_id()?,
                        consumer,
                        entries,
                    }
                };
                r.native_move = Some(receipt);
                if let Some(paste) = &mut r.view.paste {
                    paste.stage = PasteStage::RemovingOriginals;
                }
                r.view.phase = Phase::Verifying;
            }
            core.wake();
            save(core.clone(), job.clone()).await?;
            retry_move(core, job);
        }
    }
    Ok(())
}

pub(super) fn retry_move(core: Arc<Core>, job: Arc<Job>) {
    if job.control_running.swap(true, Ordering::AcqRel) {
        return;
    }
    tokio::spawn(async move {
        let receipt = job.record.lock().unwrap().native_move.clone();
        if let Some(receipt) = receipt
            && let Err(error) = finish_remote_move(core.clone(), job.clone(), receipt).await
        {
            {
                let mut r = job.record.lock().unwrap();
                if r.move_outcome.is_none() {
                    r.view.phase = Phase::Paused;
                    r.view.pause_reason = Some(PauseReason::Connection);
                    r.view.error = Some(format!(
                        "Move confirmation will resume when the source reconnects: {error:#}"
                    ));
                    if let Some(paste) = &mut r.view.paste {
                        paste.stage = PasteStage::RemovingOriginals;
                    }
                }
            }
            core.wake();
            let _ = save(core.clone(), job.clone()).await;
        }
        *job.retry.lock().unwrap() = Instant::now() + RETRY;
        job.control_running.store(false, Ordering::Release);
    });
}

pub(super) async fn finish_remote_move(
    core: Arc<Core>,
    job: Arc<Job>,
    receipt: MoveReceipt,
) -> Result<()> {
    let record = job.record.lock().unwrap().clone();
    if record.move_outcome.is_some() {
        return Ok(());
    }
    let mut pipe = pipe(&core, &record.view.peer, &record.workspace).await?;
    pipe.send(&Packet::NativeMoveCommitted {
        offer: record
            .offer
            .context("The native Move offer is unavailable")?,
        id: record.export,
        manifest: record
            .manifest
            .context("The native Move manifest is unavailable")?
            .digest()?,
        paste: receipt.paste,
        consumer: receipt.consumer,
        entries: receipt.entries,
    })
    .await?;
    let outcome = loop {
        match pipe.receive().await? {
            Packet::Working => continue,
            Packet::MoveFinalized { outcome } => break outcome,
            Packet::Error(error) => anyhow::bail!("{error}"),
            _ => anyhow::bail!("The source could not confirm the Move"),
        }
    };
    {
        let mut r = job.record.lock().unwrap();
        r.move_outcome = Some(outcome);
        r.view.phase = Phase::Completed;
        r.view.pause_reason = None;
        if let Some(paste) = &mut r.view.paste {
            paste.stage = match outcome {
                protocol::MoveOutcome::Moved => PasteStage::Moved,
                protocol::MoveOutcome::OriginalsRetained | protocol::MoveOutcome::NeedsRecovery => {
                    PasteStage::OriginalsRetained
                }
            };
        }
        r.view.error = (outcome != protocol::MoveOutcome::Moved).then(|| {
            if outcome == protocol::MoveOutcome::NeedsRecovery {
                "The files were copied. The Move needs attention on the source computer."
            } else {
                "The files were copied; the originals were kept on the source computer."
            }
            .into()
        });
    }
    core.wake();
    save(core, job).await
}

pub(super) struct SourceMoveRequest {
    pub(super) offer: String,
    pub(super) id: String,
    pub(super) digest: String,
    pub(super) paste: String,
    pub(super) consumer: NativeConsumer,
    pub(super) entries: Vec<u32>,
}

pub(super) async fn source_move(
    core: Arc<Core>,
    peer: String,
    request: SourceMoveRequest,
    pipe: &mut transport::Connection,
) -> Result<()> {
    let SourceMoveRequest {
        offer: offer_id,
        id,
        digest,
        paste,
        consumer,
        entries,
    } = request;
    protocol::valid_id(&id)?;
    protocol::valid_id(&paste)?;
    consumer.validate()?;
    let job = core
        .jobs
        .lock()
        .unwrap()
        .get(&job_id(&id, &peer))
        .cloned()
        .context("The source Move transfer is unavailable")?;
    let mut heartbeat = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(2),
        Duration::from_secs(2),
    );
    // Validate the latest record only after acquiring the per-job finalizer.
    // A duplicate receipt waits for the first durable outcome, never races a
    // recovery read against the first journal creation.
    let _finalizer = loop {
        tokio::select! {
            guard = job.move_finalizer.lock() => break guard,
            _ = heartbeat.tick() => { pipe.send(&Packet::Working).await?; }
        }
    };
    let record = job.record.lock().unwrap().clone();
    ensure!(
        core.allowed(&record.workspace, &peer, Origin::Send)
            && record.view.direction == Direction::Sending
            && record.operation == Operation::Move
            && record.offer.as_ref() == Some(&offer_id),
        "This transfer has no source Cut authority"
    );
    let manifest = record
        .manifest
        .context("The source files have not been prepared")?;
    ensure!(
        manifest.digest()? == digest,
        "The native Move receipt names different files"
    );
    let indices = entries.iter().copied().collect::<BTreeSet<_>>();
    ensure!(
        indices.len() == entries.len() && indices.into_iter().eq(0..manifest.entries.len() as u32),
        "A native Move must confirm every selected file and folder"
    );
    let receipt = MoveReceipt {
        paste: paste.clone(),
        consumer,
        entries,
    };
    if let Some(existing) = &record.native_move {
        ensure!(
            existing.paste == receipt.paste
                && existing.consumer == receipt.consumer
                && existing.entries == receipt.entries,
            "A different native Move already claimed these files"
        );
    }
    if let Some(outcome) = record.move_outcome {
        pipe.send(&Packet::MoveFinalized { outcome }).await?;
        return Ok(());
    }
    // A lease or an old Cut URI cannot acquire source deletion authority after
    // retirement. Such a paste remains a successful Copy with originals kept.
    let Some(grant) = record
        .move_grant
        .filter(|_| !job.cancel.load(Ordering::Acquire))
    else {
        {
            let mut r = job.record.lock().unwrap();
            r.native_move = Some(receipt);
            r.move_outcome = Some(protocol::MoveOutcome::OriginalsRetained);
            r.view.phase = Phase::Completed;
            r.view.error = Some("The files were copied; the original Cut was no longer current, so its originals were kept.".into());
            if let Some(paste) = &mut r.view.paste {
                paste.stage = PasteStage::OriginalsRetained;
            }
        }
        save(core.clone(), job.clone()).await?;
        core.wake();
        pipe.send(&Packet::MoveFinalized {
            outcome: protocol::MoveOutcome::OriginalsRetained,
        })
        .await?;
        return Ok(());
    };
    let source = core.lazy.lock().unwrap().locals.get(&offer_id).cloned();
    let binding = if let Some(binding) = record.move_binding {
        ensure!(
            binding.paste == paste && binding.manifest_digest == digest,
            "The source Move binding changed"
        );
        binding
    } else {
        storage::source_move::SourceMoveBinding {
            workspace: record.workspace.clone(),
            peer: peer.clone(),
            offer: offer_id.clone(),
            paste,
            export: id,
            manifest_digest: digest,
            cut_grant: grant.cut_grant,
            cut_roots: grant.cut_roots,
        }
    };
    {
        let mut r = job.record.lock().unwrap();
        r.native_move = Some(receipt);
        r.move_binding = Some(binding.clone());
        r.view.phase = Phase::Verifying;
        if let Some(paste) = &mut r.view.paste {
            paste.stage = PasteStage::RemovingOriginals;
        }
    }
    save(core.clone(), job.clone()).await?;
    core.wake();
    let dir = core.dir.clone();
    let saved_binding = binding.clone();
    let progress_core = core.clone();
    let progress_job = job.clone();
    let mut task = tokio::task::spawn_blocking(move || {
        if let Some(source) = &source {
            source.active.store(false, Ordering::Release);
            let selected = source.selection.lock().unwrap().take();
            if let Some(selected) = selected {
                return selected.finalize_move_with_progress(
                    &dir,
                    &manifest,
                    &saved_binding,
                    |_, _, name| {
                        progress_job.record.lock().unwrap().view.current_name =
                            Some(name.to_owned());
                        progress_core.wake();
                    },
                );
            }
        }
        storage::source_move::recover_source_move(&dir, &saved_binding)
    });
    let result = loop {
        tokio::select! {
            result = &mut task => break result?,
            _ = heartbeat.tick() => { let _ = pipe.send(&Packet::Working).await; }
        }
    };
    let (outcome, locations, error) = match result {
        Ok(result) => (
            match result.state {
                storage::source_move::SourceMoveState::Moved => protocol::MoveOutcome::Moved,
                storage::source_move::SourceMoveState::OriginalsRetained => {
                    protocol::MoveOutcome::OriginalsRetained
                }
                storage::source_move::SourceMoveState::NeedsRecovery => {
                    protocol::MoveOutcome::NeedsRecovery
                }
            },
            result.locations,
            None,
        ),
        // An I/O error can occur after detach. It does not prove that originals
        // are still at their old names; retain the journal for source recovery.
        Err(error) => (
            protocol::MoveOutcome::NeedsRecovery,
            vec![],
            Some(format!("{error:#}")),
        ),
    };
    {
        let mut r = job.record.lock().unwrap();
        r.move_outcome = Some(outcome);
        r.view.phase = Phase::Completed;
        r.view.error = error;
        r.view.current_name = None;
        r.view.recovery_paths = locations
            .iter()
            .filter_map(|location| location.recovery.clone())
            .collect();
        if let Some(paste) = &mut r.view.paste {
            paste.stage = match outcome {
                protocol::MoveOutcome::Moved => PasteStage::Moved,
                _ => PasteStage::OriginalsRetained,
            };
        }
    }
    save(core.clone(), job.clone()).await?;
    core.wake();
    retire_consumed(&core, &offer_id, outcome == protocol::MoveOutcome::Moved);
    pipe.send(&Packet::MoveFinalized { outcome }).await?;
    Ok(())
}

fn retire_consumed(core: &Arc<Core>, id: &str, moved: bool) {
    let mut state = core.lazy.lock().unwrap();
    let source = state.locals.get(id).cloned();
    if let Some(source) = &source {
        source.active.store(false, Ordering::Release);
        *source.retired.lock().unwrap() = Some(Instant::now());
        *source.move_claim.lock().unwrap() = None;
    }
    if state.local.as_deref() == Some(id) {
        state.local = None;
    }
    drop(state);
    if moved && let Some(source) = source {
        tokio::task::spawn_blocking(move || {
            let _ =
                crate::group::clipboard_files::clear_cut_if_current(&source.paths, source.sequence);
        });
    }
    let trust = core.trust.lock().unwrap().clone();
    for peer in trust.peers.keys().cloned().collect::<Vec<_>>() {
        let core = core.clone();
        let id = id.to_owned();
        let workspace = trust.workspace_id.clone();
        tokio::spawn(async move {
            if let Ok(mut pipe) = pipe(&core, &peer, &workspace).await {
                let _ = pipe.send(&Packet::RetireOffer { id }).await;
            }
        });
    }
}
