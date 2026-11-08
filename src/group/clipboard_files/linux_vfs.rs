//! Native file URIs backed by a private, demand-driven FUSE filesystem.
//! Metadata requests never prepare or transfer file contents.
use super::super::provider::{
    CompletionSupport, DemandProvider, NativeConsumer, NativeResult, PasteIntent, PasteSession,
    PublishedOffer, RemoteOffer,
};
use crate::group::file_transfer::protocol::{
    Description, DescriptorKind, Operation, hash, valid_id,
};
use anyhow::{Context, Result, bail, ensure};
use fuser::{
    BackgroundSession, Config, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags,
    Generation, INodeNo, LockOwner, MountOption, OpenAccMode, OpenFlags, ReplyAttr, ReplyData,
    ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyXattr, Request, SessionACL,
};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    ffi::OsStr,
    fs::{self, DirBuilder, File},
    io,
    os::unix::fs::{DirBuilderExt, FileExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const ROOT: INodeNo = INodeNo(1);
const MAX_HANDLES: usize = 128;
const MAX_QUEUED_READS: usize = 64;
const MAX_READ: usize = 128 * 1024;
const MAX_RETAINED_OFFERS: usize = 32;
const RETIRED_TTL: Duration = Duration::from_secs(5 * 60);
const ATTR_TTL: Duration = Duration::from_secs(1);

thread_local! {
    // A folder paste may close its directory scan before opening its files.
    // Keep a bounded metadata capability across that native operation gap.
    static MOUNTS: RefCell<Vec<Arc<Mounted>>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone)]
struct Node {
    parent: INodeNo,
    index: Option<usize>,
    attributes: FileAttr,
    children: BTreeMap<String, INodeNo>,
}

struct Tree {
    nodes: BTreeMap<INodeNo, Node>,
}

impl Tree {
    fn new(description: &Description, uid: u32, gid: u32) -> Result<Self> {
        description.validate()?;
        let now = SystemTime::now();
        let mut nodes = BTreeMap::new();
        let mut paths = BTreeMap::new();
        nodes.insert(
            ROOT,
            Self::node(ROOT, None, DescriptorKind::Directory, uid, gid, now),
        );
        for (index, descriptor) in description.entries.iter().enumerate() {
            let ino = INodeNo(
                u64::try_from(index)?
                    .checked_add(2)
                    .context("Too many copied files")?,
            );
            paths.insert(descriptor.path.clone(), ino);
            nodes.insert(
                ino,
                Self::node(ROOT, Some(index), descriptor.kind, uid, gid, now),
            );
        }
        for descriptor in &description.entries {
            let ino = paths[&descriptor.path];
            let parent = if descriptor.path.len() == 1 {
                ROOT
            } else {
                paths[&descriptor.path[..descriptor.path.len() - 1]]
            };
            nodes.get_mut(&ino).unwrap().parent = parent;
            let parent = nodes.get_mut(&parent).unwrap();
            parent
                .children
                .insert(descriptor.path.last().unwrap().clone(), ino);
            if matches!(descriptor.kind, DescriptorKind::Directory) {
                parent.attributes.nlink += 1;
            }
        }
        Ok(Self { nodes })
    }

    fn node(
        parent: INodeNo,
        index: Option<usize>,
        kind: DescriptorKind,
        uid: u32,
        gid: u32,
        now: SystemTime,
    ) -> Node {
        let (kind, size, perm, nlink) = match kind {
            DescriptorKind::Directory => (FileType::Directory, 4096, 0o755, 2),
            DescriptorKind::File { size } => (FileType::RegularFile, size, 0o644, 1),
        };
        Node {
            parent,
            index,
            attributes: FileAttr {
                ino: ROOT,
                size,
                blocks: size.div_ceil(512),
                atime: now,
                mtime: now,
                ctime: now,
                crtime: now,
                kind,
                perm,
                nlink,
                uid,
                gid,
                rdev: 0,
                blksize: 4096,
                flags: 0,
            },
            children: BTreeMap::new(),
        }
    }

    fn node_at(&self, ino: INodeNo) -> Option<&Node> {
        self.nodes.get(&ino)
    }

    fn attributes(&self, ino: INodeNo) -> Option<FileAttr> {
        let mut attributes = self.node_at(ino)?.attributes;
        attributes.ino = ino;
        Some(attributes)
    }

    fn lookup(&self, parent: INodeNo, name: &OsStr) -> Option<INodeNo> {
        let node = self.node_at(parent)?;
        match name.to_str()? {
            "." => Some(parent),
            ".." => Some(node.parent),
            name => node.children.get(name).copied(),
        }
    }
}

/// A FUSE request may identify a GTask thread rather than the process leader.
/// Pin the process start time as well so a reused numeric PID is never a receipt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Consumer {
    uid: u32,
    tgid: u32,
    started: u64,
}

