//! Majority-committed control, with independent pinned links between members.
use super::*;
use crate::group::{
    consensus::{Effect, Engine, HardState},
    model::Authority,
};
use std::time::Instant;

const DURABLE: &str = "workspace-consensus.json";
const ADMISSION: &str = "workspace-admission.json";
const LOCATIONS: &str = "workspace-locations.json";
const MAX_QUEUED: usize = 128;

#[derive(Debug)]
struct LocalStateFailure(anyhow::Error);
impl std::fmt::Display for LocalStateFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Workspace storage failed: {:#}", self.0)
    }
}
impl std::error::Error for LocalStateFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}
fn local_failure(error: anyhow::Error) -> anyhow::Error {
    LocalStateFailure(error).into()
}

fn validate_workspace_data(
    data: &[u8],
    workspace_id: &str,
    roster: Option<&BTreeSet<String>>,
) -> Result<Workspace> {
    let state: Workspace =
        serde_json::from_slice(data).context("Invalid replicated workspace data")?;
    state
        .validate()
        .context("Invalid replicated workspace data")?;
    ensure!(
        state.workspace_id() == workspace_id,
        "Replicated workspace identity changed"
    );
    if let Some(roster) = roster {
        ensure!(
            state.members.keys().cloned().collect::<BTreeSet<_>>() == *roster,
            "Replicated workspace membership does not match its committed configuration"
        );
    }
    Ok(state)
}

fn validate_image(image: &HardState) -> Result<Workspace> {
    image.validate()?;
    ensure!(
        image.voted_for.is_none()
            && image.learners.is_empty()
            && image.entries.is_empty()
            && image.commit_index == image.snapshot_index,
        "A workspace bootstrap must contain only a committed snapshot"
    );
    validate_workspace_data(
        &image.snapshot,
        &image.workspace_id,
        Some(image.configuration.state_members()),
    )
}

fn restore_workspace(engine: &Engine) -> Result<Workspace> {
    let (index, term, data) = engine.committed();
    let mut state: Workspace = serde_json::from_slice(data)?;
    // A proposal embeds the previous authority, and an election barrier can
    // commit the legacy snapshot as-is. Restore the actual committed stamp.
    state.authority = (index > 0).then(|| Authority {
        workspace_id: engine.hard_state().workspace_id.clone(),
        term,
        index,
    });
    Ok(state)
}

fn validate_rpc_data(engine: &Engine, rpc: &crate::group::consensus::Rpc) -> Result<()> {
    use crate::group::consensus::Message;
    ensure!(
        rpc.workspace_id == engine.hard_state().workspace_id,
        "A different workspace sent a consensus message"
    );
    match &rpc.message {
        Message::Append {
            prev_index,
            prev_term,
            entries,
            ..
        } => {
            if entries.is_empty() {
                return Ok(());
            }
            let hard = engine.hard_state();
            let prefix_term = if *prev_index == hard.snapshot_index {
                Some(hard.snapshot_term)
            } else {
                hard.entries
                    .iter()
                    .find(|entry| entry.index == *prev_index)
                    .map(|entry| entry.term)
            };
            let mut roster = if prefix_term == Some(*prev_term) {
                let mut keys = validate_workspace_data(&hard.snapshot, &hard.workspace_id, None)?
                    .members
                    .into_keys()
                    .collect::<BTreeSet<_>>();
                for entry in hard
                    .entries
                    .iter()
                    .filter(|entry| entry.index <= *prev_index)
                {
                    if let Some(configuration) = &entry.configuration {
                        keys = configuration.state_members().clone();
                    }
                }
                Some(keys)
            } else {
                // The engine will NACK an unknown/mismatched prefix. Validate
                // payload shape now without inventing its missing roster.
                None
            };
            for entry in entries {
                if entry.barrier {
                    ensure!(
                        entry.data.is_empty() && entry.configuration.is_none(),
                        "Invalid workspace election barrier"
                    );
                    continue;
                }
                if let Some(configuration) = &entry.configuration {
                    roster = Some(configuration.state_members().clone());
                }
                validate_workspace_data(&entry.data, &rpc.workspace_id, roster.as_ref())?;
            }
        }
        Message::Snapshot {
            configuration,
            data,
            ..
        } => {
            validate_workspace_data(data, &rpc.workspace_id, Some(configuration.state_members()))?;
        }
        Message::StageJoint { entry, .. } => {
            ensure!(!entry.barrier, "Invalid membership seed barrier");
            let configuration = entry
                .configuration
                .as_ref()
                .context("A membership seed is missing its voter roster")?;
            validate_workspace_data(
                &entry.data,
                &rpc.workspace_id,
                Some(configuration.state_members()),
            )?;
        }
        _ => {}
    }
    Ok(())
}

#[derive(Clone, Eq, PartialEq)]
struct DurableStamp {
    term: u64,
    vote: Option<String>,
    commit: u64,
    base: (u64, u64),
    tail: Option<(u64, u64)>,
    length: usize,
    configuration: crate::group::consensus::Configuration,
    learners: BTreeSet<String>,
}
impl From<&HardState> for DurableStamp {
    fn from(value: &HardState) -> Self {
        Self {
            term: value.term,
            vote: value.voted_for.clone(),
            commit: value.commit_index,
            base: (value.snapshot_index, value.snapshot_term),
            tail: value.entries.last().map(|entry| (entry.index, entry.term)),
            length: value.entries.len(),
            configuration: value.configuration.clone(),
            learners: value.learners.clone(),
        }
    }
}

/// A vote/log acknowledgement must follow durable storage, not a queued write.
fn durable_save(dir: &Path, hard: &HardState) -> Result<()> {
    durable_write(dir, DURABLE, &serde_json::to_string(hard)?)
}

fn durable_write(dir: &Path, name: &str, contents: &str) -> Result<()> {
    let path = dir.join(name);
    #[cfg(windows)]
    {
        use std::io::Write;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            #[link_name = "MoveFileExW"]
            fn move_file_ex_w(from: *const u16, to: *const u16, flags: u32) -> i32;
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let temporary = dir.join(format!(
            ".workspace-vote-{}-{nonce}.tmp",
            std::process::id()
        ));
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)?;
            file.write_all(contents.as_bytes())?;
            file.sync_all()?;
            drop(file);
            use std::os::windows::ffi::OsStrExt;
            let source = temporary
                .as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>();
            let destination = path
                .as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>();
            // Same-directory atomic replace, with the move completed on disk
            // before any vote/log acknowledgement leaves this process.
            ensure!(
                // SAFETY: both paths are NUL-terminated UTF-16 buffers that outlive the
                // call, and the flags request a replace inside the same directory.
                unsafe { move_file_ex_w(source.as_ptr(), destination.as_ptr(), 0x1 | 0x8) } != 0,
                "Saving workspace voting state: {}",
                std::io::Error::last_os_error()
            );
            Ok::<(), anyhow::Error>(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result?;
    }
    #[cfg(not(windows))]
    config::write_atomic(&path, contents)?;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)?
        .sync_all()?;
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
}

