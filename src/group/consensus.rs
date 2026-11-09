//! Durable majority replication for the workspace control plane.
//!
//! The caller must atomically save `hard_state()` after every operation and
//! before delivering any returned effect. Only `Committed` payloads may reach
//! application code or authorize native side effects. Transport identity is
//! supplied separately from each RPC and must come from its pinned Noise link.
use super::model::{Key, valid_key};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const HEARTBEAT_MS: u64 = 400;
pub const ELECTION_MIN_MS: u64 = 2_000;
pub const ELECTION_MAX_MS: u64 = 4_000;
const MAX_MEMBERS: usize = 256;
const MAX_DATA: usize = 8 * 1024 * 1024;
const MAX_PENDING: usize = 32;
const COMPACT_ENTRIES: u64 = 32;
const COMPACT_BYTES: usize = 8 * 1024 * 1024;

mod bytes {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(data: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(data))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        if encoded.len() > super::MAX_DATA.div_ceil(3) * 4 {
            return Err(serde::de::Error::custom("Consensus payload is too large"));
        }
        let data = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(serde::de::Error::custom)?;
        if data.len() > super::MAX_DATA {
            return Err(serde::de::Error::custom("Consensus payload is too large"));
        }
        Ok(data)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Configuration {
    pub voters: BTreeSet<Key>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub joint: Option<BTreeSet<Key>>,
}

impl Configuration {
    fn validate(&self) -> Result<()> {
        for voters in std::iter::once(&self.voters).chain(self.joint.iter()) {
            ensure!(
                !voters.is_empty() && voters.len() <= MAX_MEMBERS,
                "Invalid workspace voting membership"
            );
            for key in voters {
                valid_key(key)?;
            }
        }
        if let Some(joint) = &self.joint {
            ensure!(
                self.voters.is_subset(joint) && joint.len() == self.voters.len() + 1,
                "Invalid joint workspace membership"
            );
        }
        Ok(())
    }

    pub fn members(&self) -> BTreeSet<Key> {
        let mut members = self.voters.clone();
        if let Some(joint) = &self.joint {
            members.extend(joint.iter().cloned());
        }
        members
    }