impl Consumer {
    fn resolve(tid: u32, uid: u32) -> Result<Self> {
        ensure!(tid != 0, "The paste consumer is unavailable");
        let status = fs::read_to_string(format!("/proc/{tid}/status"))
            .context("The paste consumer has exited")?;
        ensure!(status.len() <= 32 * 1024, "Invalid paste consumer identity");
        let tgid = status
            .lines()
            .find_map(|line| line.strip_prefix("Tgid:"))
            .context("The paste consumer has no process identity")?
            .trim()
            .parse::<u32>()?;
        let stat = fs::read_to_string(format!("/proc/{tgid}/stat"))
            .context("The paste consumer has exited")?;
        ensure!(stat.len() <= 32 * 1024, "Invalid paste consumer identity");
        let after_name = stat
            .rfind(") ")
            .map(|offset| &stat[offset + 2..])
            .context("Invalid paste consumer process identity")?;
        let started = after_name
            .split_ascii_whitespace()
            .nth(19)
            .context("Invalid paste consumer process start time")?
            .parse::<u64>()?;
        ensure!(tgid != 0 && started != 0, "Invalid paste consumer identity");
        Ok(Self { uid, tgid, started })
    }
}

#[derive(Default)]
struct Coverage {
    ranges: BTreeMap<u64, u64>,
}

impl Coverage {
    fn record(&mut self, offset: u64, length: usize, size: u64) -> Result<()> {
        let mut end = offset
            .checked_add(u64::try_from(length)?)
            .context("Invalid paste read")?;
        ensure!(offset <= size && end <= size, "Invalid paste read");
        if length == 0 {
            return Ok(());
        }
        let mut start = offset;
        if let Some((&left, &right)) = self.ranges.range(..=start).next_back()
            && right >= start
        {
            start = left;
            end = end.max(right);
        }
        let overlapping = self
            .ranges
            .range(start..=end)
            .map(|(&left, &right)| (left, right))
            .collect::<Vec<_>>();
        for (left, right) in overlapping {
            self.ranges.remove(&left);
            end = end.max(right);
        }
        self.ranges.insert(start, end);
        Ok(())
    }

    fn complete(&self, size: u64) -> bool {
        size == 0 || self.ranges.get(&0).is_some_and(|end| *end == size)
    }
}

fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct OpenHandle {
    ino: INodeNo,
    consumer: Consumer,
    authority: Arc<dyn DemandProvider>,
    session: Mutex<Option<Arc<dyn PasteSession>>>,
    file: Mutex<Option<File>>,
    batch: Mutex<Option<Arc<MoveBatch>>>,
}

struct MoveBatch {
    owner: Consumer,
    consumer: NativeConsumer,
    session: Mutex<Option<Arc<dyn PasteSession>>>,
    record: Mutex<MoveRecord>,
}

#[derive(Default)]
struct MoveRecord {
    coverage: BTreeMap<usize, Coverage>,
    readers: BTreeMap<usize, u64>,
    receipts: BTreeMap<usize, bool>,
    ambiguous: bool,
    committed: bool,
    committing: Option<usize>,
}

enum Work {
    Read {
        state: Arc<State>,
        handle: Arc<OpenHandle>,
        number: u64,
        offset: u64,
        length: usize,
        reply: ReplyData,
    },
    Remove {
        state: Arc<State>,
        consumer: Consumer,
        batch: Arc<MoveBatch>,
        index: usize,
        reply: ReplyEmpty,
    },
}

struct Pool {
    sender: mpsc::SyncSender<Work>,
}
impl Pool {
    fn new() -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(MAX_QUEUED_READS);
        let receiver = Arc::new(Mutex::new(receiver));
        for index in 0..2 {
            let receiver = receiver.clone();
            std::thread::Builder::new()
                .name(format!("Desk Switch file paste {index}"))
                .spawn(move || {
                    loop {
                        let Ok(work) = lock(&receiver).recv() else {
                            break;
                        };
                        match work {
                            Work::Read {
                                state,
                                handle,
                                number,
                                offset,
                                length,
                                reply,
                            } => {
                                match state.read_data(&handle, number, offset, length) {
                                    Ok(bytes) => reply.data(&bytes),
                                    Err(error) => {
                                        state.fail_handle(&handle, &error);
                                        reply.error(Errno::EIO);
                                    }
                                }
                                state.pending.fetch_sub(1, Ordering::AcqRel);
                            }
                            Work::Remove {
                                state,
                                consumer,
                                batch,
                                index,
                                reply,
                            } => {
                                match state.remove_data(consumer, &batch, index) {
                                    Ok(()) => reply.ok(),
                                    Err(error) => {
                                        if let Some(session) = lock(&batch.session).as_ref() {
                                            let _ = session.native_complete(NativeResult::Failed(
                                                format!("{error:#}"),
                                            ));
                                        }
                                        reply.error(Errno::EIO);
                                    }
                                }
                                state.pending.fetch_sub(1, Ordering::AcqRel);
                            }
                        }
                    }
                })?;
        }
        Ok(Self { sender })
    }
}