#[derive(Deserialize, Serialize)]
struct Admission {
    own: String,
    workspace_id: String,
}
fn admission(dir: &Path, own: &str) -> Result<Option<String>> {
    let path = dir.join(ADMISSION);
    match std::fs::metadata(&path) {
        Ok(metadata) => ensure!(
            metadata.is_file() && metadata.len() <= 4096,
            "Invalid workspace admission record"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let record: Admission = serde_json::from_slice(&std::fs::read(path)?)?;
    model::valid_key(&record.workspace_id)?;
    ensure!(
        record.own == own,
        "Workspace admission belongs to a different identity"
    );
    Ok(Some(record.workspace_id))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Coordination {
    leader: Option<String>,
    writable: bool,
    online: usize,
    quorum: usize,
}

struct Control {
    own: String,
    dir: PathBuf,
    identity: config::Identity,
    local: Member,
    canonical: Workspace,
    visible: Workspace,
    observed: BTreeMap<String, Member>,
    locations: BTreeMap<String, SocketAddr>,
    engine: Option<Engine>,
    saved: Option<DurableStamp>,
    admission: Option<String>,
    links: Links,
    internal: Internal,
    notify: Notify,
    receipts: UpdateReceipts,
    actions: VecDeque<(String, Action)>,
    additions: VecDeque<Member>,
    joining: Option<(String, String)>,
    last_coordination: Option<Coordination>,
    clock: Instant,
    connected_once: bool,
    state_dirty: bool,
    auto_dirty: bool,
    directory_snapshot: BTreeMap<String, SocketAddr>,
    last_reconnect: BTreeMap<String, String>,
    #[cfg(test)]
    fail_commit_persist: bool,
}
impl Control {
    fn now(&self) -> u64 {
        self.clock.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
    }
    fn trusted(&self) -> BTreeSet<String> {
        self.engine
            .as_ref()
            .map(Engine::members)
            .unwrap_or_else(|| self.canonical.members.keys().cloned().collect())
    }
    fn workspace_id(&self) -> &str {
        self.engine
            .as_ref()
            .map(|engine| engine.hard_state().workspace_id.as_str())
            .unwrap_or_else(|| self.canonical.workspace_id())
    }
    fn handshake(&self) -> Workspace {
        let mut state = self.canonical.clone();
        if let Some(engine) = &self.engine {
            state.authority = Some(Authority {
                workspace_id: engine.hard_state().workspace_id.clone(),
                term: engine.hard_state().term.max(1),
                index: engine.hard_state().commit_index.max(1),
            });
        }
        state
            .members
            .entry(self.own.clone())
            .or_insert_with(|| self.local.clone());
        if let Some(engine) = &self.engine {
            let trusted = engine.members();
            for entry in engine.hard_state().entries.iter().rev() {
                if trusted.iter().all(|key| state.members.contains_key(key)) {
                    break;
                }
                if let Ok(pending) = serde_json::from_slice::<Workspace>(&entry.data) {
                    for (key, member) in pending.members {
                        if trusted.contains(&key) {
                            state.members.entry(key).or_insert(member);
                        }
                    }
                }
            }
            for key in trusted {
                if let Some(member) = self.observed.get(&key) {
                    state.members.entry(key).or_insert_with(|| member.clone());
                }
            }
        }
        for member in &self.additions {
            state
                .members
                .entry(member.node.key.clone())
                .or_insert_with(|| member.clone());
        }
        if let Some(leader) = self.engine.as_ref().and_then(Engine::leader) {
            state.hub = leader.to_owned();
        }
        state
    }
    fn persist(&mut self) -> Result<()> {
        if let Some(engine) = &self.engine {
            if self.admission.as_deref() != Some(engine.hard_state().workspace_id.as_str()) {
                let record = Admission {
                    own: self.own.clone(),
                    workspace_id: engine.hard_state().workspace_id.clone(),
                };
                durable_write(&self.dir, ADMISSION, &serde_json::to_string(&record)?)
                    .map_err(local_failure)?;
                self.admission = Some(record.workspace_id);
            }
            let stamp = DurableStamp::from(engine.hard_state());
            if self.saved.as_ref() != Some(&stamp) {
                #[cfg(test)]
                if self.fail_commit_persist
                    && self
                        .saved
                        .as_ref()
                        .is_some_and(|previous| stamp.commit > previous.commit)
                {
                    self.fail_commit_persist = false;
                    return Err(local_failure(
                        std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            "Injected workspace commit storage failure",
                        )
                        .into(),
                    ));
                }
                durable_save(&self.dir, engine.hard_state()).map_err(local_failure)?;
                self.saved = Some(stamp);
            }
        }
        Ok(())
    }
    fn effects(&mut self, result: Result<Vec<Effect>>) -> Result<()> {
        // Also persist a higher term observed by an operation that rejected a
        // malformed packet. No outgoing effect may precede this write.
        self.persist()?;
        for effect in result? {
            match effect {
                Effect::Send { target, rpc } => {
                    // A missing/busy link is retried by the next heartbeat;
                    // dropping an RPC never acknowledges or commits a write.
                    let _ = send(&self.links, &target, Packet::Consensus(rpc));
                }
                Effect::Committed { index, term, data } => {
                    (|| -> Result<()> {
                        let mut state: Workspace = serde_json::from_slice(&data)?;
                        ensure!(
                            state.workspace_id() == self.workspace_id(),
                            "Workspace identity changed in committed data"
                        );
                        if let Some(leader) = self.engine.as_ref().and_then(Engine::leader) {
                            ensure!(
                                state.members.contains_key(leader),
                                "Committed coordinator is not a member"
                            );
                            state.hub = leader.to_owned();
                        }
                        state.authority = Some(Authority {
                            workspace_id: self.workspace_id().to_owned(),
                            term,
                            index,
                        });
                        state.validate()?;
                        self.canonical = state;
                        self.state_dirty = true;
                        self.auto_dirty = true;
                        self.additions.retain(|member| {
                            !self.canonical.members.contains_key(&member.node.key)
                        });
                        if self.canonical.members.contains_key(&self.own) {
                            self.joining = None;
                            save(&self.dir, &self.canonical)?;
                        }
                        Ok(())
                    })()
                    .map_err(local_failure)?;
                }
                Effect::Leadership { .. } => {}
            }
        }
        Ok(())
    }
    fn proposal_effects(
        &mut self,
        result: Result<Vec<Effect>>,
        source: Option<&str>,
    ) -> Result<bool> {
        // Losing a lease or rejecting a user's oversized proposal is a normal
        // operation failure. A durable-store failure still stops the voter.
        self.persist()?;
        match result {
            Ok(effects) => {
                self.effects(Ok(effects))?;
                Ok(true)
            }
            Err(error) => {
                if let Some(source) = source {
                    self.user_error(source, error);
                }
                Ok(false)
            }
        }
    }
    fn refresh(&mut self) -> Result<()> {
        if let Some(engine) = &self.engine
            && engine.committed().1 > 0
            && engine.committed().1 == engine.hard_state().term
            && let Some(leader) = engine.leader()
            && self.canonical.members.contains_key(leader)
            && (self.canonical.hub != leader
                || self
                    .canonical
                    .authority
                    .as_ref()
                    .is_none_or(|authority| authority.index != engine.committed().0))
        {
            self.canonical.hub = leader.to_owned();
            self.canonical.authority = Some(Authority {
                workspace_id: engine.hard_state().workspace_id.clone(),
                term: engine.committed().1,
                index: engine.committed().0,
            });
            self.state_dirty = true;
        }
        if !self.state_dirty {
            return Ok(());
        }
        self.state_dirty = false;
        if !self.canonical.members.contains_key(&self.own) {
            return Ok(());
        }
        let trusted = self.trusted();
        self.links.active.retain(|key, _| {
            trusted.contains(key)
                || self.joining.as_ref().is_some_and(|(peer, _)| peer == key)
                || self.additions.iter().any(|member| member.node.key == *key)
        });
        self.links
            .retired
            .retain(|key, _| self.links.active.contains_key(key));
        let mut state = self.canonical.clone();
        let confirmation = state.confirmed_monitor.clone();
        for (key, observation) in &self.observed {
            if state.members.contains_key(key) {
                let mut node = observation.node.clone();
                node.monitor_error = state.members[key].node.monitor_error.clone();
                state.update_node(key, node, observation.native_output, 0)?;
            }
        }
        // Raw device observations cannot themselves authorize a global change.
        // The leader commits a binding invalidation in drive() below.
        state.confirmed_monitor = confirmation;
        for (key, member) in &mut state.members {
            member.online = key == &self.own || self.links.contains_key(key);
            if let Some(address) = self.locations.get(key) {
                member.address = *address;
            }
        }
        state.validate()?;
        if state != self.visible {
            self.visible = state;
            self.notify.state(&self.visible);
        }
        Ok(())
    }
    fn coordination(&mut self) {
        let trusted = self.trusted();
        let leader = self
            .engine
            .as_ref()
            .and_then(Engine::leader)
            .map(str::to_owned);
        let writable = self.engine.as_ref().is_some_and(|engine| {
            if !self.canonical.members.contains_key(&self.own) {
                return false;
            }
            if engine.leader() == Some(self.own.as_str()) {
                return engine.writable();
            }
            engine
                .leader()
                .is_some_and(|key| self.links.contains_key(key))
                && engine.hard_state().term > 0
                && engine.committed().1 == engine.hard_state().term
        });
        let online = usize::from(trusted.contains(&self.own))
            + self
                .links
                .keys()
                .filter(|key| trusted.contains(*key))
                .count();
        let status = Coordination {
            leader,
            writable,
            online,
            quorum: self
                .engine
                .as_ref()
                .map(|engine| engine.voters().len() / 2 + 1)
                .unwrap_or(trusted.len() / 2 + 1),
        };
        if self.last_coordination.as_ref() != Some(&status) {
            self.notify.event(Event::Coordination {
                leader: status.leader.clone(),
                writable: status.writable,
                online: status.online,
                quorum: status.quorum,
            });
            self.last_coordination = Some(status);
        }
    }
    fn user_error(&self, source: &str, error: anyhow::Error) {
        let text = workspace::management::status_text(format!("{error:#}"), 1024);
        if source == self.own {
            self.notify.event(Event::Error(text));
        } else {
            let _ = send(&self.links, source, Packet::Error(text));
        }
    }
    fn leader_ready(&self) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.leader() == Some(self.own.as_str()) && engine.writable())
    }
    fn directory(&mut self) {
        if !self.canonical.members.contains_key(&self.own) {
            return;
        }
        let locations: BTreeMap<_, _> = self
            .canonical
            .members
            .iter()
            .filter(|(key, _)| *key == &self.own || self.links.contains_key(*key))
            .map(|(key, member)| {
                (
                    key.clone(),
                    self.locations.get(key).copied().unwrap_or(member.address),
                )
            })
            .collect();
        let changed = self.directory_snapshot != locations;
        if changed {
            self.directory_snapshot = locations;
        }
        for (key, link) in &mut self.links.active {
            if changed {
                link.directory_queued = false;
            }
            // Retry on the next event or tick when a session's queue is full.
            // Reserving first avoids cloning a map that cannot be queued yet.
            if !link.directory_queued
                && self.directory_snapshot.contains_key(key)
                && let Ok(permit) = link.commands.try_reserve()
            {
                permit.send(Packet::Directory(self.directory_snapshot.clone()));
                link.directory_queued = true;
            }
        }
    }
    fn observation(&self) -> Member {
        let mut local = self.local.clone();
        local.input_ready = 0;
        local.address = self
            .locations
            .get(&self.own)
            .copied()
            .unwrap_or(local.address);
        local
    }
    fn announce(&self) {
        let packet = Arc::new(Packet::Observation(Box::new(self.observation())));
        for link in self.links.values() {
            link.snapshot.send_replace(Some(packet.clone()));
        }
    }
    fn remember(&mut self, mut member: Member) -> Result<()> {
        let key = member.node.key.clone();
        ensure!(
            self.trusted().contains(&key)
                || self.joining.as_ref().is_some_and(|(peer, _)| peer == &key)
                || self.additions.iter().any(|member| member.node.key == key),
            "Unknown workspace member"
        );
        member.input_ready = 0;
        member.validate(&self.canonical.hub)?;
        if let Some(address) = self.locations.get(&key) {
            member.address = *address;
        }
        self.observed.insert(key, member);
        self.state_dirty = true;
        self.auto_dirty = true;
        Ok(())
    }
    fn save_locations(&self) -> Result<()> {
        config::write_atomic(
            &self.dir.join(LOCATIONS),
            &serde_json::to_string(&self.locations)?,
        )
        .map_err(local_failure)
    }
    fn connection_error(&mut self, key: String, error: anyhow::Error, outgoing: bool) {
        if let Some(mismatch) = error.downcast_ref::<WorkspaceProtocolMismatch>() {
            if !outgoing && !self.trusted().contains(&mismatch.key) {
                return;
            }
            let reason = mismatch.to_string();
            let previous = self
                .trusted()
                .contains(&mismatch.key)
                .then(|| {
                    self.last_reconnect
                        .insert(mismatch.key.clone(), reason.clone())
                })
                .flatten();
            if key.is_empty() || previous.as_deref() != Some(&reason) {
                self.notify.event(Event::IncompatibleWorkspace {
                    key: mismatch.key.clone(),
                    address: mismatch.address,
                    version: mismatch.version.clone(),
                });
            }
            return;
        }
        if key.is_empty() {
            if outgoing {
                self.user_error(&self.own, error);
            }
            return;
        }
        let event = closure_event(key.clone(), self.trusted().contains(&key), error);
        let changed = match &event {
            Event::Reconnecting { key, reason } => {
                self.last_reconnect
                    .insert(key.clone(), reason.clone())
                    .as_ref()
                    != Some(reason)
            }
            _ => true,
        };
        if changed {
            self.notify.event(event);
        }
    }
    fn dial_saved(&self) {
        let enabled = self.connected_once
            || self
                .local
                .node
                .management
                .as_ref()
                .is_none_or(|management| management.application.auto_connect);
        if !enabled {
            return;
        }
        let state = self.handshake();
        for key in self
            .trusted()
            .into_iter()
            .filter(|key| key != &self.own && !self.links.contains_key(key))
        {
            let address = self
                .locations
                .get(&key)
                .copied()
                .or_else(|| state.members.get(&key).map(|member| member.address));
            if let Some(address) = address {
                dial(
                    &self.identity,
                    &self.local,
                    &state,
                    (address, key),
                    Intent::Mesh,
                    self.internal.clone(),
                    None,
                );
            }
        }
    }
    fn bootstrap(&mut self, key: &str, image: HardState) -> Result<()> {
        let mut canonical = validate_image(&image)?;
        let cross_workspace = image.workspace_id != self.workspace_id();
        let invited = self
            .joining
            .as_ref()
            .is_some_and(|(peer, id)| peer == key && id == &image.workspace_id);
        ensure!(
            !cross_workspace || invited,
            "A different workspace cannot replace this membership without an invitation"
        );
        if !invited {
            ensure!(self.trusted().contains(key), "Unknown bootstrap sender");
            ensure!(
                image.workspace_id == self.workspace_id(),
                "Workspace bootstrap identity changed"
            );
            if image.commit_index == 0 && key != self.canonical.workspace_id() {
                // Saved peers may race the first original-hub initialization.
                // Their uncommitted directory grants no authority or new keys.
                return Ok(());
            }
        }
        if let Some(engine) = &mut self.engine
            && !cross_workspace
        {
            if !engine.hard_state().configuration.members().contains(key) {
                // A locally pending learner cannot prove a committed prefix.
                // If it has since been admitted elsewhere, ordinary Append
                // advances this follower through the locally trusted log.
                return Ok(());
            }
            if image.commit_index < engine.committed().0 {
                return Ok(());
            }
            let result = engine.catch_up(key, image, self.clock.elapsed().as_millis() as u64);
            return self.effects(result);
        }
        ensure!(
            !(cross_workspace || invited) || !self.canonical.switch_pending(),
            "Wait for the pending monitor command before joining another workspace"
        );
        if image.commit_index > 0 {
            canonical.authority = Some(Authority {
                workspace_id: image.workspace_id.clone(),
                term: image.snapshot_term,
                index: image.commit_index,
            });
        }
        canonical.validate()?;
        self.canonical = canonical;
        self.state_dirty = true;
        self.auto_dirty = true;
        self.engine = Some(Engine::new(self.own.clone(), image, self.now())?);
        self.saved = None;
        self.persist()?;
        Ok(())
    }
    fn ready(
        &mut self,
        candidate: Candidate,
        mut member: Member,
        controller: bool,
        mesh: bool,
        workspace_id: String,
    ) -> Result<()> {
        let key = member.node.key.clone();
        if !self.links.accepts(&key, candidate.connection)? {
            return Ok(());
        }
        let mut local_address = canonical_address(candidate.stream.local_addr()?);
        local_address.set_port(self.local.address.port());
        self.local.address = local_address;
        self.locations.insert(self.own.clone(), local_address);
        let address = canonical_address(candidate.address);
        self.locations.insert(key.clone(), address);
        member.address = address;
        member = hello_observation(&self.canonical, member);
        if mesh {
            ensure!(
                workspace_id == self.workspace_id() && self.trusted().contains(&key),
                "The computer is not a saved member of this workspace"
            );
        } else if controller {
            ensure!(
                self.leader_ready(),
                "Reconnect a majority of saved computers before adding another computer"
            );
            ensure!(
                !self.canonical.members.contains_key(&key),
                "This computer is already a member of the workspace"
            );
            ensure!(
                self.additions.len() < 16,
                "Too many pending workspace invitations"
            );
        } else {
            ensure!(
                self.canonical.members.len() <= 1,
                "Leave the existing workspace before joining another"
            );
            ensure!(
                !self.canonical.switch_pending(),
                "Wait for the pending monitor command before joining another workspace"
            );
            self.joining = Some((key.clone(), workspace_id));
        }
        let link = start_session(candidate, self.internal.clone());
        ensure!(
            self.links.install(key.clone(), link)?,
            "Workspace handover was superseded"
        );
        self.connected_once = true;
        self.last_reconnect.remove(&key);
        self.state_dirty = true;
        self.auto_dirty = true;
        self.observed.insert(key.clone(), member.clone());
        let _ = send(
            &self.links,
            &key,
            Packet::Observation(Box::new(self.observation())),
        );
        if (mesh || controller)
            && let Some(engine) = &self.engine
            && (engine.committed().0 > 0 || self.own == self.workspace_id())
            && engine
                .hard_state()
                .configuration
                .members()
                .contains(&self.own)
        {
            let _ = send(
                &self.links,
                &key,
                Packet::Bootstrap(Box::new(engine.export())),
            );
            if let Some(seed) = engine.export_pending_joint()
                && matches!(
                    &seed.message,
                    crate::group::consensus::Message::StageJoint { recipient, .. }
                        if recipient == &key
                )
            {
                // A committed prefix precedes its single consented joint
                // entry on the same authenticated stream. Neither packet
                // grants application membership before the quorum commits.
                let _ = send(&self.links, &key, Packet::Consensus(seed));
            }
        }
        if !mesh && controller {
            self.additions.push_back(member);
        }
        self.save_locations()?;
        self.directory();
        Ok(())
    }
    fn queue_global(&mut self, source: &str, action: Action) -> Result<()> {
        ensure!(
            self.leader_ready(),
            "Connect a majority of saved computers to change shared settings"
        );
        ensure!(
            self.actions.len() < MAX_QUEUED,
            "Workspace settings queue is busy"
        );
        self.actions.push_back((source.to_owned(), action));
        Ok(())
    }
    fn action(&mut self, source: &str, action: Action) -> Result<()> {
        ensure!(
            self.canonical.members.contains_key(source),
            "Unknown workspace member"
        );
        if let Action::Invite { address, key } = &action {
            if !key.is_empty() {
                model::valid_key(key)?;
            }
            if (!key.is_empty() && self.trusted().contains(key))
                || (key.is_empty() && !self.leader_ready() && self.canonical.members.len() > 1)
            {
                let expected = key.clone();
                let address = canonical_address(*address);
                ensure!(
                    address.port() != 0
                        && !address.ip().is_unspecified()
                        && !address.ip().is_multicast(),
                    "Invalid saved computer address"
                );
                if !expected.is_empty() && !self.links.contains_key(&expected) {
                    self.last_reconnect.remove(&expected);
                    // Keep an explicit endpoint even when an earlier cached
                    // dial currently holds this peer's single-flight permit.
                    // The next retry still authenticates the saved identity.
                    self.locations.insert(expected.clone(), address);
                    self.save_locations()?;
                    self.state_dirty = true;
                }
                dial(
                    &self.identity,
                    &self.local,
                    &self.handshake(),
                    (address, expected),
                    Intent::Mesh,
                    self.internal.clone(),
                    None,
                );
                self.connected_once = true;
                return Ok(());
            }
        }
        let global = !matches!(
            &action,
            Action::Manage { .. }
                | Action::Update { .. }
                | Action::Clipboard { .. }
                | Action::Invite { .. }
        );
        if global {
            if self.leader_ready() {
                return self.queue_global(source, action);
            }
            let leader = self
                .engine
                .as_ref()
                .and_then(Engine::leader)
                .context("Connect a majority of saved computers to change shared settings")?;
            ensure!(
                source == self.own,
                "This computer is not the active workspace coordinator"
            );
            ensure!(
                self.last_coordination
                    .as_ref()
                    .is_some_and(|status| status.writable),
                "Connect a majority of saved computers to change shared settings"
            );
            return send(&self.links, leader, Packet::Action(action));
        }
        if matches!(&action, Action::Invite { .. }) && !self.leader_ready() {
            ensure!(
                source == self.own,
                "The workspace coordinator is reconnecting"
            );
            let leader =
                self.engine.as_ref().and_then(Engine::leader).context(
                    "Connect a majority of saved computers before adding another computer",
                )?;
            return send(&self.links, leader, Packet::Action(action));
        }
        let update = if let Action::Update { target, message } = &action {
            self.receipts.permit(target, message)?;
            Some((target.clone(), message.clone()))
        } else {
            None
        };
        apply(
            action,
            source,
            &self.own,
            &mut self.visible,
            &self.links,
            &self.notify,
            &self.identity,
            &self.internal,
        )?;
        if let Some((target, message)) = update {
            if target == self.own {
                self.notify.event(Event::UpdateSent { target, message });
            } else {
                self.receipts.remember(target, message);
            }
        }
        Ok(())
    }
    fn packet(&mut self, key: &str, packet: Packet) -> Result<()> {
        match packet {
            Packet::Ping => {}
            Packet::PairingPrompt(request) => {
                ensure!(
                    self.canonical.members.contains_key(key)
                        && self.engine.as_ref().and_then(Engine::leader) == Some(key),
                    "Invalid workspace invitation sender"
                );
                ensure!(
                    request.name.len() <= 256
                        && !request.name.chars().any(char::is_control)
                        && request.code.len() <= 32
                        && !request.code.chars().any(char::is_control),
                    "Invalid workspace invitation"
                );
                self.notify.event(Event::Request(request));
            }
            Packet::PairingExpired(id) => {
                if self.canonical.members.contains_key(key) {
                    self.notify.event(Event::RequestExpired(id));
                }
            }
            Packet::Bootstrap(image) => self.bootstrap(key, *image)?,
            Packet::Consensus(rpc) => {
                ensure!(self.trusted().contains(key), "Unknown consensus sender");
                let now = self.now();
                validate_rpc_data(
                    self.engine
                        .as_ref()
                        .context("Wait for the workspace membership to initialize")?,
                    &rpc,
                )?;
                let result = self
                    .engine
                    .as_mut()
                    .context("Wait for the workspace membership to initialize")?
                    .receive(key, rpc, now);
                self.effects(result)?;
            }
            Packet::Observation(member) => {
                ensure!(member.node.key == key, "Computer identity mismatch");
                self.remember(*member)?;
            }
            Packet::Directory(directory) => {
                if !self.canonical.members.contains_key(key) {
                    return Ok(());
                }
                let mut changed = false;
                for (peer, address) in directory {
                    if peer == self.own
                        || !self.canonical.members.contains_key(&peer)
                        || self.links.contains_key(&peer)
                    {
                        continue;
                    }
                    ensure!(
                        address.port() != 0
                            && !address.ip().is_unspecified()
                            && !address.ip().is_multicast(),
                        "Invalid saved computer address"
                    );
                    let address = canonical_address(address);
                    changed |= self.locations.insert(peer, address) != Some(address);
                }
                if changed {
                    self.state_dirty = true;
                    self.save_locations()?;
                    self.dial_saved();
                }
            }
            Packet::Node(node, native, ready) => {
                // Retained as a bounded telemetry compatibility/test path. It
                // cannot carry global readiness or an authoritative snapshot.
                let mut checked = self.visible.clone();
                checked.update_node(key, *node.clone(), native, ready)?;
                let mut member = checked
                    .members
                    .get(key)
                    .context("Unknown computer")?
                    .clone();
                member.node = *node;
                self.remember(member)?;
            }
            Packet::Action(action) => self.action(key, action)?,
            Packet::Management {
                source,
                target,
                message,
            } => {
                ensure!(
                    source == key && target == self.own && self.canonical.members.contains_key(key),
                    "Invalid management sender"
                );
                message.validate()?;
                self.notify.event(Event::Management { source, message });
            }
            Packet::Update {
                source,
                target,
                message,
            } => {
                ensure!(
                    source == key && target == self.own && self.canonical.members.contains_key(key),
                    "Invalid update sender"
                );
                message.validate()?;
                send(
                    &self.links,
                    key,
                    Packet::UpdateAck {
                        source: self.own.clone(),
                        target: source.clone(),
                        message: message.clone(),
                    },
                )?;
                self.notify.event(Event::Update { source, message });
            }
            Packet::UpdateAck {
                source,
                target,
                message,
            } => {
                ensure!(
                    source == key && target == self.own,
                    "Invalid update acknowledgement"
                );
                message.validate()?;
                if self.receipts.confirm(key, &message) {
                    self.notify.event(Event::UpdateSent {
                        target: key.to_owned(),
                        message,
                    });
                }
            }
            Packet::Clipboard {
                source,
                mime,
                bytes,
                digest,
            } => {
                ensure!(
                    source == key && self.canonical.members.contains_key(key),
                    "Invalid clipboard sender"
                );
                ensure!(
                    self.visible.settings.clipboard_enabled,
                    "Shared clipboard is off"
                );
                clipboard_data(&mime, &bytes, &digest)?;
                self.notify.event(Event::Clipboard {
                    source,
                    mime,
                    bytes,
                    digest,
                });
            }
            Packet::Error(error) => {
                ensure!(
                    error.len() <= 1024 && !error.chars().any(char::is_control),
                    "Invalid peer error"
                );
                self.notify.event(Event::Error(error));
            }
            Packet::State(_) => {
                bail!("A direct workspace snapshot cannot replace majority-committed settings")
            }
            _ => bail!("Unexpected workspace message"),
        }
        Ok(())
    }
    fn drive(&mut self) -> Result<()> {
        self.refresh()?;
        if !self.leader_ready() {
            while let Some((source, _)) = self.actions.pop_front() {
                self.user_error(
                    &source,
                    anyhow::anyhow!(
                        "The workspace coordinator changed; review and retry the change"
                    ),
                );
            }
            self.coordination();
            return Ok(());
        }
        if self.engine.as_ref().is_some_and(Engine::busy) {
            self.coordination();
            return Ok(());
        }
        let now = self.now();
        if let Some(member) = self.additions.pop_front() {
            let mut candidate = self.visible.clone();
            candidate
                .members
                .insert(member.node.key.clone(), member.clone());
            candidate.revision = candidate
                .revision
                .checked_add(1)
                .filter(|value| *value < u64::MAX)
                .context("Workspace revision exhausted")?;
            candidate.validate()?;
            let result = self.engine.as_mut().unwrap().begin_add(
                member.node.key,
                serde_json::to_vec(&candidate)?,
                now,
            );
            let source = self.own.clone();
            self.proposal_effects(result, Some(&source))?;
        } else if let Some((source, action)) = self.actions.pop_front() {
            let mut candidate = self.visible.clone();
            let retiring = match &action {
                Action::RemoveMember { key } => Some(key.clone()),
                _ => None,
            };
            match apply(
                action,
                &source,
                &self.own,
                &mut candidate,
                &self.links,
                &self.notify,
                &self.identity,
                &self.internal,
            ) {
                Ok(()) => {
                    if candidate != self.visible {
                        candidate.validate()?;
                        let engine = self.engine.as_mut().unwrap();
                        let data = serde_json::to_vec(&candidate)?;
                        let result = if let Some(key) = retiring {
                            engine.begin_remove(&key, data, now)
                        } else {
                            engine.propose(data, now)
                        };
                        self.proposal_effects(result, Some(&source))?;
                    }
                }
                Err(error) => self.user_error(&source, error),
            }
        } else if self.auto_dirty || self.canonical.switch_pending() {
            self.auto_dirty = false;
            let mut candidate = self.visible.clone();
            let before = (
                candidate.settings.clone(),
                candidate.active_input.clone(),
                candidate.input_intent.clone(),
                candidate.activation,
                candidate.monitor_command.clone(),
                candidate.confirmed_monitor.clone(),
            );
            let binding_changed = self.canonical.members.iter().any(|(key, previous)| {
                self.visible.members.get(key).is_some_and(|current| {
                    previous.node.monitor.id != current.node.monitor.id
                        || previous.node.monitor.input != current.node.monitor.input
                        || previous.node.monitor.enabled != current.node.monitor.enabled
                })
            });
            if binding_changed {
                candidate.confirmed_monitor = None;
            }
            candidate.bind_microphone();
            candidate.expire_monitor();
            candidate.reconcile_input();
            let after = (
                candidate.settings.clone(),
                candidate.active_input.clone(),
                candidate.input_intent.clone(),
                candidate.activation,
                candidate.monitor_command.clone(),
                candidate.confirmed_monitor.clone(),
            );
            if before != after {
                if candidate.revision == self.visible.revision {
                    candidate.revision = candidate
                        .revision
                        .checked_add(1)
                        .filter(|value| *value < u64::MAX)
                        .context("Workspace revision exhausted")?;
                }
                candidate.validate()?;
                let result = self
                    .engine
                    .as_mut()
                    .unwrap()
                    .propose(serde_json::to_vec(&candidate)?, now);
                self.auto_dirty = !self.proposal_effects(result, None)?;
            }
        }
        self.refresh()?;
        self.coordination();
        Ok(())
    }
}