    fn quorum(&self, votes: &BTreeSet<Key>) -> bool {
        let majority =
            |members: &BTreeSet<Key>| members.intersection(votes).count() > members.len() / 2;
        majority(&self.voters) && self.joint.as_ref().is_none_or(majority)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Entry {
    pub index: u64,
    pub term: u64,
    #[serde(default)]
    pub barrier: bool,
    #[serde(with = "bytes")]
    pub data: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<Configuration>,
}

impl Entry {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.index > 0 && self.index < u64::MAX && self.term > 0,
            "Invalid consensus entry"
        );
        ensure!(
            self.data.len() <= MAX_DATA,
            "Consensus payload is too large"
        );
        ensure!(
            !self.barrier || (self.data.is_empty() && self.configuration.is_none()),
            "An election barrier cannot contain workspace changes"
        );
        if let Some(configuration) = &self.configuration {
            configuration.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HardState {
    pub workspace_id: Key,
    pub term: u64,
    pub voted_for: Option<Key>,
    pub configuration: Configuration,
    #[serde(default)]
    pub learners: BTreeSet<Key>,
    pub snapshot_index: u64,
    pub snapshot_term: u64,
    #[serde(with = "bytes")]
    pub snapshot: Vec<u8>,
    pub commit_index: u64,
    pub entries: Vec<Entry>,
}

impl HardState {
    pub fn bootstrap(workspace_id: Key, voters: BTreeSet<Key>, data: Vec<u8>) -> Result<Self> {
        let state = Self {
            workspace_id,
            term: 0,
            voted_for: None,
            configuration: Configuration {
                voters,
                joint: None,
            },
            learners: BTreeSet::new(),
            snapshot_index: 0,
            snapshot_term: 0,
            snapshot: data,
            commit_index: 0,
            entries: Vec::new(),
        };
        state.validate()?;
        Ok(state)
    }

    pub fn validate(&self) -> Result<()> {
        valid_key(&self.workspace_id)?;
        self.configuration.validate()?;
        ensure!(
            self.learners.len() <= MAX_MEMBERS,
            "Too many workspace learners"
        );
        for key in &self.learners {
            valid_key(key)?;
        }
        if let Some(vote) = &self.voted_for {
            valid_key(vote)?;
        }
        ensure!(
            self.snapshot.len() <= MAX_DATA,
            "Consensus snapshot is too large"
        );
        ensure!(
            self.snapshot_term <= self.term
                && (self.snapshot_index == 0) == (self.snapshot_term == 0),
            "Invalid consensus snapshot stamp"
        );
        let mut index = self.snapshot_index;
        let mut term = self.snapshot_term;
        for entry in &self.entries {
            entry.validate()?;
            index = index.checked_add(1).context("Consensus index exhausted")?;
            ensure!(
                entry.index == index && entry.term >= term && entry.term <= self.term,
                "Invalid persisted consensus log"
            );
            term = entry.term;
        }
        ensure!(
            self.snapshot_index <= self.commit_index
                && self.commit_index <= index
                && index < u64::MAX,
            "Invalid consensus commit index"
        );
        ensure!(
            self.entries
                .iter()
                .filter(|entry| entry.index > self.commit_index && !entry.barrier)
                .count()
                <= MAX_PENDING,
            "Too many uncommitted workspace entries"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Rpc {
    pub workspace_id: Key,
    pub term: u64,
    pub message: Message,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Message {
    StageJoint {
        recipient: Key,
        prev_index: u64,
        prev_term: u64,
        prev_configuration: Configuration,
        prev_digest: [u8; 32],
        entry: Box<Entry>,
    },
    PreVote {
        next_term: u64,
        last_index: u64,
        last_term: u64,
    },
    PreVoteReply {
        next_term: u64,
        granted: bool,
    },
    RequestVote {
        last_index: u64,
        last_term: u64,
    },
    Vote {
        granted: bool,
    },
    Append {
        prev_index: u64,
        prev_term: u64,
        entries: Vec<Entry>,
        leader_commit: u64,
    },
    Appended {
        probe: u64,
        success: bool,
        matched: u64,
        conflict: u64,
    },
    Snapshot {
        index: u64,
        last_term: u64,
        configuration: Configuration,
        #[serde(with = "bytes")]
        data: Vec<u8>,
    },
    Snapshotted {
        index: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Effect {
    Send {
        target: Key,
        rpc: Rpc,
    },
    Committed {
        index: u64,
        term: u64,
        data: Vec<u8>,
    },
    Leadership {
        leader: Option<Key>,
        writable: bool,
    },
}

#[derive(Clone, Debug)]
enum Role {
    Follower,
    PreCandidate {
        next_term: u64,
        votes: BTreeSet<Key>,
    },
    Candidate {
        votes: BTreeSet<Key>,
    },
    Leader,
}

pub struct Engine {
    own: Key,
    hard: HardState,
    role: Role,
    leader: Option<Key>,
    writable: bool,
    ready_index: u64,
    election_at: u64,
    heartbeat_at: u64,
    quorum_grace: u64,
    now: u64,
    last_contact: BTreeMap<Key, u64>,
    next_index: BTreeMap<Key, u64>,
    matched: BTreeMap<Key, u64>,
}

impl Engine {
    pub fn new(own: Key, hard: HardState, now_ms: u64) -> Result<Self> {
        valid_key(&own)?;
        hard.validate()?;
        let mut engine = Self {
            own,
            hard,
            role: Role::Follower,
            leader: None,
            writable: false,
            ready_index: u64::MAX,
            election_at: now_ms,
            heartbeat_at: now_ms,
            quorum_grace: now_ms,
            now: now_ms,
            last_contact: BTreeMap::new(),
            next_index: BTreeMap::new(),
            matched: BTreeMap::new(),
        };
        engine.reset_election(now_ms);
        if engine.voters().len() == 1 && engine.is_voter(&engine.own) {
            engine.election_at = now_ms;
        }
        Ok(engine)
    }

    pub fn hard_state(&self) -> &HardState {
        &self.hard
    }

    /// A fresh learner imports only a committed image, never somebody else's
    /// durable vote or an abandoned, uncommitted proposal.
    pub fn export(&self) -> HardState {
        let (index, term, data) = self.committed();
        HardState {
            workspace_id: self.hard.workspace_id.clone(),
            term: self.hard.term,
            voted_for: None,
            configuration: self.hard.configuration.clone(),
            learners: BTreeSet::new(),
            snapshot_index: index,
            snapshot_term: term,
            snapshot: data.to_vec(),
            commit_index: index,
            entries: Vec::new(),
        }
    }

    pub fn committed(&self) -> (u64, u64, &[u8]) {
        let index = self.hard.commit_index;
        (
            index,
            self.term_at(index).expect("validated committed entry"),
            self.data_at(index),
        )
    }

    /// Seed an already-consented learner after a restart interrupted its first
    /// joint-entry delivery. The caller sends a committed directory first.
    /// This packet grants no leadership or application-side authority.
    pub fn export_pending_joint(&self) -> Option<Rpc> {
        if self.hard.commit_index == 0
            || self.hard.configuration.joint.is_some()
            || !self.hard.configuration.voters.contains(&self.own)
        {
            return None;
        }
        let entry = self.entry(self.hard.commit_index.checked_add(1)?)?;
        let configuration = entry.configuration.as_ref()?;
        if entry.barrier || configuration.voters != self.hard.configuration.voters {
            return None;
        }
        let joint = configuration.joint.as_ref()?;
        let mut added = joint.difference(&configuration.voters);
        let recipient = added.next()?.clone();
        if added.next().is_some() {
            return None;
        }
        let (prev_index, prev_term, data) = self.committed();
        Some(Rpc {
            workspace_id: self.hard.workspace_id.clone(),
            term: self.hard.term,
            message: Message::StageJoint {
                recipient,
                prev_index,
                prev_term,
                prev_configuration: self.hard.configuration.clone(),
                prev_digest: Sha256::digest(data).into(),
                entry: Box::new(entry.clone()),
            },
        })
    }

    /// Import a committed directory through an already trusted voter when the
    /// actual leader was admitted while this computer was offline. This keeps
    /// the recipient's own vote and never appoints the directory sender leader.
    pub fn catch_up(
        &mut self,
        from_known: &str,
        image: HardState,
        now_ms: u64,
    ) -> Result<Vec<Effect>> {
        self.observe(now_ms)?;
        image.validate()?;
        ensure!(
            image.workspace_id == self.hard.workspace_id,
            "A different workspace sent a directory"
        );
        ensure!(
            from_known != self.own && self.hard.configuration.members().contains(from_known),
            "The workspace directory sender is not a committed voter"
        );
        ensure!(
            image.voted_for.is_none()
                && image.learners.is_empty()
                && image.entries.is_empty()
                && image.commit_index == image.snapshot_index,
            "A workspace directory must contain only a committed snapshot"
        );
        ensure!(
            image.commit_index >= self.hard.commit_index,
            "The workspace directory is older than committed settings"
        );
        ensure!(
            self.hard
                .configuration
                .members()
                .is_subset(&image.configuration.members()),
            "The workspace directory would replace trusted voting identities"
        );
        ensure!(
            image.commit_index > 0 || from_known == self.hard.workspace_id,
            "Only the original coordinator may initialize a workspace directory"
        );
        if image.commit_index == self.hard.commit_index {
            let (_, term, data) = self.committed();
            ensure!(
                term == image.snapshot_term
                    && data == image.snapshot.as_slice()
                    && self.hard.configuration == image.configuration,
                "The workspace directory conflicts with committed settings"
            );
            if image.term <= self.hard.term {
                return Ok(Vec::new());
            }
        }
        let before = self.status();
        let mut effects = Vec::new();
        if image.term > self.hard.term {
            self.hard.term = image.term;
            self.hard.voted_for = None;
        }
        if image.commit_index > self.hard.commit_index {
            let retain = self.term_at(image.commit_index) == Some(image.snapshot_term)
                && self.data_at(image.commit_index) == image.snapshot.as_slice()
                && self.configuration_at(image.commit_index) == &image.configuration;
            let tail = if retain {
                self.hard
                    .entries
                    .iter()
                    .filter(|entry| entry.index > image.commit_index)
                    .cloned()
                    .collect()
            } else {
                Vec::new()
            };
            self.hard.snapshot_index = image.snapshot_index;
            self.hard.snapshot_term = image.snapshot_term;
            self.hard.snapshot = image.snapshot.clone();
            self.hard.commit_index = image.commit_index;
            self.hard.configuration = image.configuration;
            self.hard.entries = tail;
            for key in self.hard.configuration.members() {
                self.hard.learners.remove(&key);
            }
            effects.push(Effect::Committed {
                index: image.commit_index,
                term: image.snapshot_term,
                data: image.snapshot,
            });
        }
        self.follow_same_term(None, now_ms);
        self.finish(before, &mut effects);
        Ok(effects)
    }

    pub fn configuration(&self) -> &Configuration {
        self.hard
            .entries
            .iter()
            .rev()
            .find_map(|entry| entry.configuration.as_ref())
            .unwrap_or(&self.hard.configuration)
    }

    pub fn members(&self) -> BTreeSet<Key> {
        let mut members = self.configuration().members();
        members.extend(self.hard.learners.iter().cloned());
        members
    }

    pub fn voters(&self) -> BTreeSet<Key> {
        self.configuration().members()
    }
    pub fn leader(&self) -> Option<&str> {
        self.leader.as_deref()
    }
    pub fn writable(&self) -> bool {
        self.writable
    }
    pub fn busy(&self) -> bool {
        self.last_index() > self.hard.commit_index || self.hard.configuration.joint.is_some()
    }

    pub fn tick(&mut self, now_ms: u64) -> Result<Vec<Effect>> {
        self.observe(now_ms)?;
        let before = self.status();
        let mut effects = Vec::new();
        if matches!(self.role, Role::Leader) {
            if (self.writable || now_ms >= self.quorum_grace) && !self.recent_quorum(now_ms) {
                self.follow(self.hard.term, None, now_ms);
            } else if now_ms >= self.heartbeat_at {
                self.replicate_all(&mut effects);
                self.heartbeat_at = now_ms.saturating_add(HEARTBEAT_MS);
            }
        } else if now_ms >= self.election_at && self.is_voter(&self.own) {
            self.pre_campaign(now_ms, &mut effects)?;
        }
        self.finish(before, &mut effects);
        Ok(effects)
    }

    pub fn propose(&mut self, data: Vec<u8>, now_ms: u64) -> Result<Vec<Effect>> {
        self.observe(now_ms)?;
        self.can_propose(now_ms)?;
        ensure!(
            !self.busy(),
            "The previous workspace change is still committing"
        );
        let before = self.status();
        let mut effects = Vec::new();
        self.append(data, None, false)?;
        self.advance_commit(&mut effects)?;
        self.replicate_all(&mut effects);
        self.finish(before, &mut effects);
        Ok(effects)
    }

    pub fn begin_add(&mut self, peer: Key, data: Vec<u8>, now_ms: u64) -> Result<Vec<Effect>> {
        self.observe(now_ms)?;
        self.can_propose(now_ms)?;
        ensure!(
            !self.busy(),
            "A workspace membership change is still committing"
        );
        valid_key(&peer)?;
        let old = self.hard.configuration.voters.clone();
        ensure!(
            !old.contains(&peer),
            "This computer is already a workspace voter"
        );
        let mut next = old.clone();
        next.insert(peer);
        let configuration = Configuration {
            voters: old,
            joint: Some(next),
        };
        configuration.validate()?;
        let before = self.status();
        let mut effects = Vec::new();
        self.append(data, Some(configuration), false)?;
        self.advance_commit(&mut effects)?;
        self.replicate_all(&mut effects);
        self.finish(before, &mut effects);
        Ok(effects)
    }

    pub fn receive(&mut self, from: &str, rpc: Rpc, now_ms: u64) -> Result<Vec<Effect>> {
        self.observe(now_ms)?;
        ensure!(
            rpc.workspace_id == self.hard.workspace_id,
            "A different workspace sent a consensus message"
        );
        ensure!(
            from != self.own && self.members().contains(from),
            "Unknown consensus member"
        );
        if !matches!(
            rpc.message,
            Message::Appended { .. } | Message::Snapshotted { .. }
        ) {
            ensure!(
                self.is_voter(from),
                "A workspace learner cannot claim leadership or vote"
            );
        }
        let before = self.status();
        let mut effects = Vec::new();
        match rpc.message {
            Message::StageJoint {
                recipient,
                prev_index,
                prev_term,
                prev_configuration,
                prev_digest,
                entry,
            } => {
                entry.validate()?;
                prev_configuration.validate()?;
                ensure!(
                    recipient == self.own
                        && self.hard.configuration.members().contains(from)
                        && prev_configuration.voters.contains(from),
                    "A pending membership seed must target its trusted learner"
                );
                let configuration = entry
                    .configuration
                    .as_ref()
                    .context("A membership seed is missing its joint voter roster")?;
                let joint = configuration
                    .joint
                    .as_ref()
                    .context("A membership seed is not a joint voter roster")?;
                ensure!(
                    !entry.barrier
                        && prev_index > 0
                        && prev_term > 0
                        && prev_configuration.joint.is_none()
                        && entry.index
                            == prev_index
                                .checked_add(1)
                                .context("Consensus index exhausted")?
                        && entry.term >= prev_term
                        && entry.term <= rpc.term
                        && configuration.voters == prev_configuration.voters
                        && joint
                            .difference(&configuration.voters)
                            .eq(std::iter::once(&self.own)),
                    "The pending membership seed does not add exactly its intended learner"
                );
                if self.hard.configuration.members().contains(&self.own)
                    || self.hard.commit_index > prev_index
                {
                    // A delayed admission seed cannot change an existing
                    // voter or roll a newer committed directory backwards.
                    return Ok(Vec::new());
                }
                ensure!(
                    prev_configuration == self.hard.configuration,
                    "The pending membership seed has a different committed voter roster"
                );
                let (committed_index, committed_term, committed_data) = self.committed();
                let committed_digest: [u8; 32] = Sha256::digest(committed_data).into();
                ensure!(
                    prev_index == committed_index
                        && prev_term == committed_term
                        && prev_digest == committed_digest
                        && entry.term <= self.hard.term,
                    "The pending membership seed has a different committed prefix"
                );
                if let Some(existing) = self.entry(entry.index) {
                    ensure!(
                        existing == entry.as_ref(),
                        "A membership seed conflicts with an existing durable entry"
                    );
                } else {
                    ensure!(
                        self.last_index() == prev_index,
                        "A membership seed would replace a durable log suffix"
                    );
                    // Keep the recipient's own term/vote and every existing
                    // entry. Normal joint election and replication must still
                    // commit this record before it reaches application code.
                    self.hard.entries.push(*entry);
                }
            }
            Message::PreVote {
                next_term,
                last_index,
                last_term,
            } => {
                let leader_recent = self.leader.is_some() && now_ms < self.election_at;
                let granted = self.is_voter(&self.own)
                    && next_term > self.hard.term
                    && !leader_recent
                    && !matches!(self.role, Role::Leader)
                    && self.up_to_date(last_index, last_term);
                self.send(
                    from,
                    Message::PreVoteReply { next_term, granted },
                    &mut effects,
                );
            }
            Message::PreVoteReply { next_term, granted } => {
                if rpc.term > self.hard.term {
                    self.follow(rpc.term, None, now_ms);
                } else if granted {
                    let votes = match &mut self.role {
                        Role::PreCandidate {
                            next_term: expected,
                            votes,
                        } if *expected == next_term => {
                            votes.insert(from.into());
                            Some(votes.clone())
                        }
                        _ => None,
                    };
                    if votes
                        .as_ref()
                        .is_some_and(|votes| self.configuration().quorum(votes))
                    {
                        self.campaign(now_ms, &mut effects)?;
                    }
                }
            }
            Message::RequestVote {
                last_index,
                last_term,
            } => {
                if rpc.term > self.hard.term {
                    self.follow(rpc.term, None, now_ms);
                }
                let granted = self.is_voter(&self.own)
                    && rpc.term == self.hard.term
                    && self
                        .hard
                        .voted_for
                        .as_deref()
                        .is_none_or(|vote| vote == from)
                    && self.up_to_date(last_index, last_term);
                if granted {
                    self.hard.voted_for = Some(from.into());
                    self.follow_same_term(None, now_ms);
                }
                self.send(from, Message::Vote { granted }, &mut effects);
            }
            Message::Vote { granted } => {
                if rpc.term > self.hard.term {
                    self.follow(rpc.term, None, now_ms);
                } else if rpc.term == self.hard.term && granted {
                    let votes = match &mut self.role {
                        Role::Candidate { votes } => {
                            votes.insert(from.into());
                            Some(votes.clone())
                        }
                        _ => None,
                    };
                    if votes
                        .as_ref()
                        .is_some_and(|votes| self.configuration().quorum(votes))
                    {
                        self.become_leader(now_ms, &mut effects)?;
                    }
                }
            }
            Message::Append {
                prev_index,
                prev_term,
                entries,
                leader_commit,
            } => {
                ensure!(entries.len() <= 1, "Too many entries in a consensus packet");
                for entry in &entries {
                    entry.validate()?;
                }
                if rpc.term < self.hard.term {
                    self.send(
                        from,
                        Message::Appended {
                            probe: prev_index,
                            success: false,
                            matched: 0,
                            conflict: self.last_index().saturating_add(1),
                        },
                        &mut effects,
                    );
                } else {
                    if rpc.term > self.hard.term {
                        self.follow(rpc.term, None, now_ms);
                    }
                    self.follow_same_term(Some(from.into()), now_ms);
                    if self.term_at(prev_index) != Some(prev_term) {
                        let conflict = self.conflict_index(prev_index);
                        self.send(
                            from,
                            Message::Appended {
                                probe: prev_index,
                                success: false,
                                matched: 0,
                                conflict,
                            },
                            &mut effects,
                        );
                    } else {
                        let mut next = prev_index
                            .checked_add(1)
                            .context("Consensus index exhausted")?;
                        for entry in &entries {
                            ensure!(
                                entry.index == next
                                    && entry.term >= prev_term
                                    && entry.term <= rpc.term,
                                "Invalid appended consensus entry"
                            );
                            if let Some(previous) = self.entry(next) {
                                if previous.term == entry.term {
                                    ensure!(
                                        previous == entry,
                                        "Conflicting consensus entries have the same term"
                                    );
                                } else {
                                    ensure!(
                                        next > self.hard.commit_index,
                                        "A leader tried to replace committed workspace data"
                                    );
                                }
                            } else {
                                ensure!(
                                    next == self.last_index() + 1,
                                    "A consensus log contains a gap"
                                );
                            }
                            next = next.checked_add(1).context("Consensus index exhausted")?;
                        }
                        let matched = next - 1;
                        let projected_commit =
                            self.hard.commit_index.max(leader_commit.min(matched));
                        let replaced_at = entries
                            .first()
                            .filter(|entry| {
                                self.entry(entry.index)
                                    .is_none_or(|previous| previous.term != entry.term)
                            })
                            .map(|entry| entry.index);
                        let retained_pending = self
                            .hard
                            .entries
                            .iter()
                            .filter(|entry| {
                                entry.index > projected_commit
                                    && !entry.barrier
                                    && replaced_at.is_none_or(|index| entry.index < index)
                            })
                            .count();
                        let appended_pending = usize::from(entries.first().is_some_and(|entry| {
                            replaced_at.is_some()
                                && entry.index > projected_commit
                                && !entry.barrier
                        }));
                        ensure!(
                            retained_pending + appended_pending <= MAX_PENDING,
                            "Too many uncommitted workspace entries"
                        );
                        // All shape, conflict and capacity checks precede the
                        // first log mutation. Even an error whose higher term
                        // must be persisted leaves a valid durable image.
                        next = prev_index + 1;
                        for entry in entries {
                            if let Some(previous) = self.entry(next) {
                                if previous.term != entry.term {
                                    let offset = self.offset(next).unwrap();
                                    self.hard.entries.truncate(offset);
                                    self.hard.entries.push(entry);
                                }
                            } else {
                                self.hard.entries.push(entry);
                            }
                            next += 1;
                        }
                        self.commit_to(leader_commit.min(matched), &mut effects)?;
                        self.send(
                            from,
                            Message::Appended {
                                probe: prev_index,
                                success: true,
                                matched,
                                conflict: 0,
                            },
                            &mut effects,
                        );
                    }
                }
            }
            Message::Appended {
                probe,
                success,
                matched,
                conflict,
            } => {
                if rpc.term > self.hard.term {
                    self.follow(rpc.term, None, now_ms);
                }
                if rpc.term == self.hard.term && matches!(self.role, Role::Leader) {
                    self.last_contact.insert(from.into(), now_ms);
                    if success {
                        ensure!(
                            matched <= self.last_index() && matched >= probe,
                            "Invalid consensus acknowledgement"
                        );
                        let previous = self.matched.get(from).copied().unwrap_or(0);
                        self.matched.insert(from.into(), previous.max(matched));
                        self.next_index
                            .insert(from.into(), previous.max(matched).saturating_add(1));
                        self.advance_commit(&mut effects)?;
                        if matched < self.last_index() {
                            self.replicate(from, &mut effects);
                        }
                    } else if let Some(current) = probe.checked_add(1)
                        && self.next_index.get(from).copied() == Some(current)
                    {
                        let next = conflict.clamp(1, self.last_index().saturating_add(1));
                        if next != current {
                            self.next_index.insert(from.into(), next);
                            self.replicate(from, &mut effects);
                        }
                    }
                }
            }
            Message::Snapshot {
                index,
                last_term,
                configuration,
                data,
            } => {
                configuration.validate()?;
                ensure!(
                    data.len() <= MAX_DATA
                        && index > 0
                        && index < u64::MAX
                        && last_term > 0
                        && last_term <= rpc.term,
                    "Invalid consensus snapshot"
                );
                if rpc.term >= self.hard.term {
                    if rpc.term > self.hard.term {
                        self.follow(rpc.term, None, now_ms);
                    }
                    self.follow_same_term(Some(from.into()), now_ms);
                    if index == self.hard.commit_index {
                        let (_, committed_term, committed_data) = self.committed();
                        ensure!(
                            committed_term == last_term
                                && committed_data == data.as_slice()
                                && self.hard.configuration == configuration,
                            "A snapshot conflicts with committed workspace data"
                        );
                    }
                    if index > self.hard.commit_index {
                        let retain = self.term_at(index) == Some(last_term)
                            && self.data_at(index) == data.as_slice()
                            && self.configuration_at(index) == &configuration;
                        let tail = if retain {
                            self.hard
                                .entries
                                .iter()
                                .filter(|entry| entry.index > index)
                                .cloned()
                                .collect()
                        } else {
                            Vec::new()
                        };
                        self.hard.snapshot_index = index;
                        self.hard.snapshot_term = last_term;
                        self.hard.snapshot = data.clone();
                        self.hard.configuration = configuration;
                        self.hard.commit_index = index;
                        self.hard.entries = tail;
                        effects.push(Effect::Committed {
                            index,
                            term: last_term,
                            data,
                        });
                    }
                    self.send(from, Message::Snapshotted { index }, &mut effects);
                } else {
                    self.send(
                        from,
                        Message::Appended {
                            probe: index,
                            success: false,
                            matched: 0,
                            conflict: self.last_index().saturating_add(1),
                        },
                        &mut effects,
                    );
                }
            }
            Message::Snapshotted { index } => {
                if rpc.term > self.hard.term {
                    self.follow(rpc.term, None, now_ms);
                }
                if rpc.term == self.hard.term && matches!(self.role, Role::Leader) {
                    ensure!(
                        index <= self.hard.commit_index,
                        "Invalid snapshot acknowledgement"
                    );
                    self.last_contact.insert(from.into(), now_ms);
                    let matched = self.matched.get(from).copied().unwrap_or(0).max(index);
                    self.matched.insert(from.into(), matched);
                    self.next_index
                        .insert(from.into(), matched.saturating_add(1));
                    self.replicate(from, &mut effects);
                }
            }
        }
        self.finish(before, &mut effects);
        Ok(effects)
    }

    fn observe(&mut self, now: u64) -> Result<()> {
        ensure!(now >= self.now, "The consensus clock moved backwards");
        self.now = now;
        Ok(())
    }

    fn status(&self) -> (Option<Key>, bool) {
        (self.leader.clone(), self.writable)
    }
    fn finish(&self, before: (Option<Key>, bool), effects: &mut Vec<Effect>) {
        if before != self.status() {
            effects.push(Effect::Leadership {
                leader: self.leader.clone(),
                writable: self.writable,
            });
        }
    }
    fn last_index(&self) -> u64 {
        self.hard
            .entries
            .last()
            .map_or(self.hard.snapshot_index, |entry| entry.index)
    }
    fn last_term(&self) -> u64 {
        self.hard
            .entries
            .last()
            .map_or(self.hard.snapshot_term, |entry| entry.term)
    }
    fn offset(&self, index: u64) -> Option<usize> {
        index
            .checked_sub(self.hard.snapshot_index + 1)
            .and_then(|offset| usize::try_from(offset).ok())
    }
    fn entry(&self, index: u64) -> Option<&Entry> {
        self.offset(index)
            .and_then(|offset| self.hard.entries.get(offset))
    }
    fn data_at(&self, index: u64) -> &[u8] {
        if index == self.hard.snapshot_index {
            return &self.hard.snapshot;
        }
        self.hard
            .entries
            .iter()
            .rev()
            .find(|entry| entry.index <= index && !entry.barrier)
            .map_or(&self.hard.snapshot, |entry| &entry.data)
    }
    fn configuration_at(&self, index: u64) -> &Configuration {
        self.hard
            .entries
            .iter()
            .rev()
            .filter(|entry| entry.index <= index)
            .find_map(|entry| entry.configuration.as_ref())
            .unwrap_or(&self.hard.configuration)
    }
    fn term_at(&self, index: u64) -> Option<u64> {
        if index == self.hard.snapshot_index {
            Some(self.hard.snapshot_term)
        } else {
            self.entry(index).map(|entry| entry.term)
        }
    }
    fn is_voter(&self, key: &str) -> bool {
        self.configuration().members().contains(key)
    }
    fn up_to_date(&self, index: u64, term: u64) -> bool {
        (term, index) >= (self.last_term(), self.last_index())
    }
    fn reset_election(&mut self, now: u64) {
        let mut hash = Sha256::new();
        hash.update(self.own.as_bytes());
        hash.update(self.hard.term.to_le_bytes());
        hash.update(now.to_le_bytes());
        let digest = hash.finalize();
        let jitter = u64::from_le_bytes(digest[..8].try_into().unwrap());
        self.election_at =
            now.saturating_add(ELECTION_MIN_MS + jitter % (ELECTION_MAX_MS - ELECTION_MIN_MS + 1));
    }
    fn follow(&mut self, term: u64, leader: Option<Key>, now: u64) {
        if term > self.hard.term {
            self.hard.term = term;
            self.hard.voted_for = None;
        }
        self.follow_same_term(leader, now);
    }
    fn follow_same_term(&mut self, leader: Option<Key>, now: u64) {
        self.role = Role::Follower;
        self.leader = leader;
        self.writable = false;
        self.next_index.clear();
        self.matched.clear();
        self.reset_election(now);
    }
    fn send(&self, target: &str, message: Message, effects: &mut Vec<Effect>) {
        effects.push(Effect::Send {
            target: target.into(),
            rpc: Rpc {
                workspace_id: self.hard.workspace_id.clone(),
                term: self.hard.term,
                message,
            },
        });
    }
    fn pre_campaign(&mut self, now: u64, effects: &mut Vec<Effect>) -> Result<()> {
        let next_term = self
            .hard
            .term
            .checked_add(1)
            .context("Consensus term exhausted")?;
        let votes = BTreeSet::from([self.own.clone()]);
        self.role = Role::PreCandidate {
            next_term,
            votes: votes.clone(),
        };
        self.leader = None;
        self.writable = false;
        self.reset_election(now);
        if self.configuration().quorum(&votes) {
            return self.campaign(now, effects);
        }
        for peer in self.voters().into_iter().filter(|peer| peer != &self.own) {
            self.send(
                &peer,
                Message::PreVote {
                    next_term,
                    last_index: self.last_index(),
                    last_term: self.last_term(),
                },
                effects,
            );
        }
        Ok(())
    }
    fn campaign(&mut self, now: u64, effects: &mut Vec<Effect>) -> Result<()> {
        self.hard.term = self
            .hard
            .term
            .checked_add(1)
            .context("Consensus term exhausted")?;
        self.hard.voted_for = Some(self.own.clone());
        let votes = BTreeSet::from([self.own.clone()]);
        self.role = Role::Candidate {
            votes: votes.clone(),
        };
        self.leader = None;
        self.writable = false;
        self.reset_election(now);
        if self.configuration().quorum(&votes) {
            return self.become_leader(now, effects);
        }
        for peer in self.voters().into_iter().filter(|peer| peer != &self.own) {
            self.send(
                &peer,
                Message::RequestVote {
                    last_index: self.last_index(),
                    last_term: self.last_term(),
                },
                effects,
            );
        }
        Ok(())
    }
    fn become_leader(&mut self, now: u64, effects: &mut Vec<Effect>) -> Result<()> {
        // Reserve a current-term no-op before claiming leadership. Failure
        // leaves a non-writable candidate, never a leader unable to progress.
        let ready_index = self.append(Vec::new(), None, true)?;
        self.role = Role::Leader;
        self.leader = Some(self.own.clone());
        self.writable = false;
        self.quorum_grace = now.saturating_add(ELECTION_MAX_MS);
        self.heartbeat_at = now.saturating_add(HEARTBEAT_MS);
        self.last_contact.clear();
        self.next_index = self
            .members()
            .into_iter()
            .map(|key| (key, ready_index))
            .collect();
        self.matched.clear();
        self.matched.insert(self.own.clone(), ready_index);
        self.ready_index = ready_index;
        self.advance_commit(effects)?;
        self.replicate_all(effects);
        Ok(())
    }
    fn append(
        &mut self,
        data: Vec<u8>,
        configuration: Option<Configuration>,
        barrier: bool,
    ) -> Result<u64> {
        ensure!(data.len() <= MAX_DATA, "Consensus payload is too large");
        ensure!(
            barrier
                || self
                    .hard
                    .entries
                    .iter()
                    .filter(|entry| entry.index > self.hard.commit_index && !entry.barrier)
                    .count()
                    < MAX_PENDING,
            "Too many pending workspace changes"
        );
        let index = self
            .last_index()
            .checked_add(1)
            .context("Consensus index exhausted")?;
        let entry = Entry {
            index,
            term: self.hard.term,
            barrier,
            data,
            configuration,
        };
        entry.validate()?;
        self.hard.entries.push(entry);
        self.matched.insert(self.own.clone(), index);
        Ok(index)
    }
    fn recent_quorum(&self, now: u64) -> bool {
        let mut votes = BTreeSet::from([self.own.clone()]);
        for (key, seen) in &self.last_contact {
            if now.saturating_sub(*seen) < ELECTION_MIN_MS {
                votes.insert(key.clone());
            }
        }
        // An admitted learner cannot vote until it receives the pending joint
        // entry. Keep the existing leader alive using committed voters while
        // it replicates that entry. Elections and commits still require the
        // latest joint configuration, and busy() blocks unrelated proposals.
        self.hard.configuration.quorum(&votes)
    }
    fn can_propose(&self, now: u64) -> Result<()> {
        ensure!(
            matches!(self.role, Role::Leader) && self.writable,
            "Waiting for a workspace majority"
        );
        ensure!(
            self.recent_quorum(now),
            "The workspace majority is unavailable"
        );
        Ok(())
    }
    fn replicate_all(&mut self, effects: &mut Vec<Effect>) {
        for peer in self.members() {
            if peer != self.own {
                self.replicate(&peer, effects);
            }
        }
    }
    fn replicate(&mut self, peer: &str, effects: &mut Vec<Effect>) {
        let initial = self.last_index().saturating_add(1);
        let next = *self.next_index.entry(peer.into()).or_insert(initial);
        if next <= self.hard.snapshot_index {
            let (index, last_term, data) = self.committed();
            self.send(
                peer,
                Message::Snapshot {
                    index,
                    last_term,
                    configuration: self.hard.configuration.clone(),
                    data: data.to_vec(),
                },
                effects,
            );
        } else {
            let prev_index = next - 1;
            let prev_term = self.term_at(prev_index).unwrap_or(0);
            let entries = self.entry(next).cloned().into_iter().collect();
            self.send(
                peer,
                Message::Append {
                    prev_index,
                    prev_term,
                    entries,
                    leader_commit: self.hard.commit_index,
                },
                effects,
            );
        }
    }
    fn conflict_index(&self, index: u64) -> u64 {
        if index > self.last_index() {
            return self.last_index().saturating_add(1);
        }
        if index <= self.hard.snapshot_index {
            return self.hard.snapshot_index.saturating_add(1);
        }
        let term = self.term_at(index).unwrap();
        self.hard
            .entries
            .iter()
            .find(|entry| entry.term == term)
            .map_or(self.hard.snapshot_index + 1, |entry| entry.index)
    }
    fn advance_commit(&mut self, effects: &mut Vec<Effect>) -> Result<()> {
        let configuration = self.configuration().clone();
        let mut committed = self.hard.commit_index;
        for entry in self.hard.entries.iter().rev() {
            if entry.index <= committed {
                break;
            }
            if entry.term != self.hard.term {
                continue;
            }
            let matched = self
                .matched
                .iter()
                .filter(|(_, index)| **index >= entry.index)
                .map(|(key, _)| key.clone())
                .collect();
            if configuration.quorum(&matched) {
                committed = entry.index;
                break;
            }
        }
        self.commit_to(committed, effects)?;
        if self.hard.commit_index >= self.ready_index {
            self.writable = true;
        }
        if self.last_index() == self.hard.commit_index
            && let Some(next) = self.hard.configuration.joint.clone()
        {
            let data = self.committed().2.to_vec();
            self.append(
                data,
                Some(Configuration {
                    voters: next,
                    joint: None,
                }),
                false,
            )?;
            self.replicate_all(effects);
        }
        Ok(())
    }
    fn commit_to(&mut self, index: u64, effects: &mut Vec<Effect>) -> Result<()> {
        if index <= self.hard.commit_index {
            return Ok(());
        }
        ensure!(
            index <= self.last_index(),
            "Invalid consensus commit frontier"
        );
        let mut data = self.committed().2.to_vec();
        for next in self.hard.commit_index + 1..=index {
            let entry = self
                .entry(next)
                .context("A committed entry is missing")?
                .clone();
            if let Some(configuration) = &entry.configuration {
                self.hard.configuration = configuration.clone();
                for key in configuration.members() {
                    self.hard.learners.remove(&key);
                }
            }
            if !entry.barrier {
                data = entry.data;
            }
            // Intermediate no-ops need no application work. The final one
            // advances the authority stamp with the resolved preceding state.
            if !entry.barrier || next == index {
                effects.push(Effect::Committed {
                    index: entry.index,
                    term: entry.term,
                    data: data.clone(),
                });
            }
        }
        self.hard.commit_index = index;
        let bytes = self
            .hard
            .entries
            .iter()
            .filter(|entry| entry.index <= index)
            .map(|entry| entry.data.len())
            .sum::<usize>();
        if index - self.hard.snapshot_index >= COMPACT_ENTRIES || bytes >= COMPACT_BYTES {
            let entry = self.entry(index).unwrap().clone();
            self.hard.snapshot_index = index;
            self.hard.snapshot_term = entry.term;
            self.hard.snapshot = data;
            self.hard.entries.retain(|entry| entry.index > index);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn key(index: usize) -> Key {
        format!("{index:064x}")
    }

    struct Cluster {
        nodes: BTreeMap<Key, Engine>,
        durable: BTreeMap<Key, HardState>,
        enabled: BTreeSet<Key>,
        blocked: BTreeSet<(Key, Key)>,
        pending: VecDeque<(Key, Effect)>,
        history: BTreeMap<Key, Vec<Vec<u8>>>,
        now: u64,
        drop_replication: bool,
    }

    impl Cluster {
        fn new(count: usize) -> Self {
            let voters = (1..=count).map(key).collect::<BTreeSet<_>>();
            let hard = HardState::bootstrap(key(1), voters.clone(), b"initial".to_vec()).unwrap();
            Self {
                nodes: voters
                    .iter()
                    .map(|own| {
                        (
                            own.clone(),
                            Engine::new(own.clone(), hard.clone(), 0).unwrap(),
                        )
                    })
                    .collect(),
                durable: voters
                    .iter()
                    .map(|own| (own.clone(), hard.clone()))
                    .collect(),
                enabled: voters,
                blocked: BTreeSet::new(),
                pending: VecDeque::new(),
                history: BTreeMap::new(),
                now: 0,
                drop_replication: false,
            }
        }

        fn stage(&mut self, from: &str, effects: Vec<Effect>) {
            // This is the integration contract: durable state precedes every
            // packet, vote acknowledgement, commit, and native authorization.
            let hard = self.nodes[from].hard_state().clone();
            hard.validate().unwrap();
            self.durable.insert(from.into(), hard);
            self.pending
                .extend(effects.into_iter().map(|effect| (from.into(), effect)));
        }

        fn drain(&mut self) {
            let mut delivered = 0;
            while let Some((from, effect)) = self.pending.pop_front() {
                delivered += 1;
                assert!(
                    delivered < 20_000,
                    "A replication reply created an unbounded message loop"
                );
                match effect {
                    Effect::Send { target, rpc } => {
                        if self.drop_replication
                            && matches!(
                                &rpc.message,
                                Message::Append { .. } | Message::Snapshot { .. }
                            )
                        {
                            continue;
                        }
                        if self.enabled.contains(&from)
                            && self.enabled.contains(&target)
                            && !self.blocked.contains(&(from.clone(), target.clone()))
                        {
                            let effects = self
                                .nodes
                                .get_mut(&target)
                                .unwrap()
                                .receive(&from, rpc, self.now)
                                .unwrap();
                            self.stage(&target, effects);
                        }
                    }
                    Effect::Committed { data, .. } => {
                        self.history.entry(from).or_default().push(data)
                    }
                    Effect::Leadership { .. } => {}
                }
            }
        }

        fn step(&mut self, delta: u64) {
            self.now += delta;
            for own in self.enabled.clone() {
                let effects = self.nodes.get_mut(&own).unwrap().tick(self.now).unwrap();
                self.stage(&own, effects);
            }
            self.drain();
        }

        fn until(&mut self, condition: impl Fn(&Self) -> bool) {
            for _ in 0..300 {
                if condition(self) {
                    return;
                }
                self.step(100);
            }
            panic!(
                "Consensus did not settle: {:?}",
                self.nodes
                    .iter()
                    .map(|(key, node)| (
                        key,
                        node.leader(),
                        node.writable(),
                        node.hard.term,
                        node.hard.commit_index,
                        node.last_index()
                    ))
                    .collect::<Vec<_>>()
            );
        }

        fn leader(&self) -> Option<Key> {
            let leaders = self
                .enabled
                .iter()
                .filter(|key| self.nodes[*key].writable())
                .cloned()
                .collect::<Vec<_>>();
            assert!(
                leaders.len() <= 1,
                "Two writable coordinators survived in one connected simulation"
            );
            leaders.first().cloned()
        }

        fn elect(&mut self) -> Key {
            self.until(|cluster| cluster.leader().is_some());
            self.leader().unwrap()
        }

        fn propose(&mut self, leader: &str, data: &[u8]) {
            self.until(|cluster| !cluster.nodes[leader].busy());
            let effects = self
                .nodes
                .get_mut(leader)
                .unwrap()
                .propose(data.to_vec(), self.now)
                .unwrap();
            self.stage(leader, effects);
            self.drain();
        }

        fn restart(&mut self, own: &str) {
            let persisted = self.durable[own].clone();
            self.nodes.insert(
                own.into(),
                Engine::new(own.into(), persisted, self.now).unwrap(),
            );
            self.enabled.insert(own.into());
        }

        fn data(&self, own: &str) -> &[u8] {
            self.nodes[own].committed().2
        }
    }

    #[test]
    fn a_majority_elects_a_replacement_and_the_old_coordinator_rejoins_without_rollback() {
        let mut cluster = Cluster::new(3);
        let old = cluster.elect();
        cluster.propose(&old, b"before handover");
        cluster.until(|cluster| {
            cluster
                .enabled
                .iter()
                .all(|own| cluster.data(own) == b"before handover")
        });
        cluster.enabled.remove(&old);
        let replacement = cluster.elect();
        assert_ne!(replacement, old);
        cluster.propose(&replacement, b"after handover");
        cluster.restart(&old);
        cluster.until(|cluster| {
            cluster
                .nodes
                .values()
                .all(|node| node.committed().2 == b"after handover")
        });
        assert_eq!(cluster.leader(), Some(replacement));
        assert!(!cluster.nodes[&old].writable());
    }

    #[test]
    fn a_leader_barrier_preserves_an_acknowledged_tail_when_followers_missed_the_commit_notice() {
        let mut cluster = Cluster::new(3);
        let old = cluster.elect();
        cluster.propose(&old, b"acknowledged change");
        assert_eq!(cluster.data(&old), b"acknowledged change");
        assert!(
            cluster
                .nodes
                .iter()
                .any(|(own, node)| { own != &old && node.hard.commit_index < node.last_index() })
        );
        // The old leader has received the quorum acknowledgement and published
        // the change, but has not sent its next commit-index heartbeat.
        cluster.enabled.remove(&old);
        let replacement = cluster.elect();
        assert_ne!(replacement, old);
        cluster.until(|cluster| {
            cluster
                .enabled
                .iter()
                .all(|own| cluster.data(own) == b"acknowledged change")
        });
        assert_eq!(cluster.data(&replacement), b"acknowledged change");
        let history = &cluster.history[&replacement];
        let first = history
            .iter()
            .position(|data| data == b"acknowledged change")
            .unwrap();
        assert!(
            history[first..]
                .iter()
                .all(|data| data == b"acknowledged change")
        );
    }

    #[test]
    fn repeated_elections_with_lost_replication_recover_after_more_than_32_barriers_and_restart() {
        let mut cluster = Cluster::new(3);
        let own = key(1);
        cluster.drop_replication = true;
        let mut restarted_at = 0;
        for _ in 0..6_000 {
            let barriers = cluster.nodes[&own]
                .hard
                .entries
                .iter()
                .filter(|entry| entry.barrier)
                .count();
            if barriers >= MAX_PENDING + 2 {
                break;
            }
            if barriers >= restarted_at + 8 {
                cluster.restart(&own);
                restarted_at = barriers;
            }
            // Other voters receive and durably answer elections, but their
            // replication packets and commit acknowledgements never arrive.
            // Tick only this up-to-date candidate to reproduce the former cap.
            cluster.now += 100;
            let effects = cluster
                .nodes
                .get_mut(&own)
                .unwrap()
                .tick(cluster.now)
                .unwrap();
            cluster.stage(&own, effects);
            cluster.drain();
        }
        assert!(cluster.nodes[&own].hard.entries.len() >= MAX_PENDING + 2);
        assert!(
            cluster.nodes[&own]
                .hard
                .entries
                .iter()
                .all(|entry| entry.barrier && entry.data.is_empty())
        );
        assert_eq!(cluster.nodes[&own].hard.commit_index, 0);
        cluster.nodes[&own].hard.validate().unwrap();
        let retained = cluster.nodes[&own].hard.entries.clone();
        cluster.restart(&own);
        assert_eq!(cluster.nodes[&own].hard.entries, retained);
        cluster.drop_replication = false;
        for _ in 0..300 {
            if cluster.nodes[&own].writable() {
                break;
            }
            cluster.now += 100;
            let effects = cluster
                .nodes
                .get_mut(&own)
                .unwrap()
                .tick(cluster.now)
                .unwrap();
            cluster.stage(&own, effects);
            cluster.drain();
        }
        assert!(cluster.nodes[&own].writable());
        assert!(cluster.nodes[&own].hard.commit_index > MAX_PENDING as u64);
        assert!(cluster.nodes[&own].hard.snapshot_index > 0);
        assert_eq!(cluster.data(&own), b"initial");
        cluster.propose(&own, b"settings after healing");
        cluster.until(|cluster| {
            cluster
                .nodes
                .values()
                .all(|node| node.committed().2 == b"settings after healing")
        });
        for node in cluster.nodes.values() {
            node.hard.validate().unwrap();
        }
    }

    #[test]
    fn a_partitioned_minority_cannot_publish_and_discards_its_uncommitted_proposal() {
        let mut cluster = Cluster::new(5);
        let old = cluster.elect();
        let partner = cluster
            .enabled
            .iter()
            .find(|own| *own != &old)
            .unwrap()
            .clone();
        let minority = BTreeSet::from([old.clone(), partner]);
        for from in &cluster.enabled {
            for target in &cluster.enabled {
                if minority.contains(from) != minority.contains(target) {
                    cluster.blocked.insert((from.clone(), target.clone()));
                }
            }
        }
        cluster.propose(&old, b"uncommitted minority");
        assert_eq!(cluster.data(&old), b"initial");
        cluster.until(|cluster| {
            !cluster.nodes[&old].writable()
                && cluster
                    .enabled
                    .iter()
                    .any(|own| !minority.contains(own) && cluster.nodes[own].writable())
        });
        let replacement = cluster
            .enabled
            .iter()
            .find(|own| !minority.contains(*own) && cluster.nodes[*own].writable())
            .unwrap()
            .clone();
        cluster.propose(&replacement, b"majority choice");
        assert!(
            cluster
                .nodes
                .get_mut(&old)
                .unwrap()
                .propose(b"another minority write".to_vec(), cluster.now)
                .is_err()
        );
        cluster.blocked.clear();
        cluster.until(|cluster| {
            cluster
                .nodes
                .values()
                .all(|node| node.committed().2 == b"majority choice")
        });
        assert!(
            cluster
                .history
                .values()
                .flatten()
                .all(|data| data != b"uncommitted minority")
        );
    }

    #[test]
    fn a_two_computer_workspace_waits_for_both_votes_instead_of_promoting_a_lone_member() {
        let mut cluster = Cluster::new(2);
        let leader = cluster.elect();
        cluster.propose(&leader, b"saved settings");
        cluster.until(|cluster| {
            cluster
                .nodes
                .values()
                .all(|node| node.committed().2 == b"saved settings")
        });
        cluster.enabled.remove(&leader);
        for _ in 0..100 {
            cluster.step(100);
        }
        let remaining = cluster.enabled.first().unwrap().clone();
        assert!(cluster.leader().is_none());
        assert!(
            cluster
                .nodes
                .get_mut(&remaining)
                .unwrap()
                .propose(b"unsafe".to_vec(), cluster.now)
                .is_err()
        );
        assert_eq!(cluster.data(&remaining), b"saved settings");
        cluster.restart(&leader);
        let recovered = cluster.elect();
        cluster.propose(&recovered, b"connected again");
        cluster.until(|cluster| {
            cluster
                .nodes
                .values()
                .all(|node| node.committed().2 == b"connected again")
        });
    }

    #[test]
    fn votes_survive_restart_and_a_less_current_candidate_cannot_replace_committed_settings() {
        let mut cluster = Cluster::new(3);
        let leader = cluster.elect();
        cluster.propose(&leader, b"committed");
        cluster.until(|cluster| {
            cluster
                .nodes
                .values()
                .all(|node| node.committed().2 == b"committed")
        });
        let own = cluster
            .enabled
            .iter()
            .find(|own| *own != &leader)
            .unwrap()
            .clone();
        let another = cluster
            .enabled
            .iter()
            .find(|peer| *peer != &own && *peer != &leader)
            .unwrap()
            .clone();
        let term = cluster.nodes[&own].hard.term + 1;
        let last_index = cluster.nodes[&own].last_index();
        let last_term = cluster.nodes[&own].last_term();
        let request = Rpc {
            workspace_id: key(1),
            term,
            message: Message::RequestVote {
                last_index,
                last_term,
            },
        };
        let effects = cluster
            .nodes
            .get_mut(&own)
            .unwrap()
            .receive(&leader, request, cluster.now)
            .unwrap();
        cluster.stage(&own, effects);
        cluster.pending.clear();
        assert_eq!(cluster.durable[&own].voted_for.as_ref(), Some(&leader));
        cluster.restart(&own);
        let same_term = Rpc {
            workspace_id: key(1),
            term,
            message: Message::RequestVote {
                last_index,
                last_term,
            },
        };
        let denied = cluster
            .nodes
            .get_mut(&own)
            .unwrap()
            .receive(&another, same_term, cluster.now)
            .unwrap();
        assert!(denied.iter().any(|effect| matches!(
            effect,
            Effect::Send {
                rpc: Rpc {
                    message: Message::Vote { granted: false },
                    ..
                },
                ..
            }
        )));
        let stale = Rpc {
            workspace_id: key(1),
            term: term + 1,
            message: Message::RequestVote {
                last_index: 0,
                last_term: 0,
            },
        };
        let denied = cluster
            .nodes
            .get_mut(&own)
            .unwrap()
            .receive(&another, stale, cluster.now)
            .unwrap();
        assert!(denied.iter().any(|effect| matches!(
            effect,
            Effect::Send {
                rpc: Rpc {
                    message: Message::Vote { granted: false },
                    ..
                },
                ..
            }
        )));
        assert_eq!(cluster.data(&own), b"committed");
    }

    #[test]
    fn adding_a_member_uses_old_and_new_majorities_before_finalizing() {
        let mut cluster = Cluster::new(1);
        let leader = cluster.elect();
        let peer = key(2);
        let bootstrap = cluster.nodes[&leader].export();
        assert!(bootstrap.voted_for.is_none());
        assert!(bootstrap.entries.is_empty());
        let learner = Engine::new(peer.clone(), bootstrap.clone(), cluster.now).unwrap();
        assert!(!learner.is_voter(&peer));
        cluster.nodes.insert(peer.clone(), learner);
        cluster.durable.insert(peer.clone(), bootstrap);
        cluster.enabled.insert(peer.clone());
        cluster.blocked.insert((peer.clone(), leader.clone()));
        cluster.blocked.insert((leader.clone(), peer.clone()));
        let effects = cluster
            .nodes
            .get_mut(&leader)
            .unwrap()
            .begin_add(peer.clone(), b"two members".to_vec(), cluster.now)
            .unwrap();
        cluster.stage(&leader, effects);
        cluster.drain();
        assert_eq!(cluster.data(&leader), b"initial");
        assert!(cluster.nodes[&leader].busy());
        assert!(cluster.nodes[&leader].configuration().joint.is_some());
        for _ in 0..(ELECTION_MAX_MS / 100 + 5) {
            cluster.step(100);
        }
        assert_eq!(cluster.leader(), Some(leader.clone()));
        assert_eq!(cluster.data(&leader), b"initial");
        assert!(cluster.nodes[&leader].busy());
        assert!(
            cluster
                .nodes
                .get_mut(&leader)
                .unwrap()
                .propose(b"must wait for admission".to_vec(), cluster.now)
                .is_err()
        );
        assert!(!cluster.nodes[&peer].is_voter(&peer));
        cluster.blocked.clear();
        cluster.until(|cluster| {
            cluster.nodes.values().all(|node| {
                node.committed().2 == b"two members" && node.hard.configuration.joint.is_none()
            }) && !cluster.nodes[&leader].busy()
        });
        assert!(
            cluster
                .nodes
                .values()
                .all(|node| node.voters() == BTreeSet::from([key(1), key(2)]))
        );
        cluster.propose(&leader, b"both may edit");
        cluster.until(|cluster| {
            cluster
                .nodes
                .values()
                .all(|node| node.committed().2 == b"both may edit")
        });
    }

    fn admission_lost_before_first_joint_delivery() -> (Cluster, Key, Key, Rpc) {
        let mut cluster = Cluster::new(1);
        let leader = cluster.elect();
        let learner = key(2);
        let image = cluster.nodes[&leader].export();
        cluster.nodes.insert(
            learner.clone(),
            Engine::new(learner.clone(), image.clone(), cluster.now).unwrap(),
        );
        cluster.durable.insert(learner.clone(), image);
        cluster.enabled.insert(learner.clone());
        cluster.blocked.insert((leader.clone(), learner.clone()));
        cluster.blocked.insert((learner.clone(), leader.clone()));
        let effects = cluster
            .nodes
            .get_mut(&leader)
            .unwrap()
            .begin_add(learner.clone(), b"two members".to_vec(), cluster.now)
            .unwrap();
        cluster.stage(&leader, effects);
        cluster.drain();
        let seed = cluster.nodes[&leader].export_pending_joint().unwrap();
        (cluster, leader, learner, seed)
    }

    #[test]
    fn a_pending_joint_seed_recovers_restart_before_the_learners_first_entry_delivery() {
        let (mut cluster, leader, learner, seed) = admission_lost_before_first_joint_delivery();
        assert_eq!(cluster.nodes[&leader].hard.commit_index, 1);
        assert_eq!(cluster.nodes[&leader].last_index(), 2);
        assert!(!cluster.nodes[&learner].is_voter(&learner));
        assert_eq!(cluster.data(&learner), b"initial");
        cluster.restart(&leader);
        assert!(cluster.nodes[&leader].leader().is_none());
        cluster.blocked.clear();

        // The authenticated committed directory precedes its one pending
        // admission seed. Neither can publish the learner's membership yet.
        let directory = cluster.nodes[&leader].export();
        let effects = cluster
            .nodes
            .get_mut(&learner)
            .unwrap()
            .catch_up(&leader, directory, cluster.now)
            .unwrap();
        cluster.stage(&learner, effects);
        let before = cluster.nodes[&learner].hard.clone();
        let election_at = cluster.nodes[&learner].election_at;
        let encoded = serde_json::to_vec(&seed).unwrap();
        let decoded = serde_json::from_slice(&encoded).unwrap();
        let effects = cluster
            .nodes
            .get_mut(&learner)
            .unwrap()
            .receive(&leader, decoded, cluster.now)
            .unwrap();
        assert!(effects.is_empty());
        assert_eq!(cluster.nodes[&learner].hard.term, before.term);
        assert_eq!(cluster.nodes[&learner].hard.voted_for, before.voted_for);
        assert_eq!(
            cluster.nodes[&learner].hard.commit_index,
            before.commit_index
        );
        assert_eq!(cluster.nodes[&learner].election_at, election_at);
        assert!(cluster.nodes[&learner].leader().is_none());
        assert!(cluster.nodes[&learner].is_voter(&learner));
        assert_eq!(cluster.data(&learner), b"initial");
        cluster.stage(&learner, effects);
        cluster.restart(&learner);
        cluster.until(|cluster| {
            cluster.nodes.values().all(|node| {
                node.committed().2 == b"two members" && node.hard.configuration.joint.is_none()
            }) && cluster.leader().is_some()
        });
        let elected = cluster.leader().unwrap();
        cluster.propose(&elected, b"settings after admission recovery");
        cluster.until(|cluster| {
            cluster
                .nodes
                .values()
                .all(|node| node.committed().2 == b"settings after admission recovery")
        });
        assert!(
            cluster
                .nodes
                .values()
                .all(|node| node.export_pending_joint().is_none())
        );
        let before = cluster.nodes[&learner].hard.clone();
        let status = cluster.nodes[&learner].status();
        assert!(
            cluster
                .nodes
                .get_mut(&learner)
                .unwrap()
                .receive(&leader, seed, cluster.now)
                .unwrap()
                .is_empty()
        );
        assert_eq!(cluster.nodes[&learner].hard, before);
        assert_eq!(cluster.nodes[&learner].status(), status);
    }

    #[test]
    fn membership_seeds_reject_rebinding_and_conflicts_and_preserve_matching_durable_tails() {
        let (cluster, leader, learner, seed) = admission_lost_before_first_joint_delivery();
        let image = cluster.nodes[&learner].hard.clone();
        let mutations: [fn(&mut Rpc); 5] = [
            |rpc| {
                if let Message::StageJoint { recipient, .. } = &mut rpc.message {
                    *recipient = key(3);
                }
            },
            |rpc| {
                if let Message::StageJoint { prev_digest, .. } = &mut rpc.message {
                    *prev_digest = [255; 32];
                }
            },
            |rpc| {
                if let Message::StageJoint {
                    prev_configuration, ..
                } = &mut rpc.message
                {
                    prev_configuration.voters.insert(key(3));
                }
            },
            |rpc| {
                if let Message::StageJoint { entry, .. } = &mut rpc.message {
                    entry.index += 1;
                }
            },
            |rpc| {
                if let Message::StageJoint { entry, .. } = &mut rpc.message {
                    entry.term += 1;
                    rpc.term = entry.term;
                }
            },
        ];
        for mutate in mutations {
            let mut node = Engine::new(learner.clone(), image.clone(), 0).unwrap();
            let before = node.hard.clone();
            let mut invalid = seed.clone();
            mutate(&mut invalid);
            assert!(node.receive(&leader, invalid, 0).is_err());
            assert_eq!(node.hard, before);
            node.hard.validate().unwrap();
        }
        let mut node = Engine::new(learner.clone(), image, 0).unwrap();
        node.receive(&leader, seed.clone(), 0).unwrap();
        let mut persisted = node.hard.clone();
        persisted.term = 3;
        persisted.voted_for = Some(leader.clone());
        persisted.entries.push(Entry {
            index: 3,
            term: 2,
            barrier: true,
            data: Vec::new(),
            configuration: None,
        });
        let mut restarted = Engine::new(learner, persisted.clone(), 0).unwrap();
        let election_at = restarted.election_at;
        assert!(
            restarted
                .receive(&leader, seed.clone(), 0)
                .unwrap()
                .is_empty()
        );
        assert_eq!(restarted.hard, persisted);
        assert_eq!(restarted.election_at, election_at);
        let mut conflicting = seed;
        if let Message::StageJoint { entry, .. } = &mut conflicting.message {
            entry.data = b"a different uncommitted admission".to_vec();
        }
        assert!(restarted.receive(&leader, conflicting, 0).is_err());
        assert_eq!(restarted.hard, persisted);
        restarted.hard.validate().unwrap();

        // A newer committed old-roster state may have superseded admission
        // before this learner received the delayed seed. It must not re-arm it.
        let mut superseded = cluster.nodes[&key(2)].hard.clone();
        superseded.term = 2;
        superseded.snapshot_index = 2;
        superseded.snapshot_term = 2;
        superseded.commit_index = 2;
        superseded.snapshot = b"newer old-roster settings".to_vec();
        let mut node = Engine::new(key(2), superseded.clone(), 0).unwrap();
        let stale = cluster.nodes[&leader].export_pending_joint().unwrap();
        assert!(node.receive(&leader, stale, 0).unwrap().is_empty());
        assert_eq!(node.hard, superseded);
        assert!(!node.is_voter(&key(2)));
    }

    #[test]
    fn snapshot_compaction_catches_up_a_stopped_member_without_replaying_abandoned_data() {
        let mut cluster = Cluster::new(3);
        let leader = cluster.elect();
        let stopped = cluster
            .enabled
            .iter()
            .find(|own| *own != &leader)
            .unwrap()
            .clone();
        cluster.enabled.remove(&stopped);
        for index in 0..45 {
            cluster.propose(&leader, format!("settings {index}").as_bytes());
            cluster.step(HEARTBEAT_MS);
        }
        assert!(cluster.nodes[&leader].hard.snapshot_index >= COMPACT_ENTRIES);
        cluster.restart(&stopped);
        cluster.until(|cluster| {
            cluster
                .nodes
                .values()
                .all(|node| node.committed().2 == b"settings 44")
        });
        assert!(cluster.nodes[&stopped].hard.snapshot_index > 0);
        for node in cluster.nodes.values() {
            node.hard.validate().unwrap();
        }
    }

    fn sent_rpc(effects: &[Effect], target: &str) -> Rpc {
        effects
            .iter()
            .find_map(|effect| match effect {
                Effect::Send { target: peer, rpc } if peer == target => Some(rpc.clone()),
                _ => None,
            })
            .expect("A replication packet was not sent")
    }

    #[test]
    fn a_bootstrapped_snapshot_moves_a_stale_probe_forward_and_receives_the_missing_tail() {
        let mut cluster = Cluster::new(3);
        let leader = cluster.elect();
        let follower = cluster
            .enabled
            .iter()
            .find(|peer| *peer != &leader)
            .unwrap()
            .clone();
        cluster.enabled.remove(&follower);
        cluster.propose(&leader, b"committed prefix");
        assert_eq!(cluster.nodes[&leader].hard.commit_index, 2);

        // An existing session can have queued an early probe before its
        // authenticated directory fast-forwarded the follower's snapshot.
        cluster
            .nodes
            .get_mut(&leader)
            .unwrap()
            .next_index
            .insert(follower.clone(), 1);
        let image = cluster.nodes[&leader].export();
        let effects = cluster
            .nodes
            .get_mut(&follower)
            .unwrap()
            .catch_up(&leader, image, cluster.now)
            .unwrap();
        cluster.stage(&follower, effects);
        cluster.drain();
        assert_eq!(cluster.nodes[&follower].hard.snapshot_index, 2);
        assert_eq!(cluster.nodes[&follower].hard.commit_index, 2);
        assert!(cluster.nodes[&follower].hard.entries.is_empty());
        cluster.enabled.insert(follower.clone());

        let effects = cluster
            .nodes
            .get_mut(&leader)
            .unwrap()
            .propose(b"missing entry three".to_vec(), cluster.now)
            .unwrap();
        // Drop the other peer's packet so a NACK alone cannot appear to
        // advance the commit through an unrelated acknowledgement.
        let probe = sent_rpc(&effects, &follower);
        assert!(matches!(
            &probe.message,
            Message::Append { prev_index: 0, .. }
        ));
        let replies = cluster
            .nodes
            .get_mut(&follower)
            .unwrap()
            .receive(&leader, probe, cluster.now)
            .unwrap();
        let nack = sent_rpc(&replies, &leader);
        assert!(matches!(
            &nack.message,
            Message::Appended {
                success: false,
                conflict: 3,
                ..
            }
        ));
        let durable = cluster.nodes[&leader].hard.clone();
        let matched = cluster.nodes[&leader].matched.clone();
        let retry = cluster
            .nodes
            .get_mut(&leader)
            .unwrap()
            .receive(&follower, nack, cluster.now)
            .unwrap();
        assert_eq!(cluster.nodes[&leader].next_index[&follower], 3);
        assert_eq!(cluster.nodes[&leader].hard, durable);
        assert_eq!(cluster.nodes[&leader].matched, matched);
        assert!(
            matches!(sent_rpc(&retry, &follower).message, Message::Append { prev_index: 2, ref entries, .. } if entries.len() == 1 && entries[0].index == 3)
        );
        cluster.stage(&leader, retry);
        cluster.drain();
        cluster.until(|cluster| {
            cluster
                .nodes
                .values()
                .all(|node| node.committed().2 == b"missing entry three")
        });
        assert_eq!(cluster.nodes[&follower].hard.commit_index, 3);
        for node in cluster.nodes.values() {
            node.hard.validate().unwrap();
        }
    }

    #[test]
    fn nack_hints_are_bounded_and_never_grant_replication_or_commit() {
        let mut cluster = Cluster::new(3);
        let leader = cluster.elect();
        cluster.propose(&leader, b"committed prefix");
        let follower = cluster
            .enabled
            .iter()
            .find(|peer| *peer != &leader)
            .unwrap()
            .clone();
        let upper = cluster.nodes[&leader].last_index() + 1;
        for (current, hint, expected) in [
            (1, 0, 1),
            (1, u64::MAX, upper),
            (upper, 0, 1),
            (upper, u64::MAX, upper),
        ] {
            cluster
                .nodes
                .get_mut(&leader)
                .unwrap()
                .next_index
                .insert(follower.clone(), current);
            let durable = cluster.nodes[&leader].hard.clone();
            let matched = cluster.nodes[&leader].matched.clone();
            let rpc = Rpc {
                workspace_id: key(1),
                term: durable.term,
                message: Message::Appended {
                    probe: current - 1,
                    success: false,
                    matched: u64::MAX,
                    conflict: hint,
                },
            };
            let effects = cluster
                .nodes
                .get_mut(&leader)
                .unwrap()
                .receive(&follower, rpc, cluster.now)
                .unwrap();
            assert_eq!(cluster.nodes[&leader].next_index[&follower], expected);
            assert!((1..=upper).contains(&expected));
            assert_eq!(cluster.nodes[&leader].hard, durable);
            assert_eq!(cluster.nodes[&leader].matched, matched);
            assert_eq!(
                effects
                    .iter()
                    .filter(|effect| matches!(effect, Effect::Send { .. }))
                    .count(),
                usize::from(current != expected)
            );
        }
        for current in [None, Some(upper)] {
            if let Some(current) = current {
                cluster
                    .nodes
                    .get_mut(&leader)
                    .unwrap()
                    .next_index
                    .insert(follower.clone(), current);
            } else {
                cluster
                    .nodes
                    .get_mut(&leader)
                    .unwrap()
                    .next_index
                    .remove(&follower);
            }
            let before = cluster.nodes[&leader].hard.clone();
            let matched = cluster.nodes[&leader].matched.clone();
            let effects = cluster
                .nodes
                .get_mut(&leader)
                .unwrap()
                .receive(
                    &follower,
                    Rpc {
                        workspace_id: key(1),
                        term: before.term,
                        message: Message::Appended {
                            probe: u64::MAX,
                            success: false,
                            matched: u64::MAX,
                            conflict: u64::MAX,
                        },
                    },
                    cluster.now,
                )
                .unwrap();
            assert!(effects.is_empty());
            assert_eq!(
                cluster.nodes[&leader].next_index.get(&follower).copied(),
                current
            );
            assert_eq!(cluster.nodes[&leader].hard, before);
            assert_eq!(cluster.nodes[&leader].matched, matched);
        }
    }

    #[test]
    fn unchanged_or_stale_nacks_wait_for_heartbeats_instead_of_spinning() {
        let mut cluster = Cluster::new(3);
        let leader = cluster.elect();
        let follower = cluster
            .enabled
            .iter()
            .find(|peer| *peer != &leader)
            .unwrap()
            .clone();
        cluster
            .nodes
            .get_mut(&leader)
            .unwrap()
            .next_index
            .insert(follower.clone(), 1);
        let durable = cluster.nodes[&leader].hard.clone();
        let matched = cluster.nodes[&leader].matched.clone();
        let reply = |probe, conflict| Rpc {
            workspace_id: key(1),
            term: durable.term,
            message: Message::Appended {
                probe,
                success: false,
                matched: 0,
                conflict,
            },
        };
        for rpc in [reply(0, 1), reply(1, 1)] {
            let effects = cluster
                .nodes
                .get_mut(&leader)
                .unwrap()
                .receive(&follower, rpc, cluster.now)
                .unwrap();
            assert!(effects.is_empty());
            assert_eq!(cluster.nodes[&leader].next_index[&follower], 1);
            assert_eq!(cluster.nodes[&leader].hard, durable);
            assert_eq!(cluster.nodes[&leader].matched, matched);
        }
        let now = cluster.nodes[&leader].heartbeat_at;
        let effects = cluster.nodes.get_mut(&leader).unwrap().tick(now).unwrap();
        assert!(matches!(
            sent_rpc(&effects, &follower).message,
            Message::Append { prev_index: 0, .. }
        ));
        assert_eq!(cluster.nodes[&leader].hard, durable);
    }

    #[test]
    fn catchup_snapshot_keeps_its_data_and_voting_configuration_at_the_same_committed_index() {
        let mut cluster = Cluster::new(3);
        let leader = cluster.elect();
        let stopped = cluster
            .enabled
            .iter()
            .find(|own| *own != &leader)
            .unwrap()
            .clone();
        cluster.enabled.remove(&stopped);
        for index in 0..35 {
            cluster.propose(&leader, format!("old roster {index}").as_bytes());
            cluster.step(HEARTBEAT_MS);
        }
        let peer = key(4);
        let bootstrap = cluster.nodes[&leader].export();
        cluster.nodes.insert(
            peer.clone(),
            Engine::new(peer.clone(), bootstrap.clone(), cluster.now).unwrap(),
        );
        cluster.durable.insert(peer.clone(), bootstrap);
        cluster.enabled.insert(peer.clone());
        let effects = cluster
            .nodes
            .get_mut(&leader)
            .unwrap()
            .begin_add(peer, b"four-member roster".to_vec(), cluster.now)
            .unwrap();
        cluster.stage(&leader, effects);
        cluster.drain();
        cluster.until(|cluster| !cluster.nodes[&leader].busy());
        assert!(
            cluster.nodes[&leader].hard.snapshot_index < cluster.nodes[&leader].hard.commit_index
        );
        let frontier = cluster.nodes[&leader].hard.commit_index;
        cluster.restart(&stopped);
        cluster.until(|cluster| cluster.data(&stopped) == b"four-member roster");
        assert_eq!(cluster.nodes[&stopped].hard.snapshot_index, frontier);
        assert_eq!(cluster.nodes[&stopped].hard.configuration.voters.len(), 4);
        assert!(cluster.nodes[&stopped].hard.configuration.joint.is_none());
        cluster.nodes[&stopped].hard.validate().unwrap();
    }

    #[test]
    fn untrusted_or_cross_workspace_messages_cannot_change_the_durable_term_or_roster() {
        let mut cluster = Cluster::new(3);
        let own = key(1);
        let before = cluster.nodes[&own].hard.clone();
        for (from, workspace_id) in [(key(9), key(1)), (key(2), key(9))] {
            let rpc = Rpc {
                workspace_id,
                term: u64::MAX,
                message: Message::RequestVote {
                    last_index: 0,
                    last_term: 0,
                },
            };
            assert!(
                cluster
                    .nodes
                    .get_mut(&own)
                    .unwrap()
                    .receive(&from, rpc, 0)
                    .is_err()
            );
            assert_eq!(cluster.nodes[&own].hard, before);
        }
    }

    #[test]
    fn malformed_append_never_leaves_an_invalid_or_partly_replaced_durable_log() {
        let voters = BTreeSet::from([key(1), key(2), key(3)]);
        let mut hard = HardState::bootstrap(key(1), voters.clone(), b"initial".to_vec()).unwrap();
        hard.term = 1;
        hard.voted_for = Some(key(1));
        hard.entries = (1..=MAX_PENDING as u64)
            .map(|index| Entry {
                index,
                term: 1,
                barrier: false,
                data: format!("entry {index}").into_bytes(),
                configuration: None,
            })
            .collect();
        let mut node = Engine::new(key(2), hard.clone(), 0).unwrap();
        let overflow = Rpc {
            workspace_id: key(1),
            term: 2,
            message: Message::Append {
                prev_index: MAX_PENDING as u64,
                prev_term: 1,
                entries: vec![Entry {
                    index: MAX_PENDING as u64 + 1,
                    term: 2,
                    barrier: false,
                    data: b"overflow".to_vec(),
                    configuration: None,
                }],
                leader_commit: 0,
            },
        };
        assert!(node.receive(&key(1), overflow.clone(), 1).is_err());
        assert_eq!(node.hard.entries, hard.entries);
        assert_eq!(node.hard.commit_index, 0);
        assert_eq!(node.hard.term, 2);
        assert!(node.hard.voted_for.is_none());
        node.hard.validate().unwrap();

        // A full pending prefix can still make progress when this same packet
        // proves its prefix committed; it need not remain stuck at the bound.
        let Rpc {
            workspace_id,
            term,
            message:
                Message::Append {
                    prev_index,
                    prev_term,
                    entries,
                    ..
                },
        } = overflow
        else {
            unreachable!()
        };
        let committed = Rpc {
            workspace_id,
            term,
            message: Message::Append {
                prev_index,
                prev_term,
                entries,
                leader_commit: MAX_PENDING as u64 + 1,
            },
        };
        assert!(node.receive(&key(1), committed, 2).is_ok());
        node.hard.validate().unwrap();
        assert_eq!(node.hard.commit_index, MAX_PENDING as u64 + 1);

        for configuration_change in [false, true] {
            let mut hard =
                HardState::bootstrap(key(1), voters.clone(), b"initial".to_vec()).unwrap();
            hard.term = 1;
            hard.entries.push(Entry {
                index: 1,
                term: 1,
                barrier: false,
                data: b"original".to_vec(),
                configuration: None,
            });
            let mut node = Engine::new(key(2), hard.clone(), 0).unwrap();
            let mut conflicting = hard.entries[0].clone();
            if configuration_change {
                conflicting.configuration = Some(Configuration {
                    voters: voters.clone(),
                    joint: Some(BTreeSet::from([key(1), key(2), key(3), key(4)])),
                });
            } else {
                conflicting.data = b"replaced payload".to_vec();
            }
            let conflict = Rpc {
                workspace_id: key(1),
                term: 1,
                message: Message::Append {
                    prev_index: 0,
                    prev_term: 0,
                    entries: vec![conflicting],
                    leader_commit: 0,
                },
            };
            assert!(node.receive(&key(1), conflict, 0).is_err());
            assert_eq!(node.hard, hard);
            node.hard.validate().unwrap();
        }
    }

    #[test]
    fn trusted_directory_catchup_preserves_the_local_vote_and_admits_a_previously_unknown_leader() {
        let mut local = HardState::bootstrap(
            key(1),
            BTreeSet::from([key(1), key(2), key(3)]),
            b"old roster".to_vec(),
        )
        .unwrap();
        local.term = 7;
        local.voted_for = Some(key(2));
        local.snapshot_index = 5;
        local.snapshot_term = 6;
        local.commit_index = 5;
        local.entries.push(Entry {
            index: 6,
            term: 7,
            barrier: false,
            data: b"uncommitted tail".to_vec(),
            configuration: None,
        });
        let mut node = Engine::new(key(3), local, 0).unwrap();
        let unknown = Rpc {
            workspace_id: key(1),
            term: 8,
            message: Message::RequestVote {
                last_index: 10,
                last_term: 7,
            },
        };
        assert!(node.receive(&key(4), unknown.clone(), 0).is_err());
        assert_eq!(node.hard.term, 7);
        let image = HardState {
            workspace_id: key(1),
            term: 7,
            voted_for: None,
            configuration: Configuration {
                voters: BTreeSet::from([key(1), key(2), key(3), key(4)]),
                joint: None,
            },
            learners: BTreeSet::new(),
            snapshot_index: 10,
            snapshot_term: 7,
            snapshot: b"new committed roster".to_vec(),
            commit_index: 10,
            entries: Vec::new(),
        };
        let effects = node.catch_up(&key(1), image.clone(), 1).unwrap();
        assert_eq!(node.hard.voted_for, Some(key(2)));
        assert_eq!(node.hard.term, 7);
        assert!(node.hard.entries.is_empty());
        assert!(node.leader().is_none() && !node.writable());
        assert!(node.voters().contains(&key(4)));
        assert!(effects.iter().any(|effect| matches!(effect, Effect::Committed { data, index: 10, .. } if data == b"new committed roster")));
        let before = node.hard.clone();
        let mut conflicting = image;
        conflicting.snapshot = b"conflicting data".to_vec();
        assert!(node.catch_up(&key(1), conflicting, 2).is_err());
        assert_eq!(node.hard, before);
        let effects = node.receive(&key(4), unknown, 3).unwrap();
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::Send {
                rpc: Rpc {
                    message: Message::Vote { granted: true },
                    ..
                },
                ..
            }
        )));
        assert_eq!(node.hard.voted_for, Some(key(4)));
        assert_eq!(node.hard.term, 8);
    }

    #[test]
    fn a_directory_at_a_barrier_index_preserves_a_later_durable_state_entry() {
        let voters = BTreeSet::from([key(1), key(2), key(3)]);
        let mut hard =
            HardState::bootstrap(key(1), voters.clone(), b"old settings".to_vec()).unwrap();
        hard.term = 7;
        hard.voted_for = Some(key(2));
        hard.snapshot_index = 5;
        hard.snapshot_term = 5;
        hard.commit_index = 5;
        hard.entries = vec![
            Entry {
                index: 6,
                term: 6,
                barrier: false,
                data: b"acknowledged settings".to_vec(),
                configuration: None,
            },
            Entry {
                index: 7,
                term: 6,
                barrier: true,
                data: Vec::new(),
                configuration: None,
            },
            Entry {
                index: 8,
                term: 7,
                barrier: false,
                data: b"later acknowledged settings".to_vec(),
                configuration: None,
            },
        ];
        let retained = hard.entries[2].clone();
        let mut node = Engine::new(key(3), hard, 0).unwrap();
        let image = HardState {
            workspace_id: key(1),
            term: 7,
            voted_for: None,
            configuration: Configuration {
                voters,
                joint: None,
            },
            learners: BTreeSet::new(),
            snapshot_index: 7,
            snapshot_term: 6,
            snapshot: b"acknowledged settings".to_vec(),
            commit_index: 7,
            entries: Vec::new(),
        };
        node.catch_up(&key(1), image, 1).unwrap();
        assert_eq!(node.hard.entries, vec![retained]);
        assert_eq!(node.committed().2, b"acknowledged settings");
        assert_eq!(node.hard.voted_for, Some(key(2)));
        let restarted = Engine::new(key(3), node.hard.clone(), 0).unwrap();
        assert_eq!(
            restarted.hard.entries[0].data,
            b"later acknowledged settings"
        );
        restarted.hard.validate().unwrap();
    }

    #[test]
    fn election_barriers_cannot_carry_data_or_membership_and_legacy_entries_default_to_state() {
        let mut entry = Entry {
            index: 1,
            term: 1,
            barrier: true,
            data: vec![1],
            configuration: None,
        };
        assert!(entry.validate().is_err());
        entry.data.clear();
        entry.configuration = Some(Configuration {
            voters: BTreeSet::from([key(1)]),
            joint: None,
        });
        assert!(entry.validate().is_err());
        entry.barrier = false;
        let mut legacy = serde_json::to_value(&entry).unwrap();
        legacy.as_object_mut().unwrap().remove("barrier");
        let decoded: Entry = serde_json::from_value(legacy).unwrap();
        assert!(!decoded.barrier);
        decoded.validate().unwrap();
    }

    /// Cuts every link between two groups so the halves behave as separate
    /// partitions while all nodes stay online.
    fn split(cluster: &mut Cluster, left: &[usize], right: &[usize]) {
        cluster.blocked.clear();
        for one in left {
            for other in right {
                cluster.blocked.insert((key(*one), key(*other)));
                cluster.blocked.insert((key(*other), key(*one)));
            }
        }
    }

    fn writable_leaders(cluster: &Cluster) -> Vec<(Key, u64)> {
        cluster
            .nodes
            .iter()
            .filter(|(_, node)| node.writable())
            .map(|(own, node)| (own.clone(), node.hard_state().term))
            .collect()
    }

    #[test]
    fn a_minority_partition_never_elects_a_coordinator() {
        let mut cluster = Cluster::new(4);
        let leader = cluster.elect();
        cluster.propose(&leader, b"agreed before the split");
        cluster.until(|cluster| {
            cluster
                .nodes
                .values()
                .all(|node| node.committed().2 == b"agreed before the split")
        });
        // Two of four voters are not a majority, so neither half may accept
        // a workspace change while the split lasts.
        split(&mut cluster, &[1, 2], &[3, 4]);
        for _ in 0..120 {
            cluster.step(100);
        }
        assert!(
            writable_leaders(&cluster).is_empty(),
            "A minority partition elected a coordinator: {:?}",
            writable_leaders(&cluster)
        );
        for node in cluster.nodes.values() {
            assert_eq!(node.committed().2, b"agreed before the split");
        }
        // Healing the partition restores exactly one coordinator.
        cluster.blocked.clear();
        cluster.until(|cluster| writable_leaders(cluster).len() == 1);
        assert_eq!(writable_leaders(&cluster).len(), 1);
    }

    #[test]
    fn two_majories_never_lead_the_same_term_and_the_cluster_reconverges() {
        let mut cluster = Cluster::new(6);
        let leader = cluster.elect();
        cluster.propose(&leader, b"agreed before the split");
        cluster.until(|cluster| {
            cluster
                .nodes
                .values()
                .all(|node| node.committed().2 == b"agreed before the split")
        });
        // Both halves hold three of six voters, so both may run an election.
        split(&mut cluster, &[1, 2, 3], &[4, 5, 6]);
        for _ in 0..200 {
            cluster.step(100);
        }
        let leaders = writable_leaders(&cluster);
        assert!(
            leaders.len() <= 2,
            "More than one coordinator per majority: {leaders:?}"
        );
        let terms = leaders.iter().map(|(_, term)| *term).collect::<Vec<_>>();
        let mut unique = terms.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            terms.len(),
            "Two coordinators claimed the same term: {leaders:?}"
        );
        // After healing, one coordinator survives and every node agrees.
        cluster.blocked.clear();
        for _ in 0..400 {
            cluster.step(100);
            if writable_leaders(&cluster).len() == 1 {
                break;
            }
        }
        assert_eq!(writable_leaders(&cluster).len(), 1);
        for _ in 0..200 {
            cluster.step(100);
            let values = cluster
                .nodes
                .values()
                .map(|node| node.committed().2.to_vec())
                .collect::<Vec<_>>();
            if values.windows(2).all(|pair| pair[0] == pair[1]) {
                return;
            }
        }
        panic!("The cluster did not converge on one committed value");
    }

    #[test]
    fn a_leader_that_cannot_replicate_never_commits_the_change() {
        let mut cluster = Cluster::new(3);
        let leader = cluster.elect();
        cluster.propose(&leader, b"agreed change");
        cluster.until(|cluster| {
            cluster
                .nodes
                .values()
                .all(|node| node.committed().2 == b"agreed change")
        });
        // Without acknowledgements the new entry must stay uncommitted.
        cluster.drop_replication = true;
        let effects = cluster
            .nodes
            .get_mut(&leader)
            .unwrap()
            .propose(b"unreplicated change".to_vec(), cluster.now)
            .unwrap();
        cluster.stage(&leader, effects);
        cluster.drain();
        assert_eq!(
            cluster.nodes[&leader].committed().2,
            b"agreed change",
            "A change was committed without a quorum"
        );
        assert!(cluster.nodes[&leader].busy());
        // Restoring replication lets the cluster commit and converge again.
        cluster.drop_replication = false;
        for _ in 0..300 {
            cluster.step(100);
            let values = cluster
                .nodes
                .values()
                .map(|node| node.committed().2.to_vec())
                .collect::<Vec<_>>();
            if values.windows(2).all(|pair| pair[0] == pair[1]) && values[0] != b"agreed change" {
                return;
            }
        }
        panic!("The cluster did not converge after the replication returned");
    }
}