struct State {
    offer: RemoteOffer,
    authority: Arc<dyn DemandProvider>,
    tree: Tree,
    uid: u32,
    opened: Mutex<BTreeMap<u64, Arc<OpenHandle>>>,
    movement: Mutex<Option<Arc<MoveBatch>>>,
    serial: AtomicU64,
    pending: AtomicUsize,
    closed: AtomicBool,
    retired: Mutex<Option<Instant>>,
    activity: Mutex<Instant>,
    pool: OnceLock<Result<Pool, String>>,
}

impl State {
    fn new(offer: RemoteOffer) -> Result<Arc<Self>> {
        valid_id(&offer.id)?;
        offer.description.validate()?;
        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };
        // This pins metadata only. It keeps a URI obtained by a native folder
        // scan usable when the clipboard changes before the first file opens.
        let authority = offer.provider.lease()?;
        let tree = Tree::new(&offer.description, uid, gid)?;
        Ok(Arc::new(Self {
            offer,
            authority,
            tree,
            uid,
            opened: Mutex::new(BTreeMap::new()),
            movement: Mutex::new(None),
            serial: AtomicU64::new(1),
            pending: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            retired: Mutex::new(None),
            activity: Mutex::new(Instant::now()),
            pool: OnceLock::new(),
        }))
    }
    fn touch(&self) {
        *lock(&self.activity) = Instant::now();
    }
    fn retained(&self) -> bool {
        lock(&self.retired).is_none()
            || !lock(&self.opened).is_empty()
            || self.pending.load(Ordering::Acquire) != 0
            || lock(&self.activity).elapsed() < RETIRED_TTL
    }
    fn allowed(&self) -> Result<()> {
        ensure!(
            !self.closed.load(Ordering::Acquire) && self.retained(),
            "The copied files have expired; copy them again"
        );
        Ok(())
    }
    fn next_serial(&self) -> Result<u64> {
        self.serial
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |next| {
                next.checked_add(1)
            })
            .map_err(|_| anyhow::anyhow!("The file paste handle limit was reached"))
    }
    fn open(&self, ino: INodeNo, consumer: Consumer) -> Result<u64> {
        self.allowed()?;
        ensure!(
            consumer.uid == self.uid,
            "The paste consumer is not authorized"
        );
        self.reset_cancelled_move();
        let authority = self.authority.lease()?;
        let mut opened = lock(&self.opened);
        ensure!(
            opened.len() < MAX_HANDLES,
            "Too many concurrent file pastes"
        );
        let number = self.next_serial()?;
        opened.insert(
            number,
            Arc::new(OpenHandle {
                ino,
                consumer,
                authority,
                session: Mutex::new(None),
                file: Mutex::new(None),
                batch: Mutex::new(None),
            }),
        );
        self.touch();
        Ok(number)
    }
    fn close_handle(&self, number: u64) {
        lock(&self.opened).remove(&number);
        self.touch();
    }
    fn reset_cancelled_move(&self) {
        if self.pending.load(Ordering::Acquire) != 0 || !lock(&self.opened).is_empty() {
            return;
        }
        let mut movement = lock(&self.movement);
        let Some(batch) = movement.as_ref() else {
            return;
        };
        // A new native paste must never inherit a previous partial receipt.
        // Metadata callbacks must not wait for a worker initializing a session.
        let Ok(session) = batch.session.try_lock() else {
            return;
        };
        let record = lock(&batch.record);
        let retryable = !record.committed
            && record.committing.is_none()
            && match session.as_ref() {
                Some(session) => session.can_retry_move(),
                None => record.receipts.is_empty(),
            };
        drop(record);
        drop(session);
        if retryable {
            *movement = None;
        }
    }
    fn claim_move(&self, consumer: Consumer, number: u64) -> Result<Arc<MoveBatch>> {
        self.allowed()?;
        ensure!(
            consumer.uid == self.uid,
            "The paste consumer is not authorized"
        );
        let mut movement = lock(&self.movement);
        if let Some(batch) = movement.as_ref() {
            ensure!(
                batch.owner == consumer,
                "Another native file move is using this clipboard selection"
            );
            ensure!(
                !lock(&batch.record).committed,
                "This move was already completed"
            );
            return Ok(batch.clone());
        }
        let batch = Arc::new(MoveBatch {
            owner: consumer,
            consumer: self.native_consumer(consumer, number),
            session: Mutex::new(None),
            record: Mutex::new(MoveRecord::default()),
        });
        *movement = Some(batch.clone());
        Ok(batch)
    }
    fn handle_batch(&self, handle: &OpenHandle, number: u64) -> Result<Arc<MoveBatch>> {
        let mut slot = lock(&handle.batch);
        if let Some(batch) = slot.as_ref() {
            return Ok(batch.clone());
        }
        let existing = lock(&self.movement).clone();
        let batch = if let Some(batch) = existing {
            ensure!(
                batch.owner == handle.consumer,
                "Another native file move is using this clipboard selection"
            );
            batch
        } else {
            self.claim_move(handle.consumer, number)?
        };
        *slot = Some(batch.clone());
        Ok(batch)
    }
    fn native_consumer(&self, consumer: Consumer, serial: u64) -> NativeConsumer {
        let value = format!(
            "{}:{}:{}:{}",
            self.offer.id, consumer.tgid, consumer.started, serial
        );
        NativeConsumer {
            process_id: consumer.tgid,
            started: consumer.started,
            operation_id: hash(value.as_bytes())[..32].to_owned(),
        }
    }
    fn session(&self, handle: &OpenHandle, number: u64) -> Result<Arc<dyn PasteSession>> {
        if self.offer.operation == Operation::Move {
            let batch = self.handle_batch(handle, number)?;
            self.move_session(&batch, &handle.authority)
        } else {
            let mut session = lock(&handle.session);
            if let Some(session) = session.as_ref() {
                return Ok(session.clone());
            }
            let value = handle.authority.begin(PasteIntent {
                operation: Operation::Copy,
                completion: CompletionSupport::Unavailable,
                destination: None,
                consumer: Some(self.native_consumer(handle.consumer, number)),
            })?;
            if self.closed.load(Ordering::Acquire) {
                let _ = value.native_complete(NativeResult::Cancelled);
                bail!("The file paste was stopped");
            }
            *session = Some(value.clone());
            Ok(value)
        }
    }
    fn move_session(
        &self,
        batch: &MoveBatch,
        authority: &Arc<dyn DemandProvider>,
    ) -> Result<Arc<dyn PasteSession>> {
        // Only I/O workers use this lock; metadata callbacks stay responsive.
        let mut session = lock(&batch.session);
        if let Some(session) = session.as_ref() {
            ensure!(
                !session.is_terminal() || lock(&batch.record).committed,
                "The previous move ended; retry after its native file handles close"
            );
            return Ok(session.clone());
        }
        let value = authority.begin(PasteIntent {
            operation: Operation::Move,
            completion: CompletionSupport::Reported,
            destination: None,
            consumer: Some(batch.consumer.clone()),
        })?;
        if self.closed.load(Ordering::Acquire) {
            let _ = value.native_complete(NativeResult::Cancelled);
            bail!("The file paste was stopped");
        }
        *session = Some(value.clone());
        Ok(value)
    }
    fn fail_handle(&self, handle: &OpenHandle, error: &anyhow::Error) {
        let session = if self.offer.operation == Operation::Move {
            lock(&handle.batch)
                .as_ref()
                .and_then(|batch| lock(&batch.session).clone())
        } else {
            lock(&handle.session).clone()
        };
        if let Some(session) = session {
            let _ = session.native_complete(NativeResult::Failed(format!("{error:#}")));
        }
    }
    fn read_data(
        &self,
        handle: &OpenHandle,
        number: u64,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>> {
        ensure!(
            !self.closed.load(Ordering::Acquire),
            "The file paste was stopped"
        );
        let node = self
            .tree
            .node_at(handle.ino)
            .context("The copied file is unavailable")?;
        let index = node.index.context("A folder cannot be read as a file")?;
        ensure!(
            node.attributes.kind == FileType::RegularFile,
            "A folder cannot be read as a file"
        );
        let size = node.attributes.size;
        if length == 0 || offset >= size {
            return Ok(Vec::new());
        }
        let session = self.session(handle, number)?;
        session.wait_unpaused()?;
        ensure!(
            !self.closed.load(Ordering::Acquire),
            "The file paste was stopped"
        );
        let mut file = lock(&handle.file);
        if file.is_none() {
            let verified = session.open_verified(index)?;
            let metadata = verified.metadata()?;
            ensure!(
                metadata.is_file() && metadata.len() == size,
                "The copied file changed; copy it again"
            );
            *file = Some(verified);
        }
        session.wait_unpaused()?;
        let count = length
            .min(MAX_READ)
            .min(usize::try_from(size - offset).unwrap_or(usize::MAX));
        let mut bytes = vec![0; count];
        let mut filled = 0;
        while filled < bytes.len() {
            match file
                .as_ref()
                .unwrap()
                .read_at(&mut bytes[filled..], offset + filled as u64)
            {
                Ok(0) => bail!("The verified file ended before its declared size"),
                Ok(count) => filled += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error.into()),
            }
        }
        drop(file);
        session.report_read(index, offset, bytes.len())?;
        if self.offer.operation == Operation::Move {
            let batch = lock(&handle.batch)
                .clone()
                .context("The move session ended")?;
            let mut record = lock(&batch.record);
            if record.readers.get(&index).is_some_and(|old| *old != number) {
                record.ambiguous = true;
            }
            record.readers.insert(index, number);
            let coverage = record.coverage.entry(index).or_default();
            coverage.record(offset, bytes.len(), size)?;
            ensure!(
                coverage.ranges.len() <= 4096,
                "The move read pattern is too fragmented"
            );
        }
        ensure!(
            !self.closed.load(Ordering::Acquire),
            "The file paste was stopped"
        );
        self.touch();
        Ok(bytes)
    }
    fn prepare_remove(&self, consumer: Consumer, index: usize) -> Result<Arc<MoveBatch>> {
        self.allowed()?;
        ensure!(
            self.offer.operation == Operation::Move && consumer.uid == self.uid,
            "The copied files are read-only"
        );
        self.reset_cancelled_move();
        let ino = INodeNo(index as u64 + 2);
        let node = self
            .tree
            .node_at(ino)
            .context("The move item is unavailable")?;
        if node.attributes.kind == FileType::RegularFile && node.attributes.size != 0 {
            let batch = lock(&self.movement)
                .clone()
                .context("The native consumer did not read this file")?;
            ensure!(
                batch.owner == consumer,
                "Another native file move is using this clipboard selection"
            );
            ensure!(
                lock(&batch.record)
                    .coverage
                    .get(&index)
                    .is_some_and(|coverage| coverage.complete(node.attributes.size)),
                "The native consumer did not read the complete file"
            );
        }
        self.claim_move(consumer, self.next_serial()?)
    }
    fn remove_data(&self, consumer: Consumer, batch: &Arc<MoveBatch>, index: usize) -> Result<()> {
        self.allowed()?;
        ensure!(
            self.offer.operation == Operation::Move && consumer == batch.owner,
            "The copied files are read-only"
        );
        ensure!(
            lock(&self.movement)
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, batch)),
            "The native move receipt belongs to a previous paste"
        );
        let node = self
            .tree
            .node_at(INodeNo(index as u64 + 2))
            .context("The move item is unavailable")?;
        // Empty files/folders have no content read. Their native post-copy
        // remove is the zero-byte demand boundary and still needs verification.
        let session = self.move_session(batch, &self.authority)?;
        session.wait_verified()?;
        session.wait_unpaused()?;
        ensure!(
            !self.closed.load(Ordering::Acquire),
            "The file paste was stopped"
        );
        let mut record = lock(&batch.record);
        ensure!(
            !record.ambiguous && !record.committed && record.committing.is_none(),
            "The native move completion is ambiguous; originals are retained"
        );
        if record.receipts.contains_key(&index) {
            record.ambiguous = true;
            bail!("Concurrent native move receipts are ambiguous; originals are retained");
        }
        match node.attributes.kind {
            FileType::RegularFile => ensure!(
                record
                    .coverage
                    .get(&index)
                    .is_some_and(|coverage| coverage.complete(node.attributes.size))
                    || node.attributes.size == 0,
                "The native consumer did not read the complete file"
            ),
            FileType::Directory => ensure!(
                node.children.values().all(|child| self
                    .tree
                    .node_at(*child)
                    .and_then(|node| node.index)
                    .is_some_and(|child| record.receipts.contains_key(&child))),
                "The native folder move is not complete"
            ),
            _ => bail!("Unsupported copied file kind"),
        }
        record.receipts.insert(index, true);
        if record.receipts.len() == self.offer.description.entries.len() {
            record.committing = Some(index);
            let receipt = NativeResult::MoveCommitted {
                consumer: batch.consumer.clone(),
                entries: record.receipts.keys().copied().collect(),
            };
            drop(record);
            // Only the service validates/quarantines actual originals. The VFS
            // never deletes a source path, including on cancel or unmount.
            let result = session.native_complete(receipt);
            let mut record = lock(&batch.record);
            record.committing = None;
            if let Err(error) = result {
                record.receipts.remove(&index);
                return Err(error);
            }
            record.committed = true;
        }
        self.touch();
        Ok(())
    }
    fn removed(&self, ino: INodeNo) -> bool {
        let Some(index) = self.tree.node_at(ino).and_then(|node| node.index) else {
            return false;
        };
        lock(&self.movement).as_ref().is_some_and(|batch| {
            let record = lock(&batch.record);
            // Receipts are one transaction. Hiding a provisional child would
            // let another same-process folder paste enumerate only its remainder
            // and incorrectly complete a shared receipt set across destinations.
            record.committed && record.receipts.contains_key(&index)
        })
    }
    fn metadata_access(&self, request: &Request) -> bool {
        self.reset_cancelled_move();
        let owner = lock(&self.movement).as_ref().map(|batch| batch.owner);
        owner
            .is_none_or(|owner| Consumer::resolve(request.pid(), request.uid()).ok() == Some(owner))
    }
    fn queue(&self) -> Result<&Pool> {
        self.pool
            .get_or_init(|| Pool::new().map_err(|error| format!("{error:#}")))
            .as_ref()
            .map_err(|error| anyhow::anyhow!(error.clone()))
    }
    fn stop(&self) {
        self.closed.store(true, Ordering::Release);
        let handles = lock(&self.opened).values().cloned().collect::<Vec<_>>();
        let batch = lock(&self.movement).clone();
        let mut sessions = handles
            .iter()
            .filter_map(|handle| handle.session.try_lock().ok().and_then(|slot| slot.clone()))
            .collect::<Vec<_>>();
        sessions.extend(
            batch
                .iter()
                .filter_map(|batch| batch.session.try_lock().ok().and_then(|slot| slot.clone())),
        );
        for session in sessions {
            let _ = session.native_complete(NativeResult::Cancelled);
        }
    }
}