struct PendingApproval {
    owner: Option<String>,
    reply: oneshot::Sender<bool>,
}

pub(super) async fn run(
    listener: std::net::TcpListener,
    dir: PathBuf,
    state: Workspace,
    mut commands: queue::Receiver<Command>,
    status_commands: Arc<StatusCommand>,
    clipboard_commands: Arc<ClipboardCommand>,
    notify: Notify,
) -> Result<()> {
    let listener = TcpListener::from_std(listener)?;
    let identity = config::load_identity(&dir)?;
    let own = config::hex(&identity.public);
    let admission = admission(&dir, &own)?;
    let local = state.members[&own].clone();
    let (sender, mut incoming) = queue::channel(256);
    let internal = Internal {
        sender,
        dialing: Default::default(),
    };
    let mut locations = match std::fs::read(dir.join(LOCATIONS)) {
        Ok(data) => serde_json::from_slice::<BTreeMap<String, SocketAddr>>(&data)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(error) => return Err(error.into()),
    };
    locations.insert(own.clone(), local.address);
    let engine = match std::fs::read(dir.join(DURABLE)) {
        Ok(data) => {
            let hard: HardState = serde_json::from_slice(&data)?;
            ensure!(
                admission.as_ref().is_none_or(|id| id == &hard.workspace_id),
                "Workspace admission and voting records disagree; automatic recovery stopped"
            );
            Some(Engine::new(own.clone(), hard, 0)?)
        }
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                && (state.authority.is_some() || admission.is_some()) =>
        {
            bail!(
                "The workspace voting record is missing. Automatic recovery with this identity is unsafe; rejoin the workspace with a new identity."
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && state.hub == own => {
            let voters = state.members.keys().cloned().collect();
            let mut genesis = state.clone();
            if genesis.input_intent.is_none() {
                genesis.input_intent = genesis.active_input.clone();
            }
            Some(Engine::new(
                own.clone(),
                HardState::bootstrap(
                    state.workspace_id().to_owned(),
                    voters,
                    serde_json::to_vec(&genesis)?,
                )?,
                0,
            )?)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let canonical = engine
        .as_ref()
        .map(restore_workspace)
        .transpose()?
        .unwrap_or_else(|| state.clone());
    let mut control = Control {
        own: own.clone(),
        dir,
        identity,
        local: local.clone(),
        canonical,
        visible: state,
        observed: [(own.clone(), local)].into(),
        locations,
        engine,
        saved: None,
        admission,
        links: Links::default(),
        internal,
        notify,
        receipts: Default::default(),
        actions: VecDeque::new(),
        additions: VecDeque::new(),
        joining: None,
        last_coordination: None,
        clock: Instant::now(),
        connected_once: false,
        state_dirty: true,
        auto_dirty: true,
        directory_snapshot: Default::default(),
        last_reconnect: BTreeMap::new(),
        #[cfg(test)]
        fail_commit_persist: false,
    };
    control.persist()?;
    if control.engine.as_ref().is_some_and(Engine::removed) {
        control.notify.event(Event::Removed);
        return Ok(());
    }
    if let Some(engine) = &mut control.engine {
        let result = engine.tick(0);
        control.effects(result)?;
    }
    control.refresh()?;
    control.notify.state(&control.visible);
    control.coordination();
    let permits = Arc::new(tokio::sync::Semaphore::new(8));
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut reconnect = tokio::time::interval(Duration::from_secs(3));
    reconnect.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut approvals = BTreeMap::<[u8; 32], PendingApproval>::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                if let Ok(permit) = permits.clone().try_acquire_owned() {
                    let local = control.observation();
                    let state = control.handshake();
                    let internal = control.internal.clone();
                    let identity = config::Identity { private: control.identity.private, public: control.identity.public };
                    tokio::spawn(async move {
                        let _permit = permit;
                        let mut peer = String::new();
                        let result = async {
                            let candidate = timeout(AUTH, workspace::net::authenticate(stream, &identity, &local.node.name, false, None, local.address.port())).await??;
                            let key = config::hex(&candidate.key);
                            if state.members.contains_key(&key) { peer = key; }
                            setup(candidate, local, state, Intent::Invite, internal.clone(), None, None).await
                        }.await;
                        if let Err(error) = result { let _ = internal.send(Incoming::Closed(peer, None, Some(error))).await; }
                    });
                }
            }
            command = commands.recv() => match command {
                Some(Command::Disconnect) | None => break,
                #[cfg(test)]
                Some(Command::Raw(target, packet)) => { let _ = send(&control.links, &target, *packet); }
                #[cfg(test)]
                Some(Command::FailCommitPersistence(armed)) => { control.fail_commit_persist = true; let _ = armed.send(()); }
                Some(Command::Action(action)) => {
                    if let Action::ApproveInvite { id, accept } = *action {
                        if approvals.get(&id).is_some_and(|pending| pending.owner.is_none()) {
                            if let Some(pending) = approvals.remove(&id) {
                                let _ = pending.reply.send(accept);
                            }
                        } else if let Some(leader) = control.engine.as_ref().and_then(Engine::leader) {
                            let _ = send(&control.links, leader, Packet::Action(Action::ApproveInvite { id, accept }));
                        }
                        continue;
                    }
                    if let Err(error) = control.action(&own, *action) {
                        if error.downcast_ref::<LocalStateFailure>().is_some() { return Err(error); }
                        control.user_error(&own, error);
                    }
                }
            },
            _ = status_commands.changed.notified() => {
                let latest = status_commands.latest.lock().unwrap().take();
                if let Some((node, native, _ready)) = latest {
                    control.local.node = *node;
                    if native.is_some() { control.local.native_output = native; }
                    control.local.input_ready = 0;
                    control.observed.insert(own.clone(), control.local.clone());
                    control.state_dirty = true;
                    control.auto_dirty = true;
                    control.announce();
                }
            }
            _ = clipboard_commands.changed.notified() => {
                let latest = clipboard_commands.latest.lock().unwrap().take();
                if let Some(action) = latest && let Err(error) = control.action(&own, action) { control.user_error(&own, error); }
            }
            _ = tick.tick() => {
                let now = control.now();
                if let Some(engine) = &mut control.engine { let result = engine.tick(now); control.effects(result)?; }
            }
            _ = reconnect.tick() => { control.dial_saved(); }
            event = incoming.recv() => match event.context("Workspace event loop stopped")? {
                Incoming::Consent { owner, request, accept } => {
                    if approvals.len() < 16 {
                        let id = request.id;
                        if let Some(key) = &owner {
                            if !control.canonical.members.contains_key(key)
                                || send(&control.links, key, Packet::PairingPrompt(request)).is_err()
                            {
                                continue;
                            }
                        } else {
                            control.notify.event(Event::Request(request));
                        }
                        approvals.insert(id, PendingApproval { owner, reply: accept });
                    }
                }
                Incoming::Ready { candidate, member, controller, mesh, workspace_id, dial: _dial } => {
                    if let Err(error) = control.ready(candidate, *member, controller, mesh, workspace_id) {
                        if error.downcast_ref::<LocalStateFailure>().is_some() { return Err(error); }
                        control.user_error(&own, error);
                    }
                }
                Incoming::Packet(key, generation, packet) => {
                    if !control.links.packet(&key, generation) {
                        continue;
                    }
                    if let Packet::Action(Action::ApproveInvite { id, accept }) = packet {
                        if control.canonical.members.contains_key(&key)
                            && approvals.get(&id).is_some_and(|pending| pending.owner.as_ref() == Some(&key))
                            && let Some(pending) = approvals.remove(&id)
                        {
                            let _ = pending.reply.send(accept);
                        }
                        continue;
                    }
                    if let Err(error) = control.packet(&key, packet) {
                        if error.downcast_ref::<LocalStateFailure>().is_some() { return Err(error); }
                        control.user_error(&key, error);
                    }
                }
                Incoming::Closed(key, generation, error) => {
                    if let Some(generation) = generation && !close_session(&mut control.links, &mut control.visible, &mut control.receipts, &own, &key, generation) { continue; }
                    if generation.is_some() {
                        control.state_dirty = true;
                        control.auto_dirty = true;
                    }
                    if let Some(error) = error && !control.links.contains_key(&key) {
                        control.connection_error(key, error, false);
                    }
                }
                Incoming::DialFailed(key, error) => {
                    if !control.links.contains_key(&key) { control.connection_error(key, error, true); }
                }
            }
        }
        approvals.retain(|id, pending| {
            if !pending.reply.is_closed() {
                return true;
            }
            if let Some(key) = &pending.owner {
                let _ = send(&control.links, key, Packet::PairingExpired(*id));
            } else {
                control.notify.event(Event::RequestExpired(*id));
            }
            false
        });
        if control.engine.as_ref().is_some_and(Engine::removed) {
            control.notify.event(Event::Removed);
            break;
        }
        control.drive()?;
        control.directory();
    }
    control.links.shutdown();
    control.persist()?;
    if control.canonical.members.contains_key(&own) {
        save(&control.dir, &control.canonical)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::group::{
        consensus::{Entry, Message, Rpc},
        net::tests::synthetic_session,
    };

    fn test_control(state: Workspace) -> Control {
        let own = state.hub.clone();
        let local = state.members[&own].clone();
        let (events, _) = mpsc::channel();
        let (incoming, _) = queue::channel(8);
        Control {
            own: own.clone(),
            dir: PathBuf::new(),
            identity: config::Identity {
                private: [0; 32],
                public: [0; 32],
            },
            local: local.clone(),
            canonical: state.clone(),
            visible: state,
            observed: [(own, local)].into(),
            locations: Default::default(),
            engine: None,
            saved: None,
            admission: None,
            links: Default::default(),
            internal: Internal {
                sender: incoming,
                dialing: Default::default(),
            },
            notify: Notify {
                events,
                snapshot: Default::default(),
                clipboard: Default::default(),
                ctx: egui::Context::default(),
            },
            receipts: Default::default(),
            actions: Default::default(),
            additions: Default::default(),
            joining: None,
            last_coordination: None,
            clock: Instant::now(),
            connected_once: true,
            state_dirty: true,
            auto_dirty: true,
            directory_snapshot: Default::default(),
            last_reconnect: Default::default(),
            fail_commit_persist: false,
        }
    }

    #[test]
    fn directory_retries_only_blocked_sessions_with_current_addresses() {
        let mut control = test_control(model::tests::fixture(4));
        let peers = control
            .canonical
            .members
            .keys()
            .filter(|key| *key != &control.own)
            .cloned()
            .collect::<Vec<_>>();
        let blocked = &peers[0];
        let healthy = &peers[1];
        let offline = &peers[2];
        let (blocked_link, mut blocked_queue) = synthetic_session(3);
        blocked_link.commands.try_send(Packet::Ping).unwrap();
        assert!(
            control
                .links
                .install(blocked.clone(), blocked_link)
                .unwrap()
        );
        let (healthy_link, mut healthy_queue) = synthetic_session(1);
        assert!(
            control
                .links
                .install(healthy.clone(), healthy_link)
                .unwrap()
        );
        let directory = |queue: &mut queue::Receiver<Packet>| {
            let Packet::Directory(locations) = queue.try_recv().unwrap() else {
                panic!("Expected a workspace address directory");
            };
            locations
        };

        control.directory();
        let mut expected = directory(&mut healthy_queue);
        assert_eq!(expected.len(), 3);
        assert!(!expected.contains_key(offline));
        control.directory();
        assert!(healthy_queue.try_recv().is_err());

        // Supersede the first map while the other link is still blocked.
        let new_address = "192.0.2.2:45880".parse().unwrap();
        control.locations.insert(healthy.clone(), new_address);
        expected.insert(healthy.clone(), new_address);
        control.directory();
        assert_eq!(directory(&mut healthy_queue), expected);
        assert!(matches!(blocked_queue.try_recv().unwrap(), Packet::Ping));

        control.directory();
        assert_eq!(directory(&mut blocked_queue), expected);
        assert!(healthy_queue.try_recv().is_err());

        // The authenticated replacement has its own queue, even at the
        // same address. Neither the other peer nor the retired link retries.
        let (replacement, mut replacement_queue) = synthetic_session(2);
        assert!(control.links.install(blocked.clone(), replacement).unwrap());
        control.directory();
        assert_eq!(directory(&mut replacement_queue), expected);
        assert!(blocked_queue.try_recv().is_err());
        assert!(healthy_queue.try_recv().is_err());
    }

    #[test]
    fn restart_recovers_the_committed_authority_without_electing_a_coordinator() {
        let legacy = model::tests::fixture(3);
        let own = legacy.hub.clone();
        let genesis = HardState::bootstrap(
            own.clone(),
            legacy.members.keys().cloned().collect(),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        let uncommitted = Engine::new(own.clone(), genesis.clone(), 0).unwrap();
        assert_eq!(restore_workspace(&uncommitted).unwrap(), legacy);

        let mut expected = legacy;
        expected.authority = Some(Authority {
            workspace_id: own.clone(),
            term: 1,
            index: 1,
        });
        for compacted in [false, true] {
            let mut hard = genesis.clone();
            hard.term = 2;
            hard.commit_index = 1;
            if compacted {
                hard.snapshot_index = 1;
                hard.snapshot_term = 1;
            } else {
                hard.entries.push(Entry {
                    index: 1,
                    term: 1,
                    barrier: true,
                    data: Vec::new(),
                    configuration: None,
                });
            }
            let restarted = Engine::new(own.clone(), hard, 0).unwrap();
            assert_eq!(restore_workspace(&restarted).unwrap(), expected);
            assert!(restarted.leader().is_none());
            assert!(!restarted.writable());
        }
    }

    #[test]
    fn approved_queued_observation_waits_for_prior_membership_without_gaining_authority() {
        let all = model::tests::fixture(4);
        let keys = all.members.keys().cloned().collect::<Vec<_>>();
        let own = keys[0].clone();
        let mut initial = all.clone();
        initial.members.retain(|key, _| key == &own);
        let mut engine = Engine::new(
            own.clone(),
            HardState::bootstrap(
                own.clone(),
                [own.clone()].into(),
                serde_json::to_vec(&initial).unwrap(),
            )
            .unwrap(),
            0,
        )
        .unwrap();
        engine.tick(0).unwrap();
        let mut admitted = all.clone();
        admitted.members.retain(|key, _| keys[..2].contains(key));
        admitted.revision += 1;
        engine
            .begin_add(keys[1].clone(), serde_json::to_vec(&admitted).unwrap(), 1)
            .unwrap();
        engine
            .receive(
                &keys[1],
                Rpc {
                    workspace_id: own.clone(),
                    term: engine.hard_state().term,
                    message: Message::Appended {
                        probe: 1,
                        success: true,
                        matched: 2,
                        conflict: 0,
                    },
                },
                2,
            )
            .unwrap();
        assert_eq!(engine.committed().0, 2);
        assert!(engine.hard_state().configuration.joint.is_some());
        assert!(engine.busy(), "The prior admission has not finalized");
        let mut control = test_control(admitted);
        control.engine = Some(engine);
        control.additions.push_back(all.members[&keys[2]].clone());
        let mut observation = all.members[&keys[2]].clone();
        observation.node.name = "Approved pending computer".into();
        control
            .packet(&keys[2], Packet::Observation(Box::new(observation)))
            .unwrap();
        control.refresh().unwrap();
        assert!(control.observed.contains_key(&keys[2]));
        assert!(!control.trusted().contains(&keys[2]));
        assert!(!control.canonical.members.contains_key(&keys[2]));
        assert!(!control.visible.members.contains_key(&keys[2]));
        assert_eq!(control.engine.as_ref().unwrap().committed().0, 2);
        assert!(control.action(&keys[2], Action::ReturnInput).is_err());
        assert!(
            control
                .packet(
                    &keys[2],
                    Packet::Observation(Box::new(all.members[&keys[3]].clone()))
                )
                .is_err()
        );
        assert!(
            control
                .packet(
                    &keys[3],
                    Packet::Observation(Box::new(all.members[&keys[3]].clone()))
                )
                .is_err()
        );
    }

    #[test]
    fn maximum_prefix_and_pending_admission_fit_separate_workspace_packets() {
        let own = format!("{:064x}", 1);
        let learner = format!("{:064x}", 2);
        let prefix = vec![b'a'; 8 * 1024 * 1024];
        let pending = vec![b'b'; 8 * 1024 * 1024];
        let hard = HardState::bootstrap(own.clone(), [own.clone()].into(), prefix).unwrap();
        let mut holder = Engine::new(own.clone(), hard, 0).unwrap();
        holder.tick(0).unwrap();
        assert!(holder.writable());
        let image = holder.export();
        let mut recipient = Engine::new(learner.clone(), image.clone(), 0).unwrap();
        holder.begin_add(learner, pending, 1).unwrap();
        let seed = holder.export_pending_joint().unwrap();
        for packet in [
            Packet::Bootstrap(Box::new(image)),
            Packet::Consensus(seed.clone()),
        ] {
            let size = serde_json::to_vec(&packet).unwrap().len();
            assert!(size > 8 * 1024 * 1024);
            assert!(
                size <= MAX_PACKET,
                "An accepted payload cannot cross the wire"
            );
        }
        let effects = recipient.receive(&own, seed, 1).unwrap();
        assert!(effects.is_empty());
        assert_eq!(recipient.committed().0, 1);
        recipient.hard_state().validate().unwrap();
    }

    #[test]
    fn replicated_workspace_validation_rejects_poisoned_payloads_before_log_changes() {
        let state = model::tests::fixture(3);
        let own = state.hub.clone();
        let peer = state
            .members
            .keys()
            .find(|key| *key != &own)
            .unwrap()
            .clone();
        let data = serde_json::to_vec(&state).unwrap();
        let hard = HardState::bootstrap(
            own.clone(),
            state.members.keys().cloned().collect(),
            data.clone(),
        )
        .unwrap();
        let mut engine = Engine::new(own.clone(), hard, 0).unwrap();
        let before = engine.hard_state().clone();
        let rpc = |data| Rpc {
            workspace_id: own.clone(),
            term: 1,
            message: Message::Append {
                prev_index: 0,
                prev_term: 0,
                entries: vec![Entry {
                    index: 1,
                    term: 1,
                    data,
                    configuration: None,
                    barrier: false,
                }],
                leader_commit: 1,
            },
        };
        for invalid in [b"{".to_vec(), {
            let mut missing = state.clone();
            let key = missing.members.keys().next_back().unwrap().clone();
            missing.members.remove(&key);
            serde_json::to_vec(&missing).unwrap()
        }] {
            assert!(validate_rpc_data(&engine, &rpc(invalid)).is_err());
            assert_eq!(engine.hard_state(), &before);
        }
        let valid = rpc(data);
        validate_rpc_data(&engine, &valid).unwrap();
        let effects = engine.receive(&peer, valid, 1).unwrap();
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, Effect::Committed { index: 1, .. }))
        );
        engine.hard_state().validate().unwrap();
        let mut poisoned_image = engine.export();
        poisoned_image.snapshot = b"{".to_vec();
        assert!(validate_image(&poisoned_image).is_err());
        assert_eq!(engine.committed().0, 1);
    }
}
