//! Single-use source-side cut finalization after a complete native Move receipt.
//!
//! This module never permanently deletes a selected source. It atomically
//! detaches verified originals into an exclusively owned recovery directory on
//! the same volume, then hands them to recoverable platform Trash. Retention is
//! reported as needing recovery, not as a finished Move. A journal grants no
//! authority to detach an original after a restart.
use super::super::source_trash::{self, TrashPlan, TrashReceipt, TrashStatus};
use super::{
    Dir, FileIdentity, MAX_ENTRIES, MAX_MANIFEST, Manifest, Metadata, NO_CANCEL, SourceEntry,
    SourceMetadata, SourceSelection, TEMP, absolute_directory, directory, exists, finished_hash,
    hash_prefix, missing, open_file, private_builder, private_directory, protocol, read_json,
    regular, sync_dir, write_new,
};
use anyhow::{Context, Result, bail, ensure};
use cap_fs_ext::DirExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, atomic::Ordering},
    time::SystemTime,
};

const MOVE_FORMAT: u16 = 1;
const JOURNAL_LIMIT: usize = 16 * 1024 * 1024;
const OWNER_LIMIT: usize = 4096;
const ROOT_CHECKPOINT_LIMIT: usize = 256 * 1024;
const MAX_JOURNALS: usize = 1024;
static FINALIZING: Mutex<()> = Mutex::new(());

#[cfg(test)]
thread_local! {
    static BEFORE_RENAME: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static AFTER_RENAME: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

/// The service constructs this only after matching a local, explicit Cut grant
/// and a successful, verified, full-batch native destination Move receipt.
/// None of these fields is a source path or grants source filesystem authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceMoveBinding {
    pub workspace: String,
    pub peer: String,
    pub offer: String,
    pub paste: String,
    pub export: String,
    pub manifest_digest: String,
    pub cut_grant: String,
    pub cut_roots: String,
}