struct NativeFilesystem(Arc<State>);

impl NativeFilesystem {
    fn metadata_open(
        &self,
        request: &Request,
        ino: INodeNo,
        flags: OpenFlags,
        directory: bool,
        reply: ReplyOpen,
    ) {
        if !self.0.metadata_access(request) {
            reply.error(Errno::EBUSY);
            return;
        }
        let Some(attributes) = self.0.tree.attributes(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };
        if self.0.removed(ino) {
            reply.error(Errno::ENOENT);
            return;
        }
        if (attributes.kind == FileType::Directory) != directory {
            reply.error(if directory {
                Errno::ENOTDIR
            } else {
                Errno::EISDIR
            });
            return;
        }
        if flags.acc_mode() != OpenAccMode::O_RDONLY
            || flags.0 & (libc::O_TRUNC | libc::O_APPEND | libc::O_CREAT) != 0
        {
            reply.error(Errno::EROFS);
            return;
        }
        match Consumer::resolve(request.pid(), request.uid())
            .and_then(|consumer| self.0.open(ino, consumer))
        {
            Ok(number) => reply.opened(
                FileHandle(number),
                if directory {
                    FopenFlags::empty()
                } else {
                    FopenFlags::FOPEN_DIRECT_IO
                },
            ),
            Err(_) => reply.error(Errno::EACCES),
        }
    }

    fn remove(
        &self,
        request: &Request,
        parent: INodeNo,
        name: &OsStr,
        directory: bool,
        reply: ReplyEmpty,
    ) {
        if self.0.offer.operation != Operation::Move {
            reply.error(Errno::EROFS);
            return;
        }
        let Some(ino) = self
            .0
            .tree
            .node_at(parent)
            .and_then(|node| name.to_str().and_then(|name| node.children.get(name)))
            .copied()
        else {
            reply.error(Errno::ENOENT);
            return;
        };
        let node = self.0.tree.node_at(ino).unwrap();
        if (node.attributes.kind == FileType::Directory) != directory {
            reply.error(if directory {
                Errno::ENOTDIR
            } else {
                Errno::EISDIR
            });
            return;
        }
        let Some(index) = node.index else {
            reply.error(Errno::EACCES);
            return;
        };
        let Ok(consumer) = Consumer::resolve(request.pid(), request.uid()) else {
            reply.error(Errno::EACCES);
            return;
        };
        if consumer.uid != self.0.uid {
            reply.error(Errno::EACCES);
            return;
        }
        let Ok(batch) = self.0.prepare_remove(consumer, index) else {
            reply.error(Errno::EBUSY);
            return;
        };
        let Ok(pool) = self.0.queue() else {
            reply.error(Errno::EIO);
            return;
        };
        self.0.pending.fetch_add(1, Ordering::AcqRel);
        let work = Work::Remove {
            state: self.0.clone(),
            consumer,
            batch,
            index,
            reply,
        };
        if let Err(error) = pool.sender.try_send(work) {
            self.0.pending.fetch_sub(1, Ordering::AcqRel);
            let Work::Remove { reply, .. } = error.into_inner() else {
                unreachable!();
            };
            reply.error(Errno::EAGAIN);
        }
    }
}