impl SourceMoveBinding {
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.workspace.is_empty()
                && self.workspace.len() <= 256
                && !self.workspace.chars().any(char::is_control),
            "Invalid source Move workspace"
        );
        crate::group::model::valid_key(&self.peer)?;
        for id in [&self.offer, &self.paste, &self.export, &self.cut_grant] {
            protocol::valid_id(id)?;
        }
        ensure!(
            protocol::valid_hash(&self.manifest_digest) && protocol::valid_hash(&self.cut_roots),
            "Invalid source Move content binding"
        );
        Ok(())
    }

    fn journal_name(&self) -> Result<String> {
        self.validate()?;
        let mut hash = Sha256::new();
        hash.update(b"Desk Switch single-use source Cut\0");
        // A new paste, export or peer must not grant a second detachment of the
        // same local Cut. The first journal fixes those additional bindings.
        for value in [&self.workspace, &self.offer, &self.cut_grant] {
            hash.update((value.len() as u64).to_le_bytes());
            hash.update(value.as_bytes());
        }
        Ok(format!("{:x}.json", hash.finalize()))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SourceMoveState {
    /// Every original has a verified platform Trash/Recycle Bin receipt. A
    /// retained recovery payload must never be presented as this state.
    Moved,
    OriginalsRetained,
    NeedsRecovery,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecoveryLocation {
    pub original: PathBuf,
    pub recovery: Option<PathBuf>,
    pub detail: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceMoveOutcome {
    pub state: SourceMoveState,
    pub locations: Vec<RecoveryLocation>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    device: u64,
    inode: u64,
}

impl Identity {
    fn of(metadata: &Metadata) -> Result<Self> {
        Ok(Self::from(&FileIdentity::of(metadata)?))
    }
}

impl From<&FileIdentity> for Identity {
    fn from(value: &FileIdentity) -> Self {
        Self {
            device: value.device,
            inode: value.inode,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CapturedMetadata {
    identity: Identity,
    directory: bool,
    length: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    #[cfg(unix)]
    changed: (i64, i64),
}

impl CapturedMetadata {
    fn from_source(metadata: &SourceMetadata) -> Self {
        Self {
            identity: Identity::from(&metadata.identity),
            directory: metadata.directory,
            length: metadata.length,
            modified: metadata.modified.map(|time| time.into_std()),
            created: metadata.created.map(|time| time.into_std()),
            #[cfg(unix)]
            changed: metadata.changed,
        }
    }

    fn check(&self, metadata: &Metadata, renamed_root: bool) -> Result<()> {
        #[cfg(unix)]
        let mut current = Self::from_source(&SourceMetadata::of(metadata)?);
        #[cfg(not(unix))]
        let current = Self::from_source(&SourceMetadata::of(metadata)?);
        // Renaming the selected root changes its Unix ctime. Descendant
        // identities, metadata, names and file hashes still match exactly.
        #[cfg(unix)]
        if renamed_root {
            current.changed = self.changed;
        }
        #[cfg(not(unix))]
        let _ = renamed_root;
        ensure!(
            self == &current,
            "An original file or folder changed after it was cut"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CapturedEntry {
    name: String,
    path: Vec<String>,
    metadata: CapturedMetadata,
    children: Vec<CapturedEntry>,
}

#[derive(Serialize)]
struct FlatEntryRef<'a> {
    name: &'a str,
    path: &'a [String],
    metadata: &'a CapturedMetadata,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FlatEntry {
    name: String,
    path: Vec<String>,
    metadata: CapturedMetadata,
}

// A flat durable representation keeps a valid 64-level selection below JSON's
// parser nesting limit. Recovery never needs to disable that safety bound.
impl Serialize for CapturedEntry {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        fn flatten<'a>(entry: &'a CapturedEntry, entries: &mut Vec<FlatEntryRef<'a>>) {
            entries.push(FlatEntryRef {
                name: &entry.name,
                path: &entry.path,
                metadata: &entry.metadata,
            });
            for child in &entry.children {
                flatten(child, entries);
            }
        }
        let mut entries = Vec::new();
        flatten(self, &mut entries);
        entries.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CapturedEntry {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        use serde::de::Error;
        fn build<E: Error>(
            entry: FlatEntry,
            children: &mut BTreeMap<Vec<String>, Vec<FlatEntry>>,
        ) -> std::result::Result<CapturedEntry, E> {
            let descendants = children
                .remove(&entry.path)
                .unwrap_or_default()
                .into_iter()
                .map(|child| build::<E>(child, children))
                .collect::<std::result::Result<_, _>>()?;
            Ok(CapturedEntry {
                name: entry.name,
                path: entry.path,
                metadata: entry.metadata,
                children: descendants,
            })
        }
        let entries = Vec::<FlatEntry>::deserialize(deserializer)?;
        if entries.is_empty() || entries.len() > MAX_ENTRIES {
            return Err(D::Error::custom("Invalid source Move tree size"));
        }
        let mut entries = entries.into_iter();
        let root = entries.next().expect("Nonempty tree checked above");
        if root.path.len() != 1 {
            return Err(D::Error::custom("Invalid source Move root depth"));
        }
        let mut paths = BTreeSet::from([root.path.clone()]);
        let mut children = BTreeMap::<Vec<String>, Vec<FlatEntry>>::new();
        for entry in entries {
            if !(2..=64).contains(&entry.path.len()) || !paths.insert(entry.path.clone()) {
                return Err(D::Error::custom(
                    "Invalid source Move child depth or duplicate path",
                ));
            }
            children
                .entry(entry.path[..entry.path.len() - 1].to_vec())
                .or_default()
                .push(entry);
        }
        let root = build::<D::Error>(root, &mut children)?;
        if !children.is_empty() {
            return Err(D::Error::custom("A source Move child has no parent"));
        }
        Ok(root)
    }
}

impl CapturedEntry {
    fn from_source(entry: &SourceEntry) -> Result<Self> {
        Ok(Self {
            name: entry
                .name
                .to_str()
                .context("File names must be valid Unicode")?
                .into(),
            path: entry.path.clone(),
            metadata: CapturedMetadata::from_source(&entry.metadata),
            children: entry
                .children
                .iter()
                .map(Self::from_source)
                .collect::<Result<_>>()?,
        })
    }

    fn add_identities(&self, roots: &BTreeSet<Identity>, child: bool) -> Result<()> {
        ensure!(
            !child || !roots.contains(&self.metadata.identity),
            "Overlapping files or folders cannot be moved; cut each source once"
        );
        for entry in &self.children {
            entry.add_identities(roots, true)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum RootPhase {
    Untouched,
    Intent,
    Retained,
    TrashIntent,
    TrashCommitted,
    Restored,
    NeedsRecovery,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RootRecord {
    parent: PathBuf,
    parent_identity: Identity,
    entry: CapturedEntry,
    quarantine: String,
    quarantine_identity: Option<Identity>,
    trash_plan: Option<TrashPlan>,
    trash_receipt: Option<TrashReceipt>,
    phase: RootPhase,
    detail: String,
}

impl RootRecord {
    fn original(&self) -> PathBuf {
        self.parent.join(&self.entry.name)
    }

    fn recovery(&self) -> PathBuf {
        self.parent.join(&self.quarantine)
    }

    fn pin_parent(&self) -> Result<Dir> {
        let directory = absolute_directory(&self.parent, false)?;
        ensure!(
            Identity::of(&directory.dir_metadata()?)? == self.parent_identity,
            "The original source parent moved or was replaced; recovery requires attention"
        );
        Ok(directory)
    }

    fn pin_quarantine(&self, parent: &Dir, binding: &SourceMoveBinding) -> Result<Dir> {
        let quarantine = directory(parent, OsStr::new(&self.quarantine), false)?;
        private_directory(&quarantine)?;
        source_trash::require_private_dir(&quarantine)?;
        let identity = Identity::of(&quarantine.dir_metadata()?)?;
        ensure!(
            self.quarantine_identity.as_ref() == Some(&identity),
            "The source recovery directory identity changed"
        );
        let owner: QuarantineOwner = read_json(&quarantine, "owner.json", OWNER_LIMIT)?;
        ensure!(
            owner == QuarantineOwner::new(binding, self, identity),
            "The recovery directory does not belong to this Cut"
        );
        Ok(quarantine)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    format: u16,
    binding: SourceMoveBinding,
    manifest: Manifest,
    roots: Vec<RootRecord>,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CheckpointOwner {
    format: u16,
    binding: SourceMoveBinding,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RootCheckpoint {
    format: u16,
    binding: SourceMoveBinding,
    index: usize,
    parent_identity: Identity,
    source_identity: Identity,
    quarantine_identity: Option<Identity>,
    phase: RootPhase,
    trash_plan: Option<TrashPlan>,
    trash_receipt: Option<TrashReceipt>,
    detail: String,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct QuarantineOwner {
    format: u16,
    binding: SourceMoveBinding,
    parent: Identity,
    source: Identity,
    quarantine: Identity,
}

impl QuarantineOwner {
    fn new(binding: &SourceMoveBinding, root: &RootRecord, quarantine: Identity) -> Self {
        Self {
            format: MOVE_FORMAT,
            binding: binding.clone(),
            parent: root.parent_identity.clone(),
            source: root.entry.metadata.identity.clone(),
            quarantine,
        }
    }
}

impl SourceSelection {
    /// Metadata-only binding to the locally captured Cut roots. Calling this
    /// does not create transfer storage, read contents, or grant Move authority.
    pub fn cut_root_digest(&self) -> Result<String> {
        ensure!(
            self.description == self.captured_description,
            "The copied file description changed"
        );
        let mut roots = Vec::new();
        let mut identities = BTreeSet::new();
        let mut metadata_bytes = 0usize;
        for root in &self.roots {
            let entry = CapturedEntry::from_source(&root.entry)?;
            ensure!(
                identities.insert(entry.metadata.identity.clone()),
                "A source was selected more than once; cut each source once"
            );
            metadata_bytes = metadata_bytes
                .checked_add(root.parent_path.as_os_str().len())
                .context("The Cut metadata exceeds its limits")?;
            ensure!(
                metadata_bytes <= MAX_MANIFEST,
                "The Cut source paths exceed their limits"
            );
            roots.push((
                root.parent_path.clone(),
                Identity::from(&root.parent_identity),
                entry,
            ));
        }
        for (_, _, entry) in &roots {
            entry.add_identities(&identities, false)?;
        }
        captured_root_digest(&roots)
    }

    /// Consume the local selection once the service has authenticated a complete
    /// native Move receipt for this exact Cut, paste, export and verified bytes.
    /// A cache download, EOF, close/release, or Copy result must never call this.
    #[cfg(test)]
    pub fn finalize_move(
        self,
        state_dir: &Path,
        manifest: &Manifest,
        binding: &SourceMoveBinding,
    ) -> Result<SourceMoveOutcome> {
        self.finalize_move_with_progress(state_dir, manifest, binding, |_, _, _| {})
    }

    /// Publish brief source-cleanup progress from the bounded worker. The
    /// callback must not reenter storage or perform GUI work synchronously.
    pub fn finalize_move_with_progress(
        self,
        state_dir: &Path,
        manifest: &Manifest,
        binding: &SourceMoveBinding,
        mut progress: impl FnMut(usize, usize, &str),
    ) -> Result<SourceMoveOutcome> {
        binding.validate()?;
        manifest.validate()?;
        ensure!(
            binding.export == manifest.id
                && binding.manifest_digest == manifest.digest()?
                && manifest.description() == self.captured_description
                && binding.cut_roots == self.cut_root_digest()?,
            "The native Move receipt does not match the locally cut originals"
        );
        let _guard = FINALIZING
            .lock()
            .map_err(|_| anyhow::anyhow!("Source Move recovery is unavailable"))?;
        let journals =
            journal_directory(state_dir, true)?.context("Source Move recovery is unavailable")?;
        let name = binding.journal_name()?;
        if exists(&journals, &name)? {
            // Existing intent is terminal source authority. Never attempt an
            // original rename again, even if a crash left no recovery payload.
            let saved = load_journal(&journals, &name, binding)?;
            progress(saved.roots.len(), saved.roots.len(), "");
            return reconcile(&saved);
        }
        let mut count = 0usize;
        for entry in journals.entries()? {
            let _ = entry?;
            count += 1;
            ensure!(
                count < MAX_JOURNALS * 2,
                "Source Move recovery history is full; originals were retained"
            );
        }
        let mut journal = Journal {
            format: MOVE_FORMAT,
            binding: binding.clone(),
            manifest: manifest.clone(),
            roots: self
                .roots
                .iter()
                .enumerate()
                .map(|(index, root)| {
                    Ok(RootRecord {
                        parent: root.parent_path.clone(),
                        parent_identity: Identity::from(&root.parent_identity),
                        entry: CapturedEntry::from_source(&root.entry)?,
                        quarantine: quarantine_name(binding, index),
                        quarantine_identity: None,
                        trash_plan: None,
                        trash_receipt: None,
                        phase: RootPhase::Untouched,
                        detail: "The original source was retained".into(),
                    })
                })
                .collect::<Result<_>>()?,
        };
        validate_journal(&journal, binding)?;
        let bytes = journal_bytes(&journal)?;
        // New-only receipt persistence serializes competing application
        // processes too. No source namespace has changed at this point.
        create_checkpoints(&journals, &name, binding)?;
        write_new(&journals, &name, &bytes)?;
        let kinds = manifest_kinds(manifest);

        // Verify the entire batch before detaching any root. The service has
        // already required a successful native full-batch destination receipt.
        for index in 0..journal.roots.len() {
            progress(0, journal.roots.len(), &journal.roots[index].entry.name);
            let result = (|| {
                self.roots[index].check_binding()?;
                let parent = journal.roots[index].pin_parent()?;
                verify_entry(
                    &parent,
                    OsStr::new(&journal.roots[index].entry.name),
                    &journal.roots[index].entry,
                    &kinds,
                    false,
                )
            })();
            if let Err(error) = result {
                journal.roots[index].detail = format!("The original was retained: {error:#}");
                persist_root(&journals, &name, &journal, index)?;
                return Ok(outcome_from_live(&journal));
            }
        }
        // Captured roots keep only their parent capability; the selected child
        // is reopened for each bound read. Open the identity-checked native
        // rename handle without a lingering child directory handle.
        for index in 0..journal.roots.len() {
            let result = detach_root(&journals, &name, &mut journal, index, &kinds);
            if let Err(error) = result {
                let root = &mut journal.roots[index];
                if root.phase == RootPhase::Intent || root.phase == RootPhase::Retained {
                    root.phase = RootPhase::NeedsRecovery;
                }
                root.detail = if root.phase == RootPhase::Untouched {
                    format!("The original was retained: {error:#}")
                } else {
                    format!("Source recovery requires attention: {error:#}")
                };
                // Intent was durably saved before rename. Failure to save this
                // additional explanation cannot cause replay on the next run.
                let _ = persist_root(&journals, &name, &journal, index);
                progress(
                    index + 1,
                    journal.roots.len(),
                    &journal.roots[index].entry.name,
                );
                break;
            }
            if journal.roots[index].phase == RootPhase::Retained
                && let Err(error) = handoff_to_trash(&journals, &name, &mut journal, index)
            {
                let root = &mut journal.roots[index];
                if matches!(
                    root.phase,
                    RootPhase::TrashIntent | RootPhase::TrashCommitted
                ) {
                    root.phase = RootPhase::NeedsRecovery;
                }
                root.detail = format!(
                    "The source is retained for recovery; Trash did not produce a durable completion receipt: {error:#}"
                );
                let _ = persist_root(&journals, &name, &journal, index);
            }
            progress(
                index + 1,
                journal.roots.len(),
                &journal.roots[index].entry.name,
            );
        }
        Ok(outcome_from_live(&journal))
    }
}

fn quarantine_name(binding: &SourceMoveBinding, index: usize) -> String {
    format!(".desk-switch-move-{}-{index:08x}", binding.cut_grant)
}

fn captured_root_digest(roots: &impl Serialize) -> Result<String> {
    let bytes = serde_json::to_vec(roots)?;
    ensure!(
        bytes.len() <= JOURNAL_LIMIT / 2,
        "The Cut metadata exceeds its limits"
    );
    let mut hash = Sha256::new();
    hash.update(b"Desk Switch local captured Cut roots\0");
    hash.update(bytes);
    Ok(format!("{:x}", hash.finalize()))
}

fn journal_directory(state_dir: &Path, create: bool) -> Result<Option<Dir>> {
    let mut current = match absolute_directory(state_dir, create) {
        Ok(directory) => directory,
        Err(error) if !create && missing(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    for name in ["file-transfer", "source-moves"] {
        let created = if create && name == "source-moves" {
            match current.create_dir_with(name, &private_builder()) {
                Ok(()) => {
                    sync_dir(&current)?;
                    true
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
                Err(error) => return Err(error.into()),
            }
        } else {
            false
        };
        current = match directory(&current, OsStr::new(name), create && name != "source-moves") {
            Ok(directory) => directory,
            Err(error) if !create && missing(&error) => return Ok(None),
            Err(error) => return Err(error),
        };
        if created {
            source_trash::protect_new_private_dir(&current)?;
        }
        private_directory(&current)?;
        if name == "source-moves" {
            source_trash::require_private_dir(&current)?;
        }
    }
    Ok(Some(current))
}

fn journal_bytes(journal: &Journal) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(journal)?;
    ensure!(
        bytes.len() <= JOURNAL_LIMIT,
        "Source Move recovery metadata exceeds its limits"
    );
    Ok(bytes)
}

fn checkpoint_directory_name(name: &str) -> Result<String> {
    let prefix = name
        .strip_suffix(".json")
        .context("Invalid source Move journal name")?;
    ensure!(
        protocol::valid_hash(prefix),
        "Invalid source Move journal identity"
    );
    Ok(format!("{prefix}-roots"))
}

fn create_checkpoints(parent: &Dir, name: &str, binding: &SourceMoveBinding) -> Result<()> {
    let name = checkpoint_directory_name(name)?;
    // An orphan from an interrupted initial receipt is not reused. No source
    // can have moved before its immutable batch receipt was committed.
    parent.create_dir_with(&name, &private_builder())?;
    let checkpoints = directory(parent, OsStr::new(&name), false)?;
    source_trash::protect_new_private_dir(&checkpoints)?;
    private_directory(&checkpoints)?;
    source_trash::require_private_dir(&checkpoints)?;
    write_new(
        &checkpoints,
        "owner.json",
        &serde_json::to_vec(&CheckpointOwner {
            format: MOVE_FORMAT,
            binding: binding.clone(),
        })?,
    )?;
    sync_dir(parent)
}

fn open_checkpoints(parent: &Dir, name: &str, binding: &SourceMoveBinding) -> Result<Dir> {
    let name = checkpoint_directory_name(name)?;
    let checkpoints = directory(parent, OsStr::new(&name), false)?;
    private_directory(&checkpoints)?;
    source_trash::require_private_dir(&checkpoints)?;
    ensure!(
        read_json::<CheckpointOwner>(&checkpoints, "owner.json", OWNER_LIMIT)?
            == CheckpointOwner {
                format: MOVE_FORMAT,
                binding: binding.clone()
            },
        "The source Move checkpoints belong to a different Cut"
    );
    Ok(checkpoints)
}

fn checkpoint_bytes(journal: &Journal, index: usize) -> Result<Vec<u8>> {
    let root = journal
        .roots
        .get(index)
        .context("Unknown source Move root checkpoint")?;
    let checkpoint = RootCheckpoint {
        format: MOVE_FORMAT,
        binding: journal.binding.clone(),
        index,
        parent_identity: root.parent_identity.clone(),
        source_identity: root.entry.metadata.identity.clone(),
        quarantine_identity: root.quarantine_identity.clone(),
        phase: root.phase,
        trash_plan: root.trash_plan.clone(),
        trash_receipt: root.trash_receipt.clone(),
        detail: root.detail.clone(),
    };
    let bytes = serde_json::to_vec(&checkpoint)?;
    ensure!(
        bytes.len() <= ROOT_CHECKPOINT_LIMIT,
        "The source Move root checkpoint exceeds its limits"
    );
    Ok(bytes)
}

fn persist_root(parent: &Dir, name: &str, journal: &Journal, index: usize) -> Result<()> {
    let checkpoints = open_checkpoints(parent, name, &journal.binding)?;
    let bytes = checkpoint_bytes(journal, index)?;
    write_atomic(&checkpoints, &format!("r{index:08x}.json"), &bytes)
}

fn write_atomic(parent: &Dir, name: &str, bytes: &[u8]) -> Result<()> {
    for _ in 0..128 {
        let temporary = format!(
            ".move-journal-{:x}-{:x}",
            std::process::id(),
            TEMP.fetch_add(1, Ordering::Relaxed)
        );
        let mut file = match open_file(parent, &temporary, true, true) {
            Ok(file) => file,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::AlreadyExists) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        let result = (|| {
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            // Only this fixed private root checkpoint is replaced. The batch
            // manifest is immutable. Source renames use native NOREPLACE.
            parent.rename(&temporary, parent, name)?;
            sync_dir(parent)
        })();
        if result.is_err() {
            let _ = parent.remove_file(&temporary);
        }
        return result;
    }
    bail!("Could not save source Move recovery metadata")
}

fn load_journal(parent: &Dir, name: &str, binding: &SourceMoveBinding) -> Result<Journal> {
    let mut journal: Journal = read_json(parent, name, JOURNAL_LIMIT)?;
    validate_journal(&journal, binding)?;
    ensure!(
        journal
            .roots
            .iter()
            .all(|root| root.phase == RootPhase::Untouched
                && root.quarantine_identity.is_none()
                && root.trash_plan.is_none()
                && root.trash_receipt.is_none()),
        "The immutable source Move batch contains unexpected mutable state"
    );
    let checkpoints = open_checkpoints(parent, name, binding)?;
    for (index, root) in journal.roots.iter_mut().enumerate() {
        let name = format!("r{index:08x}.json");
        if !exists(&checkpoints, &name)? {
            continue;
        }
        let saved: RootCheckpoint = read_json(&checkpoints, &name, ROOT_CHECKPOINT_LIMIT)?;
        ensure!(
            saved.format == MOVE_FORMAT
                && saved.binding == *binding
                && saved.index == index
                && saved.parent_identity == root.parent_identity
                && saved.source_identity == root.entry.metadata.identity,
            "A source Move root checkpoint belongs to another source"
        );
        root.quarantine_identity = saved.quarantine_identity;
        root.phase = saved.phase;
        root.trash_plan = saved.trash_plan;
        root.trash_receipt = saved.trash_receipt;
        root.detail = saved.detail;
    }
    validate_journal(&journal, binding)?;
    Ok(journal)
}

fn validate_journal(journal: &Journal, binding: &SourceMoveBinding) -> Result<()> {
    binding.validate()?;
    journal.manifest.validate()?;
    ensure!(
        journal.format == MOVE_FORMAT
            && journal.binding == *binding
            && journal.manifest.id == binding.export
            && journal.manifest.digest()? == binding.manifest_digest
            && journal.roots.len() == journal.manifest.roots.len()
            && !journal.roots.is_empty(),
        "The source Move journal belongs to a different Cut or paste"
    );
    let kinds = manifest_kinds(&journal.manifest);
    let roots = journal
        .roots
        .iter()
        .map(|root| (&root.parent, &root.parent_identity, &root.entry))
        .collect::<Vec<_>>();
    ensure!(
        captured_root_digest(&roots)? == binding.cut_roots,
        "The saved source Move identities or paths changed"
    );
    let mut paths = BTreeSet::new();
    for (index, root) in journal.roots.iter().enumerate() {
        ensure!(
            root.parent.is_absolute()
                && root.quarantine == quarantine_name(binding, index)
                && root.entry.path == [journal.manifest.roots[index].clone()],
            "The saved source Move root is invalid"
        );
        protocol::valid_component(&root.entry.name)?;
        if let Some(plan) = &root.trash_plan {
            ensure!(
                plan.original == root.original()
                    && plan.source_identity == identity_tuple(&root.entry.metadata.identity)
                    && Some(plan.quarantine_identity)
                        == root.quarantine_identity.as_ref().map(identity_tuple)
                    && plan.token == trash_token(binding, index),
                "The saved Trash plan belongs to a different source"
            );
        }
        if let Some(receipt) = &root.trash_receipt {
            ensure!(
                root.trash_plan.as_ref() == Some(&receipt.plan)
                    && receipt.item_identity == identity_tuple(&root.entry.metadata.identity)
                    && receipt.item.is_absolute(),
                "The saved Trash receipt belongs to a different source"
            );
        }
        ensure!(
            !matches!(
                root.phase,
                RootPhase::TrashIntent | RootPhase::TrashCommitted
            ) || root.trash_plan.is_some(),
            "The saved source Move is missing its Trash plan"
        );
        ensure!(
            root.phase != RootPhase::TrashCommitted || root.trash_receipt.is_some(),
            "The saved source Move is missing its Trash receipt"
        );
        validate_entry(&root.entry, &kinds, &mut paths)?;
    }
    ensure!(
        paths.len() == kinds.len(),
        "The saved source Move tree is incomplete"
    );
    Ok(())
}

fn validate_entry(
    entry: &CapturedEntry,
    kinds: &BTreeMap<Vec<String>, protocol::Kind>,
    paths: &mut BTreeSet<Vec<String>>,
) -> Result<()> {
    protocol::valid_component(&entry.name)?;
    ensure!(
        paths.len() < MAX_ENTRIES && paths.insert(entry.path.clone()),
        "The saved source Move paths collide"
    );
    let kind = kinds
        .get(&entry.path)
        .context("The saved source Move entry is unknown")?;
    ensure!(
        match kind {
            protocol::Kind::Directory => entry.metadata.directory,
            protocol::Kind::File { size, .. } =>
                !entry.metadata.directory
                    && entry.metadata.length == *size
                    && entry.children.is_empty(),
        },
        "The saved source Move entry changed kind or length"
    );
    let mut names = BTreeSet::new();
    for child in &entry.children {
        let mut path = entry.path.clone();
        path.push(child.name.clone());
        ensure!(
            child.path == path && names.insert(child.name.clone()),
            "The saved source Move tree is invalid"
        );
        validate_entry(child, kinds, paths)?;
    }
    Ok(())
}

fn manifest_kinds(manifest: &Manifest) -> BTreeMap<Vec<String>, protocol::Kind> {
    manifest
        .entries
        .iter()
        .map(|entry| (entry.path.clone(), entry.kind.clone()))
        .collect()
}

fn check_children(directory: &Dir, children: &[CapturedEntry]) -> Result<()> {
    let mut names = children
        .iter()
        .map(|child| child.name.as_str())
        .collect::<BTreeSet<_>>();
    for entry in directory.entries()? {
        let name = entry?.file_name();
        ensure!(
            name.to_str().is_some_and(|name| names.remove(name)),
            "The original folder contents changed after Cut"
        );
    }
    ensure!(names.is_empty(), "An original file was removed after Cut");
    Ok(())
}

fn verify_entry(
    parent: &Dir,
    name: &OsStr,
    entry: &CapturedEntry,
    kinds: &BTreeMap<Vec<String>, protocol::Kind>,
    renamed_root: bool,
) -> Result<()> {
    entry
        .metadata
        .check(&parent.symlink_metadata(name)?, renamed_root)?;
    if entry.metadata.directory {
        let directory = parent.open_dir_nofollow(name)?;
        entry
            .metadata
            .check(&directory.dir_metadata()?, renamed_root)?;
        check_children(&directory, &entry.children)?;
        for child in &entry.children {
            verify_entry(&directory, OsStr::new(&child.name), child, kinds, false)?;
        }
        check_children(&directory, &entry.children)?;
        entry
            .metadata
            .check(&directory.dir_metadata()?, renamed_root)?;
    } else {
        let protocol::Kind::File { size, sha256 } =
            kinds.get(&entry.path).context("Unknown source Move file")?
        else {
            bail!("The original file changed kind");
        };
        let mut file = open_file(parent, name, false, false)?;
        entry
            .metadata
            .check(&regular(&file, false)?, renamed_root)?;
        ensure!(
            finished_hash(&hash_prefix(&mut file, *size, &NO_CANCEL)?) == *sha256,
            "An original file no longer matches the verified destination bytes"
        );
        let mut extra = [0u8; 1];
        ensure!(
            file.read(&mut extra)? == 0,
            "An original file grew after Cut"
        );
        entry
            .metadata
            .check(&regular(&file, false)?, renamed_root)?;
    }
    entry
        .metadata
        .check(&parent.symlink_metadata(name)?, renamed_root)
}

fn detach_root(
    journals: &Dir,
    name: &str,
    journal: &mut Journal,
    index: usize,
    kinds: &BTreeMap<Vec<String>, protocol::Kind>,
) -> Result<()> {
    let binding = journal.binding.clone();
    let root = &mut journal.roots[index];
    let parent = root.pin_parent()?;
    root.entry
        .metadata
        .check(&parent.symlink_metadata(&root.entry.name)?, false)?;
    // A collision never reuses an earlier directory, regardless of its name.
    parent.create_dir_with(&root.quarantine, &private_builder())?;
    let quarantine = directory(&parent, OsStr::new(&root.quarantine), false)?;
    source_trash::protect_new_private_dir(&quarantine)?;
    private_directory(&quarantine)?;
    // Inherited Windows permissions can be broader than the current user.
    // Reject that newly created, still-empty directory before moving any source.
    source_trash::require_private_dir(&quarantine)?;
    let identity = Identity::of(&quarantine.dir_metadata()?)?;
    write_new(
        &quarantine,
        "owner.json",
        &serde_json::to_vec(&QuarantineOwner::new(&binding, root, identity.clone()))?,
    )?;
    sync_dir(&parent)?;
    root.quarantine_identity = Some(identity);
    root.phase = RootPhase::Intent;
    root.detail = "Source detachment started; recovery will not repeat it".into();
    persist_root(journals, name, journal, index)?;
    let root = &mut journal.roots[index];
    #[cfg(test)]
    BEFORE_RENAME.with(|hook| {
        if let Some(action) = hook.borrow_mut().take() {
            action();
        }
    });
    if let Err(error) = rename_no_replace(
        &parent,
        OsStr::new(&root.entry.name),
        &quarantine,
        OsStr::new("payload"),
        &root.entry.metadata.identity,
    ) {
        // Ordinary atomic failure did not detach anything. Pending/interrupted
        // operations remain ambiguous durable intent for later reconciliation.
        if error.downcast_ref::<std::io::Error>().is_some_and(|error| {
            !matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
            )
        }) {
            root.phase = RootPhase::Untouched;
        }
        return Err(error);
    }
    #[cfg(test)]
    AFTER_RENAME.with(|hook| {
        if let Some(action) = hook.borrow_mut().take() {
            action();
        }
    });
    sync_dir(&parent)?;
    sync_dir(&quarantine)?;
    match verify_entry(&quarantine, OsStr::new("payload"), &root.entry, kinds, true) {
        Ok(()) => {
            root.phase = RootPhase::Retained;
            root.detail = "The destination Move completed. The original is retained here for recovery; it has not been sent to Trash".into();
        }
        Err(error) => {
            // A Unix source name can be exchanged between the precheck and
            // rename. Keep every captured byte. Immediate rollback is allowed
            // only into an atomically absent original name, never by replace.
            let restore = (|| {
                let actual = Identity::of(&quarantine.symlink_metadata("payload")?)?;
                rename_no_replace(
                    &quarantine,
                    OsStr::new("payload"),
                    &parent,
                    OsStr::new(&root.entry.name),
                    &actual,
                )?;
                sync_dir(&parent)?;
                sync_dir(&quarantine)?;
                ensure!(
                    Identity::of(&parent.symlink_metadata(&root.entry.name)?)? == actual,
                    "The restored source changed identity"
                );
                Ok::<_, anyhow::Error>(())
            })();
            match restore {
                Ok(()) => {
                    root.phase = RootPhase::Restored;
                    root.detail = format!(
                        "The changed source was restored without overwriting any file: {error:#}"
                    );
                }
                Err(restore_error) => {
                    root.phase = RootPhase::NeedsRecovery;
                    root.detail = format!(
                        "A changed source was retained for recovery. Verification: {error:#}. Safe restore: {restore_error:#}"
                    );
                }
            }
        }
    }
    persist_root(journals, name, journal, index)
}

fn identity_tuple(identity: &Identity) -> (u64, u64) {
    (identity.device, identity.inode)
}

fn trash_token(binding: &SourceMoveBinding, index: usize) -> String {
    let mut hash = Sha256::new();
    hash.update(b"Desk Switch source Move Trash handoff\0");
    hash.update(binding.cut_grant.as_bytes());
    hash.update(binding.offer.as_bytes());
    hash.update((index as u64).to_le_bytes());
    format!("{:x}", hash.finalize())[..32].to_owned()
}

fn handoff_to_trash(journals: &Dir, name: &str, journal: &mut Journal, index: usize) -> Result<()> {
    let binding = journal.binding.clone();
    let root = &journal.roots[index];
    let parent = root.pin_parent()?;
    let quarantine = root.pin_quarantine(&parent, &binding)?;
    let plan = source_trash::prepare(
        &quarantine,
        &root.original(),
        identity_tuple(&root.entry.metadata.identity),
        &trash_token(&binding, index),
    )?;
    ensure!(
        plan.original == root.original()
            && plan.source_identity == identity_tuple(&root.entry.metadata.identity)
            && Some(plan.quarantine_identity)
                == root.quarantine_identity.as_ref().map(identity_tuple)
            && plan.token == trash_token(&binding, index),
        "The prepared Trash handoff does not match the verified source"
    );
    journal.roots[index].trash_plan = Some(plan.clone());
    journal.roots[index].phase = RootPhase::TrashIntent;
    persist_root(journals, name, journal, index)?;
    let receipt = source_trash::commit(&quarantine, &plan)?;
    ensure!(
        receipt.plan == plan
            && receipt.item_identity == plan.source_identity
            && receipt.item.is_absolute(),
        "The Trash completion receipt does not match the verified source"
    );
    let root = &mut journal.roots[index];
    root.trash_receipt = Some(receipt);
    root.phase = RootPhase::TrashCommitted;
    root.detail =
        "The native destination Move completed and the original is recoverable in Trash".into();
    persist_root(journals, name, journal, index)
}

fn outcome_from_live(journal: &Journal) -> SourceMoveOutcome {
    let mut needs_recovery = false;
    let locations = journal
        .roots
        .iter()
        .map(|root| {
            let retained = matches!(
                root.phase,
                RootPhase::Intent
                    | RootPhase::Retained
                    | RootPhase::TrashIntent
                    | RootPhase::NeedsRecovery
            );
            needs_recovery |= retained;
            RecoveryLocation {
                original: root.original(),
                recovery: root
                    .trash_receipt
                    .as_ref()
                    .map(|receipt| receipt.item.clone())
                    .or_else(|| retained.then(|| root.recovery())),
                detail: root.detail.clone(),
            }
        })
        .collect();
    SourceMoveOutcome {
        state: if journal
            .roots
            .iter()
            .all(|root| root.phase == RootPhase::TrashCommitted && root.trash_receipt.is_some())
        {
            SourceMoveState::Moved
        } else if needs_recovery
            || journal
                .roots
                .iter()
                .any(|root| root.phase == RootPhase::TrashCommitted)
        {
            SourceMoveState::NeedsRecovery
        } else {
            SourceMoveState::OriginalsRetained
        },
        locations,
    }
}

/// Read-only crash/duplicate reconciliation. A saved path can locate and verify
/// an owned recovery payload; it can never authorize another source detachment.
pub fn recover_source_move(
    state_dir: &Path,
    binding: &SourceMoveBinding,
) -> Result<SourceMoveOutcome> {
    let _guard = FINALIZING
        .lock()
        .map_err(|_| anyhow::anyhow!("Source Move recovery is unavailable"))?;
    let directory = journal_directory(state_dir, false)?
        .context("No source Move recovery receipt was saved")?;
    let name = binding.journal_name()?;
    reconcile(&load_journal(&directory, &name, binding)?)
}

fn reconcile(journal: &Journal) -> Result<SourceMoveOutcome> {
    // Local, short-lived recovery result; the receipt is moved out immediately.
    #[allow(clippy::large_enum_variant)]
    enum RecoveredRoot {
        Original,
        Retained,
        Trashed(TrashReceipt),
        Uncertain { paths: Vec<PathBuf>, detail: String },
    }
    let kinds = manifest_kinds(&journal.manifest);
    let mut result = outcome_from_live(journal);
    let mut committed = 0usize;
    let mut needs_recovery = false;
    for (root, location) in journal.roots.iter().zip(&mut result.locations) {
        if root.phase == RootPhase::Restored {
            // This durable state was written only after exact-identity
            // NOREPLACE rollback succeeded. A raced replacement can have a
            // different identity from the captured Cut, and must remain so.
            location.recovery = None;
            location.detail = root.detail.clone();
            continue;
        }
        if root.phase == RootPhase::TrashCommitted {
            // A durably verified completed Move is historical fact. The user
            // may subsequently empty Trash or remove the empty quarantine.
            // Such changes affect availability of recovery, not completion,
            // and never grant authority over a new file at the original path.
            committed += 1;
            let available: Result<Option<PathBuf>> = (|| {
                let parent = root.pin_parent()?;
                let quarantine = root.pin_quarantine(&parent, &journal.binding)?;
                let plan = root
                    .trash_plan
                    .as_ref()
                    .context("Missing completed Trash plan")?;
                match source_trash::inspect(&quarantine, plan)? {
                    TrashStatus::Committed(receipt) => {
                        ensure!(
                            receipt.plan == *plan && receipt.item_identity == plan.source_identity,
                            "The current Trash item does not match the completed Move"
                        );
                        Ok(Some(receipt.item))
                    }
                    _ => Ok(None),
                }
            })();
            match available {
                Ok(Some(path)) => {
                    location.recovery = Some(path);
                    location.detail =
                        "The completed Move original remains recoverable in Trash".into();
                }
                Ok(None) => {
                    location.recovery = None;
                    location.detail = "The Move completed. Its original was sent to Trash; a current recovery location is no longer available".into();
                }
                Err(error) => {
                    location.recovery = None;
                    location.detail = format!(
                        "The Move completed. Its current Trash recovery location could not be verified: {error:#}"
                    );
                }
            }
            continue;
        }
        let inspected = (|| {
            let parent = root.pin_parent()?;
            if root.quarantine_identity.is_none() {
                ensure!(
                    root.phase == RootPhase::Untouched,
                    "The interrupted source Move has no recovery identity"
                );
                return Ok(RecoveredRoot::Original);
            }
            let quarantine = root.pin_quarantine(&parent, &journal.binding)?;
            if let Some(plan) = &root.trash_plan {
                match source_trash::inspect(&quarantine, plan)? {
                    TrashStatus::Committed(receipt) => {
                        ensure!(
                            receipt.plan == *plan && receipt.item_identity == plan.source_identity,
                            "The recovered Trash receipt changed its source binding"
                        );
                        return Ok(RecoveredRoot::Trashed(receipt));
                    }
                    TrashStatus::NeedsRecovery { paths, detail } => {
                        return Ok(RecoveredRoot::Uncertain { paths, detail });
                    }
                    TrashStatus::Pending => {
                        verify_entry(
                            &quarantine,
                            OsStr::new("payload"),
                            &root.entry,
                            &kinds,
                            true,
                        )?;
                        return Ok(RecoveredRoot::Retained);
                    }
                }
            }
            if !exists(&quarantine, "payload")? {
                // This can be an interrupted intent or completed rollback.
                // Do not touch a replacement now occupying the original name.
                ensure!(
                    root.phase == RootPhase::Untouched
                        || Identity::of(&parent.symlink_metadata(&root.entry.name)?)?
                            == root.entry.metadata.identity,
                    "The recovery payload and the captured original are both absent; no source rename will be retried"
                );
                return Ok(RecoveredRoot::Original);
            }
            verify_entry(
                &quarantine,
                OsStr::new("payload"),
                &root.entry,
                &kinds,
                true,
            )?;
            Ok::<_, anyhow::Error>(RecoveredRoot::Retained)
        })();
        match inspected {
            Ok(RecoveredRoot::Trashed(receipt)) => {
                committed += 1;
                location.recovery = Some(receipt.item);
                location.detail = "The original is recoverable in Trash; recovery did not repeat any source rename".into();
            }
            Ok(RecoveredRoot::Retained) => {
                needs_recovery = true;
                location.recovery = Some(root.recovery());
                location.detail = "The original remains in its verified recovery directory; it has not been sent to Trash".into();
            }
            Ok(RecoveredRoot::Original) => {
                location.recovery = None;
                location.detail = "No source detachment will be retried. The original path was left untouched during recovery".into();
            }
            Ok(RecoveredRoot::Uncertain { paths, detail }) => {
                needs_recovery = true;
                location.recovery = paths.into_iter().next().or_else(|| Some(root.recovery()));
                location.detail = detail;
            }
            Err(error) => {
                needs_recovery = true;
                location.recovery = root.quarantine_identity.as_ref().map(|_| root.recovery());
                location.detail = format!("Retained source recovery requires attention: {error:#}");
            }
        }
    }
    result.state = if committed == journal.roots.len() {
        SourceMoveState::Moved
    } else if committed > 0 || needs_recovery {
        SourceMoveState::NeedsRecovery
    } else {
        SourceMoveState::OriginalsRetained
    };
    Ok(result)
}

/// Shared recoverable-rename primitive for the platform Trash handoff. Callers
/// must verify the moved identity afterwards; Linux's name-based primitive can
/// capture a source substitution even though its target is never overwritten.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn rename_for_trash(
    from: &Dir,
    source: &OsStr,
    to: &Dir,
    target: &OsStr,
    expected: (u64, u64),
) -> Result<()> {
    rename_no_replace(
        from,
        source,
        to,
        target,
        &Identity {
            device: expected.0,
            inode: expected.1,
        },
    )
}

fn single_component(name: &OsStr) -> Result<()> {
    let mut components = Path::new(name).components();
    ensure!(
        matches!(components.next(), Some(std::path::Component::Normal(_)))
            && components.next().is_none(),
        "A source Move rename must name one local path component"
    );
    Ok(())
}

#[cfg(target_os = "linux")]
fn rename_no_replace(
    from: &Dir,
    source: &OsStr,
    to: &Dir,
    target: &OsStr,
    _expected: &Identity,
) -> Result<()> {
    single_component(source)?;
    single_component(target)?;
    use std::{
        ffi::CString,
        os::{fd::AsRawFd, unix::ffi::OsStrExt},
    };
    let source = CString::new(source.as_bytes())?;
    let target = CString::new(target.as_bytes())?;
    // Both names are single, locally captured/generated components relative to
    // pinned directories. Linux NOREPLACE fails atomically on every collision.
    let result = unsafe {
        libc::renameat2(
            from.as_raw_fd(),
            source.as_ptr(),
            to.as_raw_fd(),
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().into())
    }
}

#[cfg(windows)]
fn rename_no_replace(
    from: &Dir,
    source: &OsStr,
    to: &Dir,
    target: &OsStr,
    expected: &Identity,
) -> Result<()> {
    single_component(source)?;
    single_component(target)?;
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
    use cap_std::fs::{OpenOptions, OpenOptionsExt};
    use std::{
        ffi::c_void,
        mem::{offset_of, size_of},
        os::windows::{ffi::OsStrExt, io::AsRawHandle},
    };

    #[repr(C)]
    struct RenameInfo {
        flags: u32,
        root_directory: *mut c_void,
        file_name_length: u32,
        file_name: [u16; 1],
    }
    #[repr(C)]
    struct IoStatusBlock {
        status_or_pointer: usize,
        information: usize,
    }
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtSetInformationFile(
            file: *mut c_void,
            status: *mut IoStatusBlock,
            info: *const c_void,
            size: u32,
            class: i32,
        ) -> i32;
        fn RtlNtStatusToDosError(status: i32) -> u32;
    }

    let mut options = OpenOptions::new();
    // DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE; no FILE_READ_DATA is needed
    // for the exact-handle rename. All reparse points fail no-follow checks.
    // Deliberately omit FILE_FLAG_OVERLAPPED: cap-primitives 4.0.3 maps this
    // open to FILE_SYNCHRONOUS_IO_NONALERT. This owned synchronous handle and
    // NtSetInformationFile keep the IO status/name buffers valid until return;
    // an asynchronous or externally supplied handle must never enter here.
    options
        .access_mode(0x0001_0000 | 0x80 | 0x0010_0000)
        .share_mode(0x1 | 0x2 | 0x4)
        .custom_flags(0x0200_0000)
        .follow(FollowSymlinks::No);
    let source_handle = from.open_with(Path::new(source), &options)?;
    ensure!(
        Identity::of(&source_handle.metadata()?)? == *expected,
        "The original source was replaced before Move"
    );
    let wide = target.encode_wide().collect::<Vec<_>>();
    let header = offset_of!(RenameInfo, file_name);
    let bytes = header + (wide.len() + 1) * size_of::<u16>();
    let mut aligned = vec![
        0usize;
        bytes
            .div_ceil(size_of::<usize>())
            .max(size_of::<RenameInfo>().div_ceil(size_of::<usize>()))
    ];
    let info = aligned.as_mut_ptr().cast::<RenameInfo>();
    // The zeroed, usize-aligned allocation includes the complete variable tail.
    // flags=0 means ReplaceIfExists=false. RootDirectory pins the target instead
    // of resolving an ambient absolute path that another process could replace.
    unsafe {
        (*info).root_directory = to.as_raw_handle();
        (*info).file_name_length = u32::try_from(wide.len() * size_of::<u16>())?;
        std::ptr::copy_nonoverlapping(
            wide.as_ptr(),
            std::ptr::addr_of_mut!((*info).file_name).cast::<u16>(),
            wide.len(),
        );
    }
    let mut completion = IoStatusBlock {
        status_or_pointer: 0,
        information: 0,
    };
    // FILE_RENAME_INFORMATION documents both no-replace behavior and a target
    // name relative to RootDirectory. FileRenameInformation is class 10:
    // https://learn.microsoft.com/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information
    let status = unsafe {
        NtSetInformationFile(
            source_handle.as_raw_handle(),
            &mut completion,
            info.cast(),
            u32::try_from(bytes)?,
            10,
        )
    };
    if status == 0 {
        Ok(())
    } else if status > 0 {
        Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "The native source rename has no synchronous completion receipt",
        )
        .into())
    } else {
        // A pending/unsupported native operation is not a completion receipt.
        // The durable intent remains available for read-only reconciliation.
        Err(
            std::io::Error::from_raw_os_error(unsafe { RtlNtStatusToDosError(status) } as i32)
                .into(),
        )
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn rename_no_replace(
    _from: &Dir,
    _source: &OsStr,
    _to: &Dir,
    _target: &OsStr,
    _expected: &Identity,
) -> Result<()> {
    bail!("This platform cannot safely finalize source Move; the originals were retained")
}

#[cfg(all(test, any(windows, target_os = "linux")))]
mod tests {
    use super::*;
    use std::fs;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            for _ in 0..128 {
                let path = std::env::temp_dir().join(format!(
                    "desk-switch-source-move-test-{}-{}",
                    std::process::id(),
                    TEMP.fetch_add(1, Ordering::Relaxed)
                ));
                match fs::create_dir(&path) {
                    Ok(()) => {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
                        }
                        return Self(path);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("Could not create isolated source Move fixture: {error}"),
                }
            }
            panic!("Could not reserve isolated source Move fixture")
        }

        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }

        fn write(&self, name: &str, bytes: &[u8]) {
            let path = self.path(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }

        fn prepare(&self, names: &[&str]) -> (SourceSelection, Manifest, SourceMoveBinding) {
            let paths = names.iter().map(|name| self.path(name)).collect::<Vec<_>>();
            let selection = SourceSelection::capture(&paths, &NO_CANCEL).unwrap();
            let export = selection
                .prepare(
                    &self.path("state"),
                    &"44".repeat(16),
                    protocol::MAX_TOTAL,
                    &NO_CANCEL,
                    |_| {},
                )
                .unwrap();
            let manifest = export.manifest.clone();
            let binding = SourceMoveBinding {
                workspace: "fixture-workspace".into(),
                peer: "11".repeat(32),
                offer: "22".repeat(16),
                paste: "33".repeat(16),
                export: manifest.id.clone(),
                manifest_digest: manifest.digest().unwrap(),
                cut_grant: "55".repeat(16),
                cut_roots: selection.cut_root_digest().unwrap(),
            };
            (selection, manifest, binding)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // Only the unique directory just reserved by this fixture is
            // removed. Production code never recursively removes sources.
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn verified_file_and_folder_are_retained_and_never_reported_as_moved() {
        let fixture = Fixture::new();
        fixture.write("source/report.txt", b"verified report");
        fixture.write("source/folder/nested.txt", b"nested data");
        fs::create_dir(fixture.path("source/folder/empty")).unwrap();
        let (selection, manifest, binding) =
            fixture.prepare(&["source/report.txt", "source/folder"]);
        let outcome = selection
            .finalize_move(&fixture.path("state"), &manifest, &binding)
            .unwrap();
        assert_eq!(outcome.state, SourceMoveState::NeedsRecovery);
        assert!(!fixture.path("source/report.txt").exists());
        assert!(!fixture.path("source/folder").exists());
        let first = outcome.locations[0].recovery.as_ref().unwrap();
        let second = outcome.locations[1].recovery.as_ref().unwrap();
        assert_eq!(fs::read(first.join("payload")).unwrap(), b"verified report");
        assert_eq!(
            fs::read(second.join("payload/nested.txt")).unwrap(),
            b"nested data"
        );
        assert!(second.join("payload/empty").is_dir());
        assert_eq!(
            recover_source_move(&fixture.path("state"), &binding)
                .unwrap()
                .state,
            SourceMoveState::NeedsRecovery
        );
    }

    #[test]
    fn every_root_must_have_a_real_fixture_trash_commit_before_moved() {
        let fixture = Fixture::new();
        fixture.write("source/report.txt", b"verified report");
        fixture.write("source/folder/nested.txt", b"nested data");
        let (selection, manifest, binding) =
            fixture.prepare(&["source/report.txt", "source/folder"]);
        source_trash::with_test_trash(&fixture.path("fake-trash"), || {
            let outcome = selection
                .finalize_move(&fixture.path("state"), &manifest, &binding)
                .unwrap();
            assert_eq!(outcome.state, SourceMoveState::Moved);
            assert!(!fixture.path("source/report.txt").exists());
            assert!(!fixture.path("source/folder").exists());
            assert_eq!(
                fs::read(outcome.locations[0].recovery.as_ref().unwrap()).unwrap(),
                b"verified report"
            );
            assert_eq!(
                fs::read(
                    outcome.locations[1]
                        .recovery
                        .as_ref()
                        .unwrap()
                        .join("nested.txt")
                )
                .unwrap(),
                b"nested data"
            );
            fixture.write("source/report.txt", b"new user file");
            let recovered = recover_source_move(&fixture.path("state"), &binding).unwrap();
            assert_eq!(recovered.state, SourceMoveState::Moved);
            assert_eq!(
                fs::read(fixture.path("source/report.txt")).unwrap(),
                b"new user file"
            );
        });
    }

    #[test]
    fn completed_move_stays_completed_after_fixture_trash_is_emptied() {
        let fixture = Fixture::new();
        fixture.write("source/report.txt", b"original");
        let (selection, manifest, binding) = fixture.prepare(&["source/report.txt"]);
        source_trash::with_test_trash(&fixture.path("fake-trash"), || {
            let outcome = selection
                .finalize_move(&fixture.path("state"), &manifest, &binding)
                .unwrap();
            assert_eq!(outcome.state, SourceMoveState::Moved);
            // Simulate the user emptying only this explicit fixture's Trash.
            fs::remove_file(outcome.locations[0].recovery.as_ref().unwrap()).unwrap();
            fixture.write("source/report.txt", b"new user file");
            let recovered = recover_source_move(&fixture.path("state"), &binding).unwrap();
            assert_eq!(recovered.state, SourceMoveState::Moved);
            assert!(recovered.locations[0].recovery.is_none());
            fs::remove_dir_all(fixture.path("source").join(quarantine_name(&binding, 0))).unwrap();
            let recovered = recover_source_move(&fixture.path("state"), &binding).unwrap();
            assert_eq!(recovered.state, SourceMoveState::Moved);
            assert!(recovered.locations[0].recovery.is_none());
            assert_eq!(
                fs::read(fixture.path("source/report.txt")).unwrap(),
                b"new user file"
            );
        });
    }

    #[test]
    fn duplicate_receipt_never_detaches_a_recreated_source() {
        let fixture = Fixture::new();
        fixture.write("source/report.txt", b"first version");
        let (selection, manifest, binding) = fixture.prepare(&["source/report.txt"]);
        let duplicate =
            SourceSelection::capture(&[fixture.path("source/report.txt")], &NO_CANCEL).unwrap();
        assert_eq!(duplicate.cut_root_digest().unwrap(), binding.cut_roots);
        selection
            .finalize_move(&fixture.path("state"), &manifest, &binding)
            .unwrap();
        fixture.write("source/report.txt", b"a new user file at the same path");
        let outcome = duplicate
            .finalize_move(&fixture.path("state"), &manifest, &binding)
            .unwrap();
        assert_eq!(outcome.state, SourceMoveState::NeedsRecovery);
        assert_eq!(
            fs::read(fixture.path("source/report.txt")).unwrap(),
            b"a new user file at the same path"
        );
    }

    #[test]
    fn a_changed_source_prevents_any_root_detachment() {
        let fixture = Fixture::new();
        fixture.write("source/first.txt", b"first");
        fixture.write("source/second.txt", b"second");
        let (selection, manifest, binding) =
            fixture.prepare(&["source/first.txt", "source/second.txt"]);
        fixture.write(
            "source/second.txt",
            b"changed by the user after the destination commit",
        );
        let outcome = selection
            .finalize_move(&fixture.path("state"), &manifest, &binding)
            .unwrap();
        assert_eq!(outcome.state, SourceMoveState::OriginalsRetained);
        assert!(
            outcome
                .locations
                .iter()
                .all(|location| location.recovery.is_none())
        );
        assert_eq!(
            fs::read(fixture.path("source/first.txt")).unwrap(),
            b"first"
        );
        assert_eq!(
            fs::read(fixture.path("source/second.txt")).unwrap(),
            b"changed by the user after the destination commit"
        );
    }

    #[test]
    fn second_paste_or_peer_cannot_reuse_a_cut_journal() {
        let fixture = Fixture::new();
        fixture.write("source/report.txt", b"original");
        let (selection, manifest, binding) = fixture.prepare(&["source/report.txt"]);
        selection
            .finalize_move(&fixture.path("state"), &manifest, &binding)
            .unwrap();
        fixture.write("source/report.txt", b"replacement");
        for changed in ["paste", "peer", "manifest", "cut_roots"] {
            let mut other = binding.clone();
            match changed {
                "paste" => other.paste = "66".repeat(16),
                "peer" => other.peer = "77".repeat(32),
                "manifest" => other.manifest_digest = "88".repeat(32),
                "cut_roots" => other.cut_roots = "99".repeat(32),
                _ => unreachable!(),
            }
            assert!(recover_source_move(&fixture.path("state"), &other).is_err());
        }
        assert_eq!(
            fs::read(fixture.path("source/report.txt")).unwrap(),
            b"replacement"
        );
    }

    fn interrupted(
        fixture: &Fixture,
        selection: &SourceSelection,
        manifest: &Manifest,
        binding: &SourceMoveBinding,
        detach: bool,
    ) {
        let source = &selection.roots[0];
        let parent = absolute_directory(&source.parent_path, false).unwrap();
        let mut root = RootRecord {
            parent: source.parent_path.clone(),
            parent_identity: Identity::from(&source.parent_identity),
            entry: CapturedEntry::from_source(&source.entry).unwrap(),
            quarantine: quarantine_name(binding, 0),
            quarantine_identity: None,
            trash_plan: None,
            trash_receipt: None,
            phase: RootPhase::Intent,
            detail: "Fixture interrupted before receipt completion".into(),
        };
        parent
            .create_dir_with(&root.quarantine, &private_builder())
            .unwrap();
        let quarantine = directory(&parent, OsStr::new(&root.quarantine), false).unwrap();
        let identity = Identity::of(&quarantine.dir_metadata().unwrap()).unwrap();
        write_new(
            &quarantine,
            "owner.json",
            &serde_json::to_vec(&QuarantineOwner::new(binding, &root, identity.clone())).unwrap(),
        )
        .unwrap();
        root.quarantine_identity = Some(identity);
        let mut journal = Journal {
            format: MOVE_FORMAT,
            binding: binding.clone(),
            manifest: manifest.clone(),
            roots: vec![root],
        };
        let journals = journal_directory(&fixture.path("state"), true)
            .unwrap()
            .unwrap();
        let intent = journal.roots[0].clone();
        journal.roots[0].phase = RootPhase::Untouched;
        journal.roots[0].quarantine_identity = None;
        let name = binding.journal_name().unwrap();
        create_checkpoints(&journals, &name, binding).unwrap();
        write_new(&journals, &name, &journal_bytes(&journal).unwrap()).unwrap();
        journal.roots[0] = intent;
        persist_root(&journals, &name, &journal, 0).unwrap();
        if detach {
            rename_no_replace(
                &parent,
                &source.entry.name,
                &quarantine,
                OsStr::new("payload"),
                &journal.roots[0].entry.metadata.identity,
            )
            .unwrap();
        }
    }

    #[test]
    fn crash_before_rename_never_retries_the_original_name() {
        let fixture = Fixture::new();
        fixture.write("source/report.txt", b"original");
        let (selection, manifest, binding) = fixture.prepare(&["source/report.txt"]);
        interrupted(&fixture, &selection, &manifest, &binding, false);
        fs::rename(
            fixture.path("source/report.txt"),
            fixture.path("source/parked.txt"),
        )
        .unwrap();
        fixture.write("source/report.txt", b"replacement after interruption");
        let outcome = selection
            .finalize_move(&fixture.path("state"), &manifest, &binding)
            .unwrap();
        assert_eq!(outcome.state, SourceMoveState::NeedsRecovery);
        assert_eq!(
            fs::read(fixture.path("source/report.txt")).unwrap(),
            b"replacement after interruption"
        );
        assert_eq!(
            fs::read(fixture.path("source/parked.txt")).unwrap(),
            b"original"
        );
    }

    #[test]
    fn crash_after_rename_recovers_the_owned_payload_without_replaying() {
        let fixture = Fixture::new();
        fixture.write("source/report.txt", b"original");
        let (selection, manifest, binding) = fixture.prepare(&["source/report.txt"]);
        interrupted(&fixture, &selection, &manifest, &binding, true);
        fixture.write("source/report.txt", b"replacement after detachment");
        let outcome = recover_source_move(&fixture.path("state"), &binding).unwrap();
        assert_eq!(outcome.state, SourceMoveState::NeedsRecovery);
        assert_eq!(
            fs::read(
                outcome.locations[0]
                    .recovery
                    .as_ref()
                    .unwrap()
                    .join("payload")
            )
            .unwrap(),
            b"original"
        );
        assert_eq!(
            fs::read(fixture.path("source/report.txt")).unwrap(),
            b"replacement after detachment"
        );
    }

    #[test]
    fn crash_after_trash_commit_before_journal_receipt_is_reconciled_read_only() {
        let fixture = Fixture::new();
        fixture.write("source/report.txt", b"original");
        let (selection, manifest, binding) = fixture.prepare(&["source/report.txt"]);
        interrupted(&fixture, &selection, &manifest, &binding, true);
        source_trash::with_test_trash(&fixture.path("fake-trash"), || {
            let journals = journal_directory(&fixture.path("state"), false)
                .unwrap()
                .unwrap();
            let name = binding.journal_name().unwrap();
            let mut journal = load_journal(&journals, &name, &binding).unwrap();
            let parent = journal.roots[0].pin_parent().unwrap();
            let quarantine = journal.roots[0].pin_quarantine(&parent, &binding).unwrap();
            let plan = source_trash::prepare(
                &quarantine,
                &journal.roots[0].original(),
                identity_tuple(&journal.roots[0].entry.metadata.identity),
                &trash_token(&binding, 0),
            )
            .unwrap();
            journal.roots[0].trash_plan = Some(plan.clone());
            journal.roots[0].phase = RootPhase::TrashIntent;
            persist_root(&journals, &name, &journal, 0).unwrap();
            let committed = source_trash::commit(&quarantine, &plan).unwrap();
            fixture.write("source/report.txt", b"new file after committed Trash");
            let outcome = recover_source_move(&fixture.path("state"), &binding).unwrap();
            assert_eq!(outcome.state, SourceMoveState::Moved);
            assert_eq!(
                outcome.locations[0].recovery.as_ref(),
                Some(&committed.item)
            );
            assert_eq!(
                fs::read(fixture.path("source/report.txt")).unwrap(),
                b"new file after committed Trash"
            );
            assert_eq!(fs::read(committed.item).unwrap(), b"original");
            assert_eq!(
                load_journal(&journals, &name, &binding).unwrap().roots[0].phase,
                RootPhase::TrashIntent
            );
        });
    }

    #[test]
    fn no_replace_rename_never_overwrites_a_collision() {
        let fixture = Fixture::new();
        fixture.write("source/original.txt", b"original");
        fixture.write("source/existing.txt", b"existing");
        let parent = absolute_directory(&fixture.path("source"), false).unwrap();
        let expected = Identity::of(&parent.symlink_metadata("original.txt").unwrap()).unwrap();
        assert!(
            rename_no_replace(
                &parent,
                OsStr::new("original.txt"),
                &parent,
                OsStr::new("existing.txt"),
                &expected
            )
            .is_err()
        );
        assert_eq!(
            fs::read(fixture.path("source/original.txt")).unwrap(),
            b"original"
        );
        assert_eq!(
            fs::read(fixture.path("source/existing.txt")).unwrap(),
            b"existing"
        );
    }

    #[test]
    fn shared_rename_primitive_rejects_parent_path_authority() {
        let fixture = Fixture::new();
        fixture.write("outside.txt", b"unrelated fixture data");
        fs::create_dir(fixture.path("source")).unwrap();
        let parent = absolute_directory(&fixture.0, false).unwrap();
        let outside = Identity::of(&parent.symlink_metadata("outside.txt").unwrap()).unwrap();
        let confined = absolute_directory(&fixture.path("source"), false).unwrap();
        assert!(
            rename_for_trash(
                &confined,
                OsStr::new("../outside.txt"),
                &confined,
                OsStr::new("payload"),
                identity_tuple(&outside)
            )
            .is_err()
        );
        assert_eq!(
            fs::read(fixture.path("outside.txt")).unwrap(),
            b"unrelated fixture data"
        );
        assert!(!fixture.path("source/payload").exists());
    }

    #[test]
    fn duplicate_and_overlapping_cut_roots_are_rejected_without_reading_bytes() {
        let fixture = Fixture::new();
        fixture.write("source/folder/child.txt", b"contents");
        for names in [
            ["source/folder", "source/folder"],
            ["source/folder", "source/folder/child.txt"],
        ] {
            let paths = names
                .iter()
                .map(|name| fixture.path(name))
                .collect::<Vec<_>>();
            let selection = SourceSelection::capture(&paths, &NO_CANCEL).unwrap();
            assert!(selection.cut_root_digest().is_err());
        }
        assert_eq!(
            fs::read(fixture.path("source/folder/child.txt")).unwrap(),
            b"contents"
        );
    }

    #[test]
    fn captured_deep_tree_round_trips_with_normal_json_parser_limits() {
        let fixture = Fixture::new();
        let mut name = "source/r".to_owned();
        for _ in 0..62 {
            name.push_str("/d");
        }
        name.push_str("/f");
        fixture.write(&name, b"deep");
        let selection = SourceSelection::capture(&[fixture.path("source/r")], &NO_CANCEL).unwrap();
        let captured = CapturedEntry::from_source(&selection.roots[0].entry).unwrap();
        let encoded = serde_json::to_vec(&captured).unwrap();
        let decoded: CapturedEntry = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, captured);
    }

    #[test]
    fn ten_thousand_roots_have_a_linear_encoded_checkpoint_write_budget() {
        let fixture = Fixture::new();
        fixture.write("source/template", b"");
        let (selection, mut manifest, mut binding) = fixture.prepare(&["source/template"]);
        let source = &selection.roots[0];
        let metadata = CapturedEntry::from_source(&source.entry).unwrap().metadata;
        manifest.roots.clear();
        manifest.entries.clear();
        manifest.name = format!("{MAX_ENTRIES} items");
        let mut roots = Vec::new();
        for index in 0..MAX_ENTRIES {
            let name = format!("f{index:08x}");
            let mut metadata = metadata.clone();
            metadata.identity.inode = metadata.identity.inode.wrapping_add(index as u64 + 1);
            manifest.roots.push(name.clone());
            manifest.entries.push(protocol::Entry {
                path: vec![name.clone()],
                kind: protocol::Kind::File {
                    size: 0,
                    sha256: protocol::hash(b""),
                },
            });
            roots.push(RootRecord {
                parent: source.parent_path.clone(),
                parent_identity: Identity::from(&source.parent_identity),
                entry: CapturedEntry {
                    name: name.clone(),
                    path: vec![name],
                    metadata,
                    children: Vec::new(),
                },
                quarantine: quarantine_name(&binding, index),
                quarantine_identity: None,
                trash_plan: None,
                trash_receipt: None,
                phase: RootPhase::Untouched,
                detail: "The original source was retained".into(),
            });
        }
        binding.manifest_digest = manifest.digest().unwrap();
        let captured = roots
            .iter()
            .map(|root| (&root.parent, &root.parent_identity, &root.entry))
            .collect::<Vec<_>>();
        binding.cut_roots = captured_root_digest(&captured).unwrap();
        let mut journal = Journal {
            format: MOVE_FORMAT,
            binding,
            manifest,
            roots,
        };
        validate_journal(&journal, &journal.binding).unwrap();
        let mut encoded_write_bytes = journal_bytes(&journal).unwrap().len();
        for index in 0..MAX_ENTRIES {
            let identity = identity_tuple(&journal.roots[index].entry.metadata.identity);
            let quarantine_identity = (identity.0, identity.1.wrapping_add(MAX_ENTRIES as u64 + 1));
            let token = trash_token(&journal.binding, index);
            let plan = TrashPlan {
                format: MOVE_FORMAT,
                backend: source_trash::TrashBackend::Freedesktop,
                token: token.clone(),
                original: journal.roots[index].original(),
                source_identity: identity,
                quarantine_identity,
                target: fixture
                    .path("fake-trash/files")
                    .join(format!("desk-switch-{token}")),
                info: Some(
                    fixture
                        .path("fake-trash/info")
                        .join(format!("desk-switch-{token}.trashinfo")),
                ),
                target_parent_identity: (identity.0, 1),
                info_parent_identity: Some((identity.0, 2)),
                deletion_date: "2026-11-08T00:00:00".into(),
            };
            journal.roots[index].quarantine_identity = Some(Identity {
                device: quarantine_identity.0,
                inode: quarantine_identity.1,
            });
            for phase in [
                RootPhase::Intent,
                RootPhase::Retained,
                RootPhase::TrashIntent,
                RootPhase::TrashCommitted,
            ] {
                journal.roots[index].phase = phase;
                if matches!(phase, RootPhase::TrashIntent | RootPhase::TrashCommitted) {
                    journal.roots[index].trash_plan = Some(plan.clone());
                }
                if phase == RootPhase::TrashCommitted {
                    journal.roots[index].trash_receipt = Some(TrashReceipt {
                        plan: plan.clone(),
                        item: plan.target.clone(),
                        item_identity: identity,
                    });
                }
                let checkpoint = checkpoint_bytes(&journal, index).unwrap();
                // Each call must encode only this root, never the 10k-root
                // manifest/tree. A regression fails on its first large record.
                assert!(checkpoint.len() < 16 * 1024);
                encoded_write_bytes += checkpoint.len();
            }
        }
        assert!(
            encoded_write_bytes < 256 * 1024 * 1024,
            "Encoded {encoded_write_bytes} bytes for the 10k-root batch"
        );
    }

    #[test]
    fn modified_journal_paths_cannot_become_recovery_authority() {
        let fixture = Fixture::new();
        fixture.write("source/report.txt", b"original");
        fixture.write("unrelated/report.txt", b"unrelated user data");
        let (selection, manifest, binding) = fixture.prepare(&["source/report.txt"]);
        interrupted(&fixture, &selection, &manifest, &binding, false);
        let directory = journal_directory(&fixture.path("state"), false)
            .unwrap()
            .unwrap();
        let name = binding.journal_name().unwrap();
        let mut journal = load_journal(&directory, &name, &binding).unwrap();
        journal.roots[0].parent = fixture.path("unrelated");
        write_atomic(&directory, &name, &journal_bytes(&journal).unwrap()).unwrap();
        assert!(recover_source_move(&fixture.path("state"), &binding).is_err());
        assert_eq!(
            fs::read(fixture.path("unrelated/report.txt")).unwrap(),
            b"unrelated user data"
        );
        assert_eq!(
            fs::read(fixture.path("source/report.txt")).unwrap(),
            b"original"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_replacement_between_precheck_and_rename_is_restored_without_deletion() {
        let fixture = Fixture::new();
        fixture.write("source/report.txt", b"original bytes");
        let (selection, manifest, binding) = fixture.prepare(&["source/report.txt"]);
        let original = fixture.path("source/report.txt");
        let parked = fixture.path("source/parked.txt");
        BEFORE_RENAME.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::rename(&original, parked).unwrap();
                fs::write(original, b"racing replacement").unwrap();
            }))
        });
        let outcome = selection
            .finalize_move(&fixture.path("state"), &manifest, &binding)
            .unwrap();
        assert_eq!(outcome.state, SourceMoveState::OriginalsRetained);
        assert_eq!(
            fs::read(fixture.path("source/report.txt")).unwrap(),
            b"racing replacement"
        );
        assert_eq!(
            fs::read(fixture.path("source/parked.txt")).unwrap(),
            b"original bytes"
        );
        assert_eq!(
            recover_source_move(&fixture.path("state"), &binding)
                .unwrap()
                .state,
            SourceMoveState::OriginalsRetained
        );
        assert_eq!(
            fs::read(fixture.path("source/report.txt")).unwrap(),
            b"racing replacement"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rollback_conflict_retains_both_source_and_recovery_payload() {
        let fixture = Fixture::new();
        fixture.write("source/report.txt", b"original bytes");
        let (selection, manifest, binding) = fixture.prepare(&["source/report.txt"]);
        let original = fixture.path("source/report.txt");
        let parked = fixture.path("source/parked.txt");
        BEFORE_RENAME.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::rename(&original, parked).unwrap();
                fs::write(original, b"racing replacement").unwrap();
            }))
        });
        let source = fixture.path("source/report.txt");
        AFTER_RENAME.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::write(source, b"new source occupant").unwrap()
            }))
        });
        let outcome = selection
            .finalize_move(&fixture.path("state"), &manifest, &binding)
            .unwrap();
        assert_eq!(outcome.state, SourceMoveState::NeedsRecovery);
        assert_eq!(
            fs::read(fixture.path("source/report.txt")).unwrap(),
            b"new source occupant"
        );
        assert_eq!(
            fs::read(fixture.path("source/parked.txt")).unwrap(),
            b"original bytes"
        );
        assert_eq!(
            fs::read(
                outcome.locations[0]
                    .recovery
                    .as_ref()
                    .unwrap()
                    .join("payload")
            )
            .unwrap(),
            b"racing replacement"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_partially_trashed_batch_is_never_reported_as_moved() {
        let fixture = Fixture::new();
        fixture.write("source/first.txt", b"first original");
        fixture.write("source/second.txt", b"second original");
        let (selection, manifest, binding) =
            fixture.prepare(&["source/first.txt", "source/second.txt"]);
        let second = fixture.path("source/second.txt");
        let parked = fixture.path("source/parked-second.txt");
        AFTER_RENAME.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                BEFORE_RENAME.with(|next| {
                    *next.borrow_mut() = Some(Box::new(move || {
                        fs::rename(&second, parked).unwrap();
                        fs::write(second, b"racing second replacement").unwrap();
                    }))
                });
            }))
        });
        source_trash::with_test_trash(&fixture.path("fake-trash"), || {
            let outcome = selection
                .finalize_move(&fixture.path("state"), &manifest, &binding)
                .unwrap();
            assert_eq!(outcome.state, SourceMoveState::NeedsRecovery);
            assert_eq!(
                fs::read(outcome.locations[0].recovery.as_ref().unwrap()).unwrap(),
                b"first original"
            );
            assert_eq!(
                fs::read(fixture.path("source/second.txt")).unwrap(),
                b"racing second replacement"
            );
            assert_eq!(
                fs::read(fixture.path("source/parked-second.txt")).unwrap(),
                b"second original"
            );
        });
    }

    #[cfg(unix)]
    #[test]
    fn a_replaced_symlink_is_never_followed_or_detached() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        fixture.write("source/folder/child.txt", b"original");
        fixture.write("outside.txt", b"unrelated data");
        let (selection, manifest, binding) = fixture.prepare(&["source/folder"]);
        fs::remove_file(fixture.path("source/folder/child.txt")).unwrap();
        symlink(
            fixture.path("outside.txt"),
            fixture.path("source/folder/child.txt"),
        )
        .unwrap();
        let outcome = selection
            .finalize_move(&fixture.path("state"), &manifest, &binding)
            .unwrap();
        assert_eq!(outcome.state, SourceMoveState::OriginalsRetained);
        assert!(
            fs::symlink_metadata(fixture.path("source/folder/child.txt"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read(fixture.path("outside.txt")).unwrap(),
            b"unrelated data"
        );
    }
}