impl Filesystem for NativeFilesystem {
    fn destroy(&mut self) {
        self.0.stop();
    }
    fn lookup(&self, request: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        if self.0.allowed().is_err() {
            reply.error(Errno::ESTALE);
            return;
        }
        if !self.0.metadata_access(request) {
            reply.error(Errno::EBUSY);
            return;
        }
        let Some(ino) = self.0.tree.lookup(parent, name) else {
            reply.error(Errno::ENOENT);
            return;
        };
        if self.0.removed(ino) {
            reply.error(Errno::ENOENT);
            return;
        }
        self.0.touch();
        reply.entry(
            &ATTR_TTL,
            &self.0.tree.attributes(ino).unwrap(),
            Generation(0),
        );
    }
    fn getattr(&self, request: &Request, ino: INodeNo, fh: Option<FileHandle>, reply: ReplyAttr) {
        if self.0.allowed().is_err() {
            reply.error(Errno::ESTALE);
            return;
        }
        if fh.is_none() && !self.0.metadata_access(request) {
            reply.error(Errno::EBUSY);
            return;
        }
        if self.0.removed(ino) && fh.is_none() {
            reply.error(Errno::ENOENT);
            return;
        }
        match self.0.tree.attributes(ino) {
            Some(attributes) => {
                self.0.touch();
                reply.attr(&ATTR_TTL, &attributes);
            }
            None => reply.error(Errno::ENOENT),
        }
    }
    fn open(&self, request: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        self.metadata_open(request, ino, flags, false, reply);
    }
    fn opendir(&self, request: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        self.metadata_open(request, ino, flags, true, reply);
    }
    fn read(
        &self,
        request: &Request,
        ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        if self.0.closed.load(Ordering::Acquire) {
            reply.error(Errno::EIO);
            return;
        }
        let Some(handle) = lock(&self.0.opened).get(&fh.0).cloned() else {
            reply.error(Errno::EBADF);
            return;
        };
        if handle.ino != ino || request.uid() != self.0.uid {
            reply.error(Errno::EACCES);
            return;
        }
        let Some(node) = self.0.tree.node_at(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };
        if node.attributes.kind != FileType::RegularFile {
            reply.error(Errno::EISDIR);
            return;
        }
        // A zero-byte read and EOF are metadata boundaries, not transfer intent.
        if size == 0 || offset >= node.attributes.size {
            reply.data(&[]);
            return;
        }
        if self.0.offer.operation == Operation::Move {
            if Consumer::resolve(request.pid(), request.uid()).ok() != Some(handle.consumer) {
                reply.error(Errno::EACCES);
                return;
            }
            if self.0.handle_batch(&handle, fh.0).is_err() {
                reply.error(Errno::EBUSY);
                return;
            }
        }
        let Ok(pool) = self.0.queue() else {
            reply.error(Errno::EIO);
            return;
        };
        self.0.pending.fetch_add(1, Ordering::AcqRel);
        let work = Work::Read {
            state: self.0.clone(),
            handle,
            number: fh.0,
            offset,
            length: size as usize,
            reply,
        };
        if let Err(error) = pool.sender.try_send(work) {
            self.0.pending.fetch_sub(1, Ordering::AcqRel);
            let Work::Read { reply, .. } = error.into_inner() else {
                unreachable!();
            };
            reply.error(Errno::EAGAIN);
        }
    }
    fn readdir(
        &self,
        request: &Request,
        ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        if self.0.closed.load(Ordering::Acquire) {
            reply.error(Errno::EIO);
            return;
        }
        if !self.0.metadata_access(request) {
            reply.error(Errno::EBUSY);
            return;
        }
        if !lock(&self.0.opened)
            .get(&fh.0)
            .is_some_and(|handle| handle.ino == ino)
        {
            reply.error(Errno::EBADF);
            return;
        }
        let Some(node) = self.0.tree.node_at(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };
        if node.attributes.kind != FileType::Directory {
            reply.error(Errno::ENOTDIR);
            return;
        }
        let dots = [
            (ino, FileType::Directory, "."),
            (node.parent, FileType::Directory, ".."),
        ];
        let entries = dots
            .into_iter()
            .chain(node.children.iter().map(|(name, child)| {
                (
                    *child,
                    self.0.tree.node_at(*child).unwrap().attributes.kind,
                    name.as_str(),
                )
            }));
        for (index, (child, kind, name)) in entries
            .enumerate()
            .skip(usize::try_from(offset).unwrap_or(usize::MAX))
        {
            // Native directory cookies remain stable when a preceding item is
            // removed during a folder move. Avoid rebuilding a full folder list.
            if self.0.removed(child) {
                continue;
            }
            if reply.add(child, index as u64 + 1, kind, name) {
                break;
            }
        }
        self.0.touch();
        reply.ok();
    }
    fn release(
        &self,
        _request: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        self.0.close_handle(fh.0);
        reply.ok();
    }
    fn releasedir(
        &self,
        _request: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        reply: ReplyEmpty,
    ) {
        self.0.close_handle(fh.0);
        reply.ok();
    }
    fn unlink(&self, request: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        self.remove(request, parent, name, false, reply);
    }
    fn rmdir(&self, request: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        self.remove(request, parent, name, true, reply);
    }
    fn getxattr(
        &self,
        _request: &Request,
        _ino: INodeNo,
        _name: &OsStr,
        _size: u32,
        reply: ReplyXattr,
    ) {
        reply.error(Errno::ENODATA);
    }
    fn listxattr(&self, _request: &Request, _ino: INodeNo, size: u32, reply: ReplyXattr) {
        if size == 0 {
            reply.size(0);
        } else {
            reply.data(&[]);
        }
    }
}

struct Mounted {
    state: Arc<State>,
    session: Mutex<Option<BackgroundSession>>,
    path: PathBuf,
}

impl Mounted {
    fn create(offer: RemoteOffer) -> Result<Arc<Self>> {
        ensure!(
            Path::new("/dev/fuse").exists(),
            "File paste needs FUSE: install the Ubuntu fuse3 package and make /dev/fuse available"
        );
        let state = State::new(offer)?;
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .context("File paste needs a private XDG_RUNTIME_DIR")?;
        verify_private_directory(&base, state.uid)?;
        let parent = base.join("desk-switch-file-paste");
        match DirBuilder::new().mode(0o700).create(&parent) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        verify_private_directory(&parent, state.uid)?;
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = parent.join(format!("{}-{}-{nonce}", state.offer.id, std::process::id()));
        DirBuilder::new().mode(0o700).create(&path)?;
        let mut configuration = Config::default();
        configuration.mount_options = vec![
            MountOption::FSName("Desk Switch copied files".into()),
            MountOption::Subtype("desk-switch-file-paste".into()),
            MountOption::DefaultPermissions,
            MountOption::NoSuid,
            MountOption::NoDev,
            MountOption::NoExec,
            MountOption::NoAtime,
            if state.offer.operation == Operation::Copy {
                MountOption::RO
            } else {
                MountOption::RW
            },
        ];
        configuration.acl = SessionACL::Owner;
        configuration.n_threads = Some(1);
        let session = match fuser::spawn_mount(
            NativeFilesystem(state.clone()),
            &path,
            &configuration,
        ) {
            Ok(session) => session,
            Err(error) => {
                let _ = fs::remove_dir(&path);
                return Err(error).context("Could not enable native file paste; install fuse3 and make /dev/fuse available");
            }
        };
        Ok(Arc::new(Self {
            state,
            session: Mutex::new(Some(session)),
            path,
        }))
    }
    fn paths(&self) -> Vec<PathBuf> {
        self.state
            .offer
            .description
            .roots
            .iter()
            .map(|name| self.path.join(name))
            .collect()
    }
}

impl Drop for Mounted {
    fn drop(&mut self) {
        self.state.stop();
        // Unmount only our newly-created private mount. Never recursively remove
        // a mountpoint: it contains virtual file capabilities, not disposable data.
        if let Some(session) = lock(&self.session).take() {
            let _ = session.umount_and_join();
        }
        let _ = fs::remove_dir(&self.path);
    }
}

fn verify_private_directory(path: &Path, uid: u32) -> Result<()> {
    ensure!(
        path.is_absolute(),
        "The private file paste directory must be absolute"
    );
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == uid
            && metadata.permissions().mode() & 0o077 == 0,
        "File paste needs a private directory owned by this user"
    );
    Ok(())
}

struct Binding {
    mount: Arc<Mounted>,
    ownership: NativeOwnership,
}

struct NativeOwnership {
    serial: AtomicU64,
    paths: Vec<PathBuf>,
    operation: Operation,
}
impl NativeOwnership {
    fn observe(&self, selection: Option<&super::Selection>) -> bool {
        let Some(selection) = selection else {
            return false;
        };
        if selection.paths != self.paths || selection.operation != self.operation {
            return false;
        }
        // A clipboard manager may re-own the same URI metadata. Its new native
        // revision does not retire the provider or start a demand/lease/dialog.
        self.serial.store(selection.sequence, Ordering::Release);
        true
    }
}
impl PublishedOffer for Binding {
    fn is_current(&self) -> Result<bool> {
        Ok(self.ownership.observe(super::read()?.as_ref()))
    }
    fn sequence(&self) -> Option<u64> {
        Some(self.ownership.serial.load(Ordering::Acquire))
    }
    fn local_paths(&self) -> Vec<PathBuf> {
        self.ownership.paths.clone()
    }
}
impl Drop for Binding {
    fn drop(&mut self) {
        *lock(&self.mount.state.retired) = Some(Instant::now());
        self.mount.state.touch();
    }
}

pub(super) fn publish_offer_if_current(
    offer: RemoteOffer,
    expected: u64,
) -> Result<Option<Box<dyn PublishedOffer>>> {
    // Clean only expired inactive capabilities. Started pastes retain their
    // mount while the clipboard moves on to an unrelated user copy.
    MOUNTS.with(|mounts| {
        let mut mounts = mounts.borrow_mut();
        mounts.retain(|mount: &Arc<Mounted>| mount.state.retained());
        ensure!(
            mounts.len() < MAX_RETAINED_OFFERS,
            "Too many active copied file selections; finish or cancel a file paste first"
        );
        Ok::<(), anyhow::Error>(())
    })?;
    let operation = offer.operation;
    let mount = Mounted::create(offer)?;
    let paths = mount.paths();
    if !super::write_operation_if_current(&paths, expected, operation)? {
        return Ok(None);
    }
    let serial = super::sequence().context("The native file clipboard revision is unavailable")?;
    MOUNTS.with(|mounts| mounts.borrow_mut().push(mount.clone()));
    Ok(Some(Box::new(Binding {
        mount,
        ownership: NativeOwnership {
            serial: AtomicU64::new(serial),
            paths,
            operation,
        },
    })))
}

/// Clipboard managers can take ownership while retaining our virtual URIs.
/// Recognize active and retained mounts without stat/open or source export.
pub(super) fn is_provider_path(path: &Path) -> bool {
    MOUNTS.with(|mounts| {
        mounts
            .borrow()
            .iter()
            .any(|mount| path.starts_with(&mount.path))
    })
}

#[cfg(test)]
#[path = "linux_vfs_tests.rs"]
mod tests;
