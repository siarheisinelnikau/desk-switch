//! On-demand filesystem files retain the existing atomic CF_HDROP publication.
//! Only a data read or an explicit out-of-root Move starts the remote provider.
#[cfg(test)]
#[path = "cloud_tests.rs"]
mod native_tests;

use super::{Clipboard, cloud_api::Api, publish_paths, read_locked};
use crate::group::{
    clipboard_files::provider::{
        CompletionSupport, DemandProvider, NativeConsumer, NativeResult, PasteIntent, PasteSession,
        PublishedOffer, RemoteOffer,
    },
    file_transfer::protocol::{self, DescriptorKind, Operation},
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    ffi::{OsString, c_void},
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    mem::{offset_of, size_of},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::MetadataExt,
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Component, Path, PathBuf},
    ptr,
    sync::{
        Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak,
        atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{HANDLE, NTSTATUS},
        Storage::CloudFilters::*,
    },
    core::{GUID, PCWSTR},
};

const ROOT_LIMIT: usize = 32;
const REQUEST_LIMIT: usize = 32;
const RETIRED_TTL: Duration = Duration::from_secs(300);
const NATIVE_DEADLINE: Duration = Duration::from_secs(50);
const HEARTBEAT: Duration = Duration::from_secs(5);
const BLOCK: usize = 64 * 1024;
const REPARSE: u32 = 0x400;
const DIRECTORY: u32 = 0x10;
const NOT_INDEXED: u32 = 0x2000;
const STATUS_OK: NTSTATUS = NTSTATUS(0);
const STATUS_FAILED: NTSTATUS = NTSTATUS(0xc000cf00u32 as i32);
const STATUS_CANCELLED: NTSTATUS = NTSTATUS(0xc0000120u32 as i32);
const MARKER: &str = ".desk-switch-root";
static LIVE_ROOTS: AtomicUsize = AtomicUsize::new(0);
static ACTIVE_WORKERS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Identity {
    volume: u32,
    file: i64,
    size: u64,
    written: u64,
}
#[repr(C)]
#[derive(Default)]
struct FileTime {
    low: u32,
    high: u32,
}
impl FileTime {
    fn value(&self) -> u64 {
        u64::from(self.low) | (u64::from(self.high) << 32)
    }
}
#[repr(C)]
#[derive(Default)]
struct HandleInfo {
    attributes: u32,
    created: FileTime,
    accessed: FileTime,
    written: FileTime,
    volume: u32,
    size_high: u32,
    size_low: u32,
    links: u32,
    index_high: u32,
    index_low: u32,
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        disposition: u32,
        flags: u32,
        template: *mut c_void,
    ) -> *mut c_void;
    fn GetFileInformationByHandle(handle: *mut c_void, info: *mut HandleInfo) -> i32;
    fn OpenProcess(access: u32, inherit: i32, id: u32) -> *mut c_void;
    fn GetProcessTimes(
        handle: *mut c_void,
        created: *mut FileTime,
        exit: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
}
fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
fn attributes(path: &Path) -> Result<(File, Identity)> {
    let name = wide(path);
    // Metadata-only access plus OPEN_REPARSE_POINT never hydrates a placeholder.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            0x80,
            7,
            ptr::null(),
            3,
            0x0220_0000,
            ptr::null_mut(),
        )
    };
    ensure!(
        handle as isize != -1,
        "Could not inspect the native clipboard cache"
    );
    let file = unsafe { File::from_raw_handle(handle) };
    Ok((file.try_clone()?, identity(&file)?))
}
fn identity(file: &File) -> Result<Identity> {
    let mut info = HandleInfo::default();
    ensure!(
        unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } != 0,
        "Could not identify a native clipboard file"
    );
    Ok(Identity {
        volume: info.volume,
        file: (u64::from(info.index_low) | (u64::from(info.index_high) << 32)) as i64,
        size: u64::from(info.size_low) | (u64::from(info.size_high) << 32),
        written: info.written.value(),
    })
}
fn native_consumer(id: u32, operation: &str) -> Option<NativeConsumer> {
    if id == 0 || id == std::process::id() {
        return None;
    }
    let handle = unsafe { OpenProcess(0x1000, 0, id) };
    if handle.is_null() {
        return None;
    }
    let mut created = FileTime::default();
    let mut exit = FileTime::default();
    let mut kernel = FileTime::default();
    let mut user = FileTime::default();
    let valid = unsafe { GetProcessTimes(handle, &mut created, &mut exit, &mut kernel, &mut user) };
    unsafe { CloseHandle(handle) };
    (valid != 0 && created.value() != 0).then(|| NativeConsumer {
        process_id: id,
        started: created.value(),
        operation_id: operation.to_owned(),
    })
}
fn safe_cache_base(base: &Path) -> Result<PathBuf> {
    ensure!(
        base.is_absolute(),
        "The file clipboard cache must be local and absolute"
    );
    let mut path = PathBuf::new();
    for component in base.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => path.push(component),
            Component::Normal(name) => {
                path.push(name);
                match fs::create_dir(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error.into()),
                }
                let metadata = fs::symlink_metadata(&path)?;
                ensure!(
                    metadata.is_dir() && metadata.file_attributes() & REPARSE == 0,
                    "The file clipboard cache cannot use redirected directories"
                );
            }
            _ => bail!("The file clipboard cache path is invalid"),
        }
    }
    Ok(path.canonicalize()?)
}
fn nonce() -> Result<Vec<u8>> {
    let keys = snow::Builder::new(crate::config::NOISE.parse()?).generate_keypair()?;
    Ok(keys.private[..16].to_vec())
}
fn joined(root: &Path, components: &[String]) -> PathBuf {
    components
        .iter()
        .fold(root.to_path_buf(), |path, name| path.join(name))
}
fn normalized(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .to_ascii_lowercase()
}
fn inside(path: &Path, root: &Path) -> bool {
    let path = normalized(path);
    let root = normalized(root);
    path == root
        || path
            .strip_prefix(&root)
            .is_some_and(|tail| tail.starts_with('\\'))
}
fn clipboard_path(path: &Path) -> PathBuf {
    // canonicalize() uses the extended Win32 spelling. CF_HDROP deliberately
    // accepts only safe local-drive paths, so publish the equivalent DOS path
    // without relaxing its device/UNC namespace rejection.
    let units = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if units.starts_with(&[92, 92, 63, 92])
        && units.len() >= 7
        && (units[4] as u8).is_ascii_alphabetic()
        && units[4] <= 127
        && units[5] == 58
        && units[6] == 92
    {
        return PathBuf::from(OsString::from_wide(&units[4..]));
    }
    path.to_path_buf()
}
fn source_identity(token: &[u8], index: usize) -> Vec<u8> {
    let mut bytes = b"DSCF".to_vec();
    bytes.extend_from_slice(token);
    bytes.extend_from_slice(&(index as u32).to_le_bytes());
    bytes
}
fn decode_identity(bytes: &[u8], token: &[u8], count: usize) -> Option<usize> {
    if bytes.len() != 24 || &bytes[..4] != b"DSCF" || &bytes[4..20] != token {
        return None;
    }
    let index = u32::from_le_bytes(bytes[20..24].try_into().ok()?) as usize;
    (index < count).then_some(index)
}

#[derive(Default)]
struct Registry {
    roots: Vec<Arc<Root>>,
    bases: BTreeSet<PathBuf>,
}
thread_local! {
    static ROOTS: RefCell<Registry> = RefCell::new(Registry::default());
}
pub(super) fn owns_paths(paths: &[PathBuf]) -> bool {
    ROOTS.with(|registry| {
        let registry = registry.borrow();
        !paths.is_empty()
            && paths
                .iter()
                .any(|path| registry.bases.iter().any(|base| inside(path, base)))
    })
}
pub(super) fn owns_path(path: &Path) -> bool {
    ROOTS.with(|registry| {
        registry
            .borrow()
            .bases
            .iter()
            .any(|base| inside(path, base))
    })
}
pub(super) fn publish(
    offer: RemoteOffer,
    expected: u64,
) -> Result<Option<Box<dyn PublishedOffer>>> {
    let base = dirs::cache_dir()
        .context("The local cache directory is unavailable")?
        .join("Desk Switch")
        .join("File clipboard");
    publish_at(offer, expected, &base)
}
fn publish_at(
    offer: RemoteOffer,
    expected: u64,
    base: &Path,
) -> Result<Option<Box<dyn PublishedOffer>>> {
    offer.description.validate()?;
    ROOTS.with(|registry| {
        registry.borrow_mut().roots.retain(|root| !root.reapable());
    });
    let root = Root::create(offer, base)?;
    let paths = root.paths();
    let Some(receipt) = publish_paths(&paths, Some(expected), root.state.offer.operation)? else {
        return Ok(None);
    };
    ROOTS.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.bases.insert(root.base.clone());
        registry.roots.push(root.clone());
    });
    Ok(Some(Box::new(Binding {
        root,
        sequence: AtomicU64::new(receipt.sequence),
    })))
}
struct Binding {
    root: Arc<Root>,
    sequence: AtomicU64,
}
impl PublishedOffer for Binding {
    fn is_current(&self) -> Result<bool> {
        let clipboard = Clipboard::open(ptr::null_mut())?;
        let Some(selection) = read_locked(&clipboard, false)? else {
            return Ok(false);
        };
        let matches = same_selection(
            &self.root.paths(),
            self.root.state.offer.operation,
            &selection.paths,
            selection.operation,
        );
        if matches {
            self.sequence.store(selection.sequence, Ordering::Release);
        }
        Ok(matches)
    }
    fn sequence(&self) -> Option<u64> {
        Some(self.sequence.load(Ordering::Acquire))
    }
    fn local_paths(&self) -> Vec<PathBuf> {
        self.root.paths()
    }
}
fn same_selection(
    expected: &[PathBuf],
    operation: Operation,
    actual: &[PathBuf],
    actual_operation: Operation,
) -> bool {
    operation == actual_operation
        && expected.len() == actual.len()
        && expected
            .iter()
            .zip(actual)
            .all(|(expected, actual)| normalized(expected) == normalized(actual))
}
impl Drop for Binding {
    fn drop(&mut self) {
        *self.root.retired.lock().unwrap() = Some(Instant::now());
        self.root.state.accepting.store(false, Ordering::Release);
    }
}
struct LivePermit;
impl LivePermit {
    fn acquire() -> Result<Self> {
        LIVE_ROOTS
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < ROOT_LIMIT).then_some(count + 1)
            })
            .map_err(|_| anyhow!("Too many file clipboard operations are still in use"))?;
        Ok(Self)
    }
}
impl Drop for LivePermit {
    fn drop(&mut self) {
        LIVE_ROOTS.fetch_sub(1, Ordering::AcqRel);
    }
}
struct Root {
    base: PathBuf,
    path: PathBuf,
    state: Arc<State>,
    retired: Mutex<Option<Instant>>,
    cleanup: Mutex<Option<Cleanup>>,
}
struct Cleanup {
    api: Arc<Api>,
    path: PathBuf,
    base: PathBuf,
    root_identity: Identity,
    marker: Vec<u8>,
    // The native callback context points at the Arc inside this allocation.
    // Boxing keeps that address stable when Cleanup moves to the cleanup thread.
    #[allow(clippy::redundant_allocation)]
    context: Box<Arc<State>>,
    connection: Option<CF_CONNECTION_KEY>,
    registered: bool,
    _permit: LivePermit,
}
fn cleanup_queue() -> &'static mpsc::Sender<Cleanup> {
    static QUEUE: OnceLock<mpsc::Sender<Cleanup>> = OnceLock::new();
    QUEUE.get_or_init(|| {
        let (send, receive) = mpsc::channel::<Cleanup>();
        thread::Builder::new()
            .name("Desk Switch native clipboard cleanup".into())
            .spawn(move || {
                while let Ok(cleanup) = receive.recv() {
                    cleanup.run();
                }
            })
            .expect("Could not start native clipboard cleanup");
        send
    })
}
impl Cleanup {
    fn run(mut self) {
        self.context.tearing_down.store(true, Ordering::Release);
        if let Some(connection) = self.connection.take() {
            // The context remains allocated until all native callbacks finish.
            if self.api.disconnect_sync_root(connection).is_err() {
                // A live callback context must never become a dangling pointer.
                std::mem::forget(self);
                return;
            }
        }
        let same_root = attributes(&self.path).is_ok_and(|(_, id)| {
            id.volume == self.root_identity.volume && id.file == self.root_identity.file
        }) && self.path.parent() == Some(self.base.as_path())
            && fs::read(self.path.join(MARKER)).ok().as_deref() == Some(self.marker.as_slice());
        if !same_root {
            return;
        }
        if self.registered {
            let name = wide(&self.path);
            if unsafe { self.api.unregister_sync_root(PCWSTR(name.as_ptr())) }.is_err() {
                return;
            }
        }
        // Never recursively remove a computed directory. Remove only entries
        // whose immutable native identity and last verified generation match.
        let identities = self.context.identities.lock().unwrap().clone();
        let mut entries = (0..self.context.offer.description.entries.len()).collect::<Vec<_>>();
        entries.sort_by_key(|index| {
            std::cmp::Reverse(self.context.offer.description.entries[*index].path.len())
        });
        for index in entries {
            let entry = &self.context.offer.description.entries[index];
            let path = joined(&self.path, &entry.path);
            let Some(expected) = identities.get(&index) else {
                continue;
            };
            let Ok((_, actual)) = attributes(&path) else {
                continue;
            };
            if actual.volume != expected.volume || actual.file != expected.file {
                continue;
            }
            match entry.kind {
                DescriptorKind::Directory => {
                    let _ = fs::remove_dir(&path);
                }
                DescriptorKind::File { .. } => {
                    // The sync root is a private cache. The immutable identity
                    // check above is sufficient; requiring the last verified
                    // generation leaves partially hydrated files behind.
                    let _ = fs::remove_file(&path);
                }
            }
        }
        let _ = fs::remove_file(self.path.join(MARKER));
        let _ = fs::remove_dir(&self.path);
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        self.state.tearing_down.store(true, Ordering::Release);
        if let Some(cleanup) = self.cleanup.get_mut().unwrap().take() {
            // In particular this must not synchronously disconnect from inside
            // a Cloud Files callback, which would wait for itself to return.
            let _ = cleanup_queue().send(cleanup);
        }
    }
}
impl Root {
    fn create(offer: RemoteOffer, base: &Path) -> Result<Arc<Self>> {
        let permit = LivePermit::acquire()?;
        let api =
            Api::new().context("On-demand file paste requires the Windows Cloud Files platform")?;
        let base = safe_cache_base(base)?;
        let token = nonce()?;
        let path = base.join(format!("offer-{}", crate::config::hex(&token)));
        fs::create_dir(&path)?;
        let (_, root_identity) = attributes(&path)?;
        let marker = source_identity(&token, 0);
        fs::write(path.join(MARKER), &marker)?;
        let file_locks = (0..offer.description.entries.len())
            .map(|_| Mutex::new(()))
            .collect();
        let pinned = (0..offer.description.entries.len())
            .map(|_| Mutex::new(None))
            .collect();
        let state = Arc::new(State {
            offer,
            api: api.clone(),
            token,
            owner: Mutex::new(Weak::new()),
            connection: AtomicI64::new(0),
            root_identity,
            identities: Mutex::new(BTreeMap::new()),
            verified: Mutex::new(BTreeMap::new()),
            hydrated: Mutex::new(BTreeMap::new()),
            consumer_reads: Mutex::new(BTreeMap::new()),
            requests: Mutex::new(BTreeMap::new()),
            file_locks,
            pinned,
            opened: Mutex::new(BTreeMap::new()),
            sessions: Mutex::new(BTreeMap::new()),
            explicit: Mutex::new(BTreeMap::new()),
            moves: Mutex::new(BTreeMap::new()),
            moved: Mutex::new(BTreeSet::new()),
            destinations: Mutex::new(BTreeMap::new()),
            accepting: AtomicBool::new(true),
            tearing_down: AtomicBool::new(false),
            busy: AtomicUsize::new(0),
            initialized: AtomicBool::new(false),
            ambiguous: AtomicBool::new(false),
            move_blocked: AtomicBool::new(false),
            move_done: AtomicBool::new(false),
            monitor_running: AtomicBool::new(false),
            native_activity: Mutex::new(Instant::now()),
            #[cfg(test)]
            test_destination: Mutex::new(None),
            move_lock: Mutex::new(()),
        });
        let context = Box::new(state.clone());
        let root = Arc::new(Self {
            base: base.clone(),
            path: path.clone(),
            state: state.clone(),
            retired: Mutex::new(None),
            cleanup: Mutex::new(Some(Cleanup {
                api: api.clone(),
                path: path.clone(),
                base,
                root_identity,
                marker,
                context,
                connection: None,
                registered: false,
                _permit: permit,
            })),
        });
        *state.owner.lock().unwrap() = Arc::downgrade(&root);
        let name = wide(&path);
        let provider = "Desk Switch file clipboard\0"
            .encode_utf16()
            .collect::<Vec<_>>();
        let version = concat!(env!("CARGO_PKG_VERSION"), "\0")
            .encode_utf16()
            .collect::<Vec<_>>();
        let registration = CF_SYNC_REGISTRATION {
            StructSize: size_of::<CF_SYNC_REGISTRATION>() as u32,
            ProviderName: PCWSTR(provider.as_ptr()),
            ProviderVersion: PCWSTR(version.as_ptr()),
            SyncRootIdentity: state.token.as_ptr().cast(),
            SyncRootIdentityLength: state.token.len() as u32,
            ProviderId: GUID::from_u128(0xdba8f38b_7bbd_4a22_91e6_64d679d0488c),
            ..Default::default()
        };
        let policies = CF_SYNC_POLICIES {
            StructSize: size_of::<CF_SYNC_POLICIES>() as u32,
            Hydration: CF_HYDRATION_POLICY {
                Primary: CF_HYDRATION_POLICY_PROGRESSIVE,
                Modifier: CF_HYDRATION_POLICY_MODIFIER_VALIDATION_REQUIRED,
            },
            Population: CF_POPULATION_POLICY {
                Primary: CF_POPULATION_POLICY_ALWAYS_FULL,
                Modifier: CF_POPULATION_POLICY_MODIFIER_NONE,
            },
            InSync: CF_INSYNC_POLICY_NONE,
            HardLink: CF_HARDLINK_POLICY_NONE,
            ..Default::default()
        };
        unsafe {
            api.register_sync_root(
                PCWSTR(name.as_ptr()),
                &registration,
                &policies,
                CF_REGISTER_FLAG_MARK_IN_SYNC_ON_ROOT,
            )
        }?;
        root.cleanup.lock().unwrap().as_mut().unwrap().registered = true;
        let callbacks = callbacks();
        let pointer = {
            let cleanup = root.cleanup.lock().unwrap();
            (&*cleanup.as_ref().unwrap().context) as *const Arc<State> as *const c_void
        };
        let key = unsafe {
            api.connect_sync_root(
                PCWSTR(name.as_ptr()),
                callbacks.as_ptr(),
                pointer,
                CF_CONNECT_FLAG_REQUIRE_PROCESS_INFO
                    | CF_CONNECT_FLAG_REQUIRE_FULL_FILE_PATH
                    | CF_CONNECT_FLAG_BLOCK_SELF_IMPLICIT_HYDRATION,
            )
        }?;
        state.connection.store(key.0, Ordering::Release);
        root.cleanup.lock().unwrap().as_mut().unwrap().connection = Some(key);
        root.create_placeholders()?;
        state.initialized.store(true, Ordering::Release);
        Ok(root)
    }
    fn paths(&self) -> Vec<PathBuf> {
        self.state
            .offer
            .description
            .roots
            .iter()
            .map(|name| clipboard_path(&self.path.join(name)))
            .collect()
    }
    fn create_placeholders(&self) -> Result<()> {
        let mut entries = (0..self.state.offer.description.entries.len()).collect::<Vec<_>>();
        entries.sort_by_key(|index| self.state.offer.description.entries[*index].path.len());
        for index in entries {
            let entry = &self.state.offer.description.entries[index];
            let parent = joined(&self.path, &entry.path[..entry.path.len() - 1]);
            let name = entry
                .path
                .last()
                .unwrap()
                .encode_utf16()
                .chain(Some(0))
                .collect::<Vec<_>>();
            let identity_blob = source_identity(&self.state.token, index);
            let mut placeholder = CF_PLACEHOLDER_CREATE_INFO {
                RelativeFileName: PCWSTR(name.as_ptr()),
                FileIdentity: identity_blob.as_ptr().cast(),
                FileIdentityLength: identity_blob.len() as u32,
                Flags: CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC
                    | CF_PLACEHOLDER_CREATE_FLAG_DISABLE_ON_DEMAND_POPULATION,
                ..Default::default()
            };
            placeholder.FsMetadata.FileSize = match entry.kind {
                DescriptorKind::Directory => 0,
                DescriptorKind::File { size } => size as i64,
            };
            placeholder.FsMetadata.BasicInfo.FileAttributes = match entry.kind {
                DescriptorKind::Directory => DIRECTORY,
                DescriptorKind::File { .. } => 0x20,
            } | NOT_INDEXED;
            let parent_name = wide(&parent);
            unsafe {
                self.state.api.create_placeholders(
                    PCWSTR(parent_name.as_ptr()),
                    std::slice::from_mut(&mut placeholder),
                    CF_CREATE_FLAG_STOP_ON_ERROR,
                )
            }?;
            let (_, identity) = attributes(&parent.join(entry.path.last().unwrap()))?;
            ensure!(
                identity.volume == self.state.root_identity.volume,
                "The native clipboard cache changed volumes"
            );
            self.state
                .identities
                .lock()
                .unwrap()
                .insert(index, identity);
            if matches!(entry.kind, DescriptorKind::File { size: 0 }) {
                self.state.verified.lock().unwrap().insert(index, identity);
            }
        }
        Ok(())
    }
    fn reapable(&self) -> bool {
        let retired = self.retired.lock().unwrap().is_some_and(|at| {
            at.elapsed() >= RETIRED_TTL || self.state.move_done.load(Ordering::Acquire)
        });
        // Each guard ends at its own statement; State::session/release_idle use
        // sessions before opened, so a compound temporary must not invert it.
        let sessions = self.state.sessions.lock().unwrap().is_empty();
        let opened = self.state.opened.lock().unwrap().is_empty();
        retired && self.state.busy.load(Ordering::Acquire) == 0 && opened && sessions
    }
}

struct Opened {
    count: usize,
    lease: Arc<dyn DemandProvider>,
}
struct Session {
    handle: Arc<dyn PasteSession>,
    consumer: NativeConsumer,
    destination: Option<PathBuf>,
}
struct PinnedSource {
    file: File,
    identity: Identity,
}
#[derive(Clone)]
struct MoveReceipt {
    index: usize,
    entries: Vec<usize>,
    consumer: NativeConsumer,
    target: Option<PathBuf>,
    session: Arc<dyn PasteSession>,
    rename: bool,
    created: Instant,
}
struct ActiveRequest {
    cancelled: Arc<AtomicBool>,
    range: Option<(u64, u64)>,
    consumer: Option<NativeConsumer>,
}
#[derive(Clone, Debug, Default)]
struct Coverage(Vec<(u64, u64)>);
impl Coverage {
    fn add(&mut self, offset: u64, count: u64, size: u64) -> Result<()> {
        let end = offset
            .checked_add(count)
            .context("Native file read overflow")?;
        ensure!(end <= size, "Native file read exceeded its file");
        if count == 0 {
            return Ok(());
        }
        let mut ranges = self.0.clone();
        ranges.push((offset, end));
        ranges.sort_unstable();
        let mut merged: Vec<(u64, u64)> = Vec::new();
        for (start, end) in ranges {
            if let Some(last) = merged.last_mut().filter(|last| start <= last.1) {
                last.1 = last.1.max(end);
            } else {
                merged.push((start, end));
            }
        }
        ensure!(merged.len() <= 1024, "Too many disjoint native file reads");
        self.0 = merged;
        Ok(())
    }
    fn covers(&self, size: u64) -> bool {
        size == 0 || self.0.as_slice() == [(0, size)]
    }
}
struct State {
    offer: RemoteOffer,
    api: Arc<Api>,
    token: Vec<u8>,
    owner: Mutex<Weak<Root>>,
    connection: AtomicI64,
    root_identity: Identity,
    identities: Mutex<BTreeMap<usize, Identity>>,
    verified: Mutex<BTreeMap<usize, Identity>>,
    hydrated: Mutex<BTreeMap<usize, Coverage>>,
    consumer_reads: Mutex<BTreeMap<(u32, u64, usize), Coverage>>,
    requests: Mutex<BTreeMap<(i64, i64), ActiveRequest>>,
    file_locks: Vec<Mutex<()>>,
    pinned: Vec<Mutex<Option<PinnedSource>>>,
    opened: Mutex<BTreeMap<(i64, u32, u64), Opened>>,
    sessions: Mutex<BTreeMap<(u32, u64), Session>>,
    explicit: Mutex<BTreeMap<usize, NativeConsumer>>,
    moves: Mutex<BTreeMap<(i64, i64), MoveReceipt>>,
    moved: Mutex<BTreeSet<usize>>,
    destinations: Mutex<BTreeMap<usize, (PathBuf, bool)>>,
    accepting: AtomicBool,
    tearing_down: AtomicBool,
    busy: AtomicUsize,
    initialized: AtomicBool,
    ambiguous: AtomicBool,
    move_blocked: AtomicBool,
    move_done: AtomicBool,
    monitor_running: AtomicBool,
    native_activity: Mutex<Instant>,
    #[cfg(test)]
    test_destination: Mutex<Option<(u32, PathBuf)>>,
    move_lock: Mutex<()>,
}

// The kernel owns all callback pointers. Only bounded, owned values leave the
// callback; in particular neither a PCWSTR nor CF_CALLBACK_INFO is queued.
#[derive(Clone)]
struct Request {
    index: usize,
    file: i64,
    transfer: i64,
    key: i64,
    consumer: Option<NativeConsumer>,
    self_process: bool,
    volume: PathBuf,
    range: Option<(u64, u64)>,
}
impl Request {
    fn operation(&self, state: &State, kind: CF_OPERATION_TYPE) -> CF_OPERATION_INFO {
        CF_OPERATION_INFO {
            StructSize: size_of::<CF_OPERATION_INFO>() as u32,
            Type: kind,
            ConnectionKey: CF_CONNECTION_KEY(state.connection.load(Ordering::Acquire)),
            TransferKey: self.transfer,
            RequestKey: self.key,
            ..Default::default()
        }
    }
}
unsafe fn bounded_text(value: PCWSTR) -> Result<OsString> {
    ensure!(!value.is_null(), "Missing native clipboard path");
    for length in 0..32_768 {
        if unsafe { *value.0.add(length) } == 0 {
            return Ok(OsString::from_wide(unsafe {
                std::slice::from_raw_parts(value.0, length)
            }));
        }
    }
    bail!("The native clipboard path is too long")
}
unsafe fn snapshot(info: *const CF_CALLBACK_INFO) -> Result<(Arc<Root>, Request)> {
    ensure!(!info.is_null(), "Missing Cloud Files callback information");
    // RequestKey is version dependent; reject older layouts rather than reading
    // beyond their allocation or matching an ambiguous native operation.
    ensure!(
        unsafe { (*info).StructSize } as usize >= size_of::<CF_CALLBACK_INFO>(),
        "The Cloud Files callback layout is unsupported"
    );
    let info = unsafe { &*info };
    ensure!(
        !info.CallbackContext.is_null(),
        "Missing native file provider"
    );
    let state = unsafe { &*info.CallbackContext.cast::<Arc<State>>() }.clone();
    let root = state
        .owner
        .lock()
        .unwrap()
        .upgrade()
        .context("The native file provider stopped")?;
    ensure!(
        state.initialized.load(Ordering::Acquire),
        "The native file provider is not ready"
    );
    ensure!(
        info.ConnectionKey.0 == state.connection.load(Ordering::Acquire)
            && info.VolumeSerialNumber == state.root_identity.volume
            && info.SyncRootFileId == state.root_identity.file,
        "The native file request belongs to another cache"
    );
    ensure!(
        info.SyncRootIdentityLength as usize == state.token.len()
            && !info.SyncRootIdentity.is_null(),
        "Missing native cache identity"
    );
    ensure!(
        unsafe {
            std::slice::from_raw_parts(info.SyncRootIdentity.cast::<u8>(), state.token.len())
        } == state.token,
        "The native cache identity changed"
    );
    ensure!(
        info.FileIdentityLength == 24 && !info.FileIdentity.is_null(),
        "Missing native file identity"
    );
    let index = decode_identity(
        unsafe { std::slice::from_raw_parts(info.FileIdentity.cast::<u8>(), 24) },
        &state.token,
        state.offer.description.entries.len(),
    )
    .context("Unknown native file identity")?;
    ensure!(
        state
            .identities
            .lock()
            .unwrap()
            .get(&index)
            .is_some_and(|id| id.file == info.FileId),
        "The native file identity changed"
    );
    if let DescriptorKind::File { size } = state.offer.description.entries[index].kind {
        ensure!(
            info.FileSize >= 0 && info.FileSize as u64 == size,
            "The native file size changed"
        );
    }
    let mut process = 0;
    if !info.ProcessInfo.is_null() {
        let process_info = unsafe { &*info.ProcessInfo };
        if process_info.StructSize as usize
            >= offset_of!(CF_PROCESS_INFO, ProcessId) + size_of::<u32>()
        {
            process = process_info.ProcessId;
        }
    }
    let self_process = process == std::process::id();
    let consumer = if self_process {
        state.explicit.lock().unwrap().get(&index).cloned()
    } else {
        native_consumer(process, &crate::config::hex(&state.token))
    };
    let volume = PathBuf::from(unsafe { bounded_text(info.VolumeDosName) }?);
    Ok((
        root,
        Request {
            index,
            file: info.FileId,
            transfer: info.TransferKey,
            key: info.RequestKey,
            consumer,
            self_process,
            volume,
            range: None,
        },
    ))
}
unsafe fn parameters<'a, T>(
    raw: *const CF_CALLBACK_PARAMETERS,
) -> Result<&'a CF_CALLBACK_PARAMETERS> {
    ensure!(!raw.is_null(), "Missing Cloud Files callback parameters");
    // CF_CALLBACK_PARAMETERS documents ParamSize as ULONG plus the specific
    // variant, rather than the native union's padded x64 offset.
    ensure!(
        unsafe { (*raw).ParamSize } as usize >= size_of::<u32>() + size_of::<T>(),
        "The Cloud Files request layout is unsupported"
    );
    Ok(unsafe { &*raw })
}
fn callback(action: impl FnOnce() -> Result<()>) {
    // Rust unwinding must never cross the system callback boundary. Individual
    // workers explicitly fail native I/O; malformed metadata does not gain a
    // content lease or a source-deletion receipt.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(action));
}
fn io_callback(
    info: *const CF_CALLBACK_INFO,
    kind: CF_OPERATION_TYPE,
    action: impl FnOnce() -> Result<()>,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(action));
    if matches!(result, Ok(Ok(()))) {
        return;
    }
    // Reject unsupported callback versions and tampered identities promptly.
    // Failing this native request grants neither file access nor a Move receipt.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        if info.is_null() || ((*info).StructSize as usize) < size_of::<CF_CALLBACK_INFO>() {
            return;
        }
        let info = &*info;
        if info.CallbackContext.is_null() {
            return;
        }
        let state = &*info.CallbackContext.cast::<Arc<State>>();
        if info.ConnectionKey.0 != state.connection.load(Ordering::Acquire)
            || info.SyncRootFileId != state.root_identity.file
            || info.VolumeSerialNumber != state.root_identity.volume
        {
            return;
        }
        let operation = CF_OPERATION_INFO {
            StructSize: size_of::<CF_OPERATION_INFO>() as u32,
            Type: kind,
            ConnectionKey: info.ConnectionKey,
            TransferKey: info.TransferKey,
            RequestKey: info.RequestKey,
            ..Default::default()
        };
        let mut args = CF_OPERATION_PARAMETERS::default();
        if kind == CF_OPERATION_TYPE_TRANSFER_DATA {
            args.ParamSize = (offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
                + size_of::<CF_OPERATION_PARAMETERS_0_0>()) as u32;
            args.Anonymous.TransferData = CF_OPERATION_PARAMETERS_0_0 {
                CompletionStatus: STATUS_FAILED,
                Length: info.FileSize.max(0),
                ..Default::default()
            };
        } else if kind == CF_OPERATION_TYPE_ACK_DATA {
            args.ParamSize = (offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
                + size_of::<CF_OPERATION_PARAMETERS_0_2>()) as u32;
            args.Anonymous.AckData = CF_OPERATION_PARAMETERS_0_2 {
                CompletionStatus: STATUS_FAILED,
                Length: info.FileSize.max(0),
                ..Default::default()
            };
        } else if kind == CF_OPERATION_TYPE_ACK_RENAME {
            args.ParamSize = (offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
                + size_of::<CF_OPERATION_PARAMETERS_0_6>()) as u32;
            args.Anonymous.AckRename = CF_OPERATION_PARAMETERS_0_6 {
                CompletionStatus: STATUS_FAILED,
                ..Default::default()
            };
        } else {
            args.ParamSize = (offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
                + size_of::<CF_OPERATION_PARAMETERS_0_7>()) as u32;
            args.Anonymous.AckDelete = CF_OPERATION_PARAMETERS_0_7 {
                CompletionStatus: STATUS_FAILED,
                ..Default::default()
            };
        }
        let _ = state.api.execute(&operation, &mut args);
    }));
}
fn callbacks() -> [CF_CALLBACK_REGISTRATION; 10] {
    [
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_FETCH_DATA,
            Callback: Some(fetch_callback),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_VALIDATE_DATA,
            Callback: Some(validate_callback),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_CANCEL_FETCH_DATA,
            Callback: Some(cancel_callback),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NOTIFY_FILE_OPEN_COMPLETION,
            Callback: Some(open_callback),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NOTIFY_FILE_CLOSE_COMPLETION,
            Callback: Some(close_callback),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NOTIFY_RENAME,
            Callback: Some(rename_callback),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NOTIFY_RENAME_COMPLETION,
            Callback: Some(rename_complete_callback),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NOTIFY_DELETE,
            Callback: Some(delete_callback),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NOTIFY_DELETE_COMPLETION,
            Callback: Some(delete_complete_callback),
        },
        CF_CALLBACK_REGISTRATION {
            Type: CF_CALLBACK_TYPE_NONE,
            Callback: None,
        },
    ]
}
struct Work {
    root: Arc<Root>,
    key: (i64, i64),
    cancelled: Arc<AtomicBool>,
}
impl Work {
    fn start(root: Arc<Root>, request: &Request) -> Result<Self> {
        ACTIVE_WORKERS
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < REQUEST_LIMIT).then_some(count + 1)
            })
            .map_err(|_| anyhow!("Too many native file reads are pending"))?;
        let key = (request.transfer, request.key);
        let cancelled = Arc::new(AtomicBool::new(false));
        root.state.requests.lock().unwrap().insert(
            key,
            ActiveRequest {
                cancelled: cancelled.clone(),
                range: request.range,
                consumer: request.consumer.clone(),
            },
        );
        root.state.busy.fetch_add(1, Ordering::AcqRel);
        Ok(Self {
            root,
            key,
            cancelled,
        })
    }
    fn check(&self) -> Result<()> {
        ensure!(
            !self.cancelled.load(Ordering::Acquire)
                && !self.root.state.tearing_down.load(Ordering::Acquire),
            "The native file read was cancelled"
        );
        Ok(())
    }
}
impl Drop for Work {
    fn drop(&mut self) {
        let mut requests = self.root.state.requests.lock().unwrap();
        if requests
            .get(&self.key)
            .is_some_and(|current| Arc::ptr_eq(&current.cancelled, &self.cancelled))
        {
            requests.remove(&self.key);
        }
        drop(requests);
        self.root.state.busy.fetch_sub(1, Ordering::AcqRel);
        ACTIVE_WORKERS.fetch_sub(1, Ordering::AcqRel);
        self.root.state.release_idle_sessions();
    }
}
struct Keepalive {
    stop: Arc<(Mutex<bool>, Condvar)>,
}
impl Keepalive {
    fn start(
        work: &Work,
        request: &Request,
        deadline: Option<(CF_OPERATION_TYPE, Arc<dyn PasteSession>)>,
    ) -> Self {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let signal = stop.clone();
        let root = work.root.clone();
        let request = request.clone();
        let cancelled = work.cancelled.clone();
        thread::spawn(move || {
            let started = Instant::now();
            let (lock, changed) = &*signal;
            let mut done = lock.lock().unwrap();
            loop {
                let result = changed.wait_timeout(done, HEARTBEAT).unwrap();
                done = result.0;
                if *done || cancelled.load(Ordering::Acquire) {
                    break;
                }
                if let Some((kind, session)) = &deadline {
                    if started.elapsed() >= NATIVE_DEADLINE {
                        cancelled.store(true, Ordering::Release);
                        let _ = root.state.ack_action(&request, *kind, STATUS_CANCELLED);
                        let _ = session.native_complete(NativeResult::Failed(
                            "The native file operation timed out; originals were retained".into(),
                        ));
                        break;
                    }
                } else {
                    // This is a liveness report while verified staging or a
                    // user pause is pending, not fictitious transferred bytes.
                    let total = root.state.file_size(request.index).unwrap_or(0).max(1) as i64;
                    if root
                        .state
                        .api
                        .report_provider_progress(
                            CF_CONNECTION_KEY(root.state.connection.load(Ordering::Acquire)),
                            request.transfer,
                            total,
                            0,
                        )
                        .is_err()
                    {
                        cancelled.store(true, Ordering::Release);
                        let _ = root.state.fail_data(&request, STATUS_FAILED);
                        break;
                    }
                }
            }
        });
        Self { stop }
    }
}
impl Drop for Keepalive {
    fn drop(&mut self) {
        let (lock, changed) = &*self.stop;
        *lock.lock().unwrap() = true;
        changed.notify_one();
    }
}
impl State {
    fn capture_destination(&self, process: u32) -> Option<PathBuf> {
        #[cfg(test)]
        if std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true")
            && std::env::var("RUNNER_ENVIRONMENT").as_deref() == Ok("github-hosted")
            && std::env::var("DESK_SWITCH_NATIVE_CLIPBOARD_TEST").as_deref() == Ok("1")
            && let Some((expected, path)) = self.test_destination.lock().unwrap().as_ref()
            && *expected == process
        {
            return Some(path.clone());
        }
        super::destination::capture(process)
    }
    fn source<'a>(
        &'a self,
        index: usize,
        session: &Arc<dyn PasteSession>,
        cancelled: &AtomicBool,
    ) -> Result<MutexGuard<'a, Option<PinnedSource>>> {
        let mut pinned = cancellable_lock(&self.pinned[index], cancelled)?;
        if pinned.is_none() {
            let file = session.open_verified_cancellable(index, cancelled)?;
            let identity = identity(&file)?;
            *pinned = Some(PinnedSource { file, identity });
        }
        let source = pinned.as_ref().unwrap();
        ensure!(
            identity(&source.file)? == source.identity,
            "The verified transfer cache changed"
        );
        Ok(pinned)
    }
    fn release_idle_sessions(&self) {
        if self.busy.load(Ordering::Acquire) != 0 {
            return;
        }
        let mut sessions = self.sessions.lock().unwrap();
        let opened = self.opened.lock().unwrap();
        sessions.retain(|key, session| {
            let handles = opened
                .keys()
                .any(|(_, id, started)| (*id, *started) == *key);
            let terminal = session.handle.is_terminal();
            if !handles
                && terminal
                && self.offer.operation == Operation::Move
                && (!session.handle.can_retry_move() || !self.moved.lock().unwrap().is_empty())
            {
                self.move_blocked.store(true, Ordering::Release);
            }
            retain_native_session(self.offer.operation, handles, terminal)
        });
        drop(sessions);
        drop(opened);
        self.moves
            .lock()
            .unwrap()
            .retain(|_, receipt| receipt.created.elapsed() < Duration::from_secs(60));
    }
    fn file_size(&self, index: usize) -> Result<u64> {
        match self.offer.description.entries[index].kind {
            DescriptorKind::File { size } => Ok(size),
            DescriptorKind::Directory => bail!("The native request is a folder"),
        }
    }
    fn path(&self, root: &Root, index: usize) -> PathBuf {
        joined(&root.path, &self.offer.description.entries[index].path)
    }
    fn session(
        &self,
        request: &Request,
        destination: Option<String>,
    ) -> Result<Arc<dyn PasteSession>> {
        let consumer = request
            .consumer
            .clone()
            .context("The native file consumer could not be identified")?;
        let key = (consumer.process_id, consumer.started);
        let mut sessions = self.sessions.lock().unwrap();
        if self.offer.operation == Operation::Move {
            ensure!(
                !self.move_blocked.load(Ordering::Acquire),
                "This Cut operation has ended; cut the files again"
            );
            if sessions.keys().any(|existing| *existing != key) {
                self.ambiguous.store(true, Ordering::Release);
            }
            ensure!(
                !self.ambiguous.load(Ordering::Acquire),
                "The Cut selection has multiple consumers; originals were retained"
            );
        }
        if let Some(existing) = sessions.get(&key) {
            ensure!(
                existing.consumer == consumer,
                "The native file operation identity changed"
            );
            ensure!(
                !existing.handle.is_terminal(),
                "This native file operation has ended"
            );
            return Ok(existing.handle.clone());
        }
        let lease = {
            let opened = self.opened.lock().unwrap();
            let identities = self.identities.lock().unwrap();
            opened
                .iter()
                .find_map(|((file, process, started), opened)| {
                    if (*process, *started) != key {
                        return None;
                    }
                    let authority = identities
                        .iter()
                        .find_map(|(index, identity)| (identity.file == *file).then_some(*index))?;
                    let entry = &self.offer.description.entries[authority];
                    let requested = &self.offer.description.entries[request.index];
                    let permits = authority == request.index
                        || (matches!(entry.kind, DescriptorKind::Directory)
                            && requested.path.starts_with(&entry.path));
                    permits.then(|| opened.lease.clone())
                })
        };
        ensure!(
            self.accepting.load(Ordering::Acquire) || lease.is_some(),
            "The copied file selection has been replaced"
        );
        let provider = match lease {
            Some(provider) => provider,
            None => self.offer.provider.lease()?,
        };
        // Capture before begin emits PasteStarted and focuses our progress
        // window. Unknown Explorer folders never become guessed destinations.
        let folder = self.capture_destination(consumer.process_id);
        let handle = provider.begin(PasteIntent {
            operation: self.offer.operation,
            completion: if self.offer.operation == Operation::Move {
                CompletionSupport::Reported
            } else {
                CompletionSupport::Unavailable
            },
            destination: destination.or_else(|| {
                folder
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned())
            }),
            consumer: Some(consumer.clone()),
        })?;
        sessions.insert(
            key,
            Session {
                handle: handle.clone(),
                consumer,
                destination: folder,
            },
        );
        *self.native_activity.lock().unwrap() = Instant::now();
        if self.offer.operation == Operation::Move
            && let Some(root) = self.owner.lock().unwrap().upgrade()
        {
            monitor_move(&root);
        }
        Ok(handle)
    }
    fn ack_data(
        &self,
        request: &Request,
        offset: u64,
        length: u64,
        status: NTSTATUS,
    ) -> Result<()> {
        let info = request.operation(self, CF_OPERATION_TYPE_ACK_DATA);
        let mut args = CF_OPERATION_PARAMETERS {
            ParamSize: (offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
                + size_of::<CF_OPERATION_PARAMETERS_0_2>()) as u32,
            Anonymous: CF_OPERATION_PARAMETERS_0 {
                AckData: CF_OPERATION_PARAMETERS_0_2 {
                    CompletionStatus: status,
                    Offset: offset as i64,
                    Length: length as i64,
                    ..Default::default()
                },
            },
        };
        unsafe { self.api.execute(&info, &mut args) }
    }
    fn fail_data(&self, request: &Request, status: NTSTATUS) -> Result<()> {
        let info = request.operation(self, CF_OPERATION_TYPE_TRANSFER_DATA);
        let mut args = CF_OPERATION_PARAMETERS {
            ParamSize: (offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
                + size_of::<CF_OPERATION_PARAMETERS_0_0>()) as u32,
            Anonymous: CF_OPERATION_PARAMETERS_0 {
                TransferData: CF_OPERATION_PARAMETERS_0_0 {
                    CompletionStatus: status,
                    Offset: 0,
                    Length: self.file_size(request.index).unwrap_or(0) as i64,
                    ..Default::default()
                },
            },
        };
        unsafe { self.api.execute(&info, &mut args) }
    }
    fn ack_action(
        &self,
        request: &Request,
        kind: CF_OPERATION_TYPE,
        status: NTSTATUS,
    ) -> Result<()> {
        let info = request.operation(self, kind);
        let mut args = if kind == CF_OPERATION_TYPE_ACK_RENAME {
            CF_OPERATION_PARAMETERS {
                ParamSize: (offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
                    + size_of::<CF_OPERATION_PARAMETERS_0_6>()) as u32,
                Anonymous: CF_OPERATION_PARAMETERS_0 {
                    AckRename: CF_OPERATION_PARAMETERS_0_6 {
                        CompletionStatus: status,
                        ..Default::default()
                    },
                },
            }
        } else {
            CF_OPERATION_PARAMETERS {
                ParamSize: (offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
                    + size_of::<CF_OPERATION_PARAMETERS_0_7>()) as u32,
                Anonymous: CF_OPERATION_PARAMETERS_0 {
                    AckDelete: CF_OPERATION_PARAMETERS_0_7 {
                        CompletionStatus: status,
                        ..Default::default()
                    },
                },
            }
        };
        unsafe { self.api.execute(&info, &mut args) }
    }
    fn transfer(&self, request: &Request, offset: u64, data: &[u8]) -> Result<()> {
        let info = request.operation(self, CF_OPERATION_TYPE_TRANSFER_DATA);
        let mut args = CF_OPERATION_PARAMETERS {
            ParamSize: (offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
                + size_of::<CF_OPERATION_PARAMETERS_0_0>()) as u32,
            Anonymous: CF_OPERATION_PARAMETERS_0 {
                TransferData: CF_OPERATION_PARAMETERS_0_0 {
                    CompletionStatus: STATUS_OK,
                    Buffer: data.as_ptr().cast(),
                    Offset: offset as i64,
                    Length: data.len() as i64,
                    ..Default::default()
                },
            },
        };
        unsafe { self.api.execute(&info, &mut args) }
    }
    fn retrieve(&self, request: &Request, offset: u64, data: &mut [u8]) -> Result<()> {
        let info = request.operation(self, CF_OPERATION_TYPE_RETRIEVE_DATA);
        let mut args = CF_OPERATION_PARAMETERS {
            ParamSize: (offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
                + size_of::<CF_OPERATION_PARAMETERS_0_1>()) as u32,
            Anonymous: CF_OPERATION_PARAMETERS_0 {
                RetrieveData: CF_OPERATION_PARAMETERS_0_1 {
                    Buffer: data.as_mut_ptr().cast(),
                    Offset: offset as i64,
                    Length: data.len() as i64,
                    ..Default::default()
                },
            },
        };
        unsafe { self.api.execute(&info, &mut args) }?;
        ensure!(
            unsafe { args.Anonymous.RetrieveData.ReturnedLength } == data.len() as i64,
            "The native file cache returned an incomplete range"
        );
        Ok(())
    }
}
fn retain_native_session(operation: Operation, handles: bool, terminal: bool) -> bool {
    handles || (operation == Operation::Move && !terminal)
}
fn monitor_move(root: &Arc<Root>) {
    if root.state.monitor_running.swap(true, Ordering::AcqRel) {
        return;
    }
    let root = root.clone();
    thread::spawn(move || {
        loop {
            thread::park_timeout(Duration::from_millis(250));
            let state = &root.state;
            let expired = {
                let mut pending = state.moves.lock().unwrap();
                let keys = pending
                    .iter()
                    .filter_map(|(key, receipt)| {
                        (receipt.session.is_terminal()
                            || receipt.created.elapsed() >= NATIVE_DEADLINE)
                            .then_some(*key)
                    })
                    .collect::<Vec<_>>();
                keys.into_iter()
                    .filter_map(|key| pending.remove(&key))
                    .collect::<Vec<_>>()
            };
            for receipt in expired {
                if !receipt.session.is_terminal() {
                    let _ = receipt.session.native_complete(NativeResult::Failed(
                        "The native Move did not confirm completion; originals were retained"
                            .into(),
                    ));
                }
            }
            let idle =
                state.busy.load(Ordering::Acquire) == 0 && state.opened.lock().unwrap().is_empty();
            if idle && state.native_activity.lock().unwrap().elapsed() >= NATIVE_DEADLINE {
                let abandoned = state
                    .sessions
                    .lock()
                    .unwrap()
                    .values()
                    .filter(|session| !session.handle.is_terminal())
                    .map(|session| session.handle.clone())
                    .collect::<Vec<_>>();
                for session in abandoned {
                    let _ = session.native_complete(NativeResult::Failed(
                        "The native consumer did not complete Move; originals were retained".into(),
                    ));
                }
            }
            state.release_idle_sessions();
            // Serialize the handoff with State::session so a new session cannot
            // miss its watchdog between this empty check and flag reset.
            let sessions = state.sessions.lock().unwrap();
            if sessions.is_empty() && state.moves.lock().unwrap().is_empty() {
                state.monitor_running.store(false, Ordering::Release);
                break;
            }
        }
    });
}
fn cancellable_lock<'a, T>(
    lock: &'a Mutex<T>,
    cancelled: &AtomicBool,
) -> Result<MutexGuard<'a, T>> {
    loop {
        ensure!(
            !cancelled.load(Ordering::Acquire),
            "The native file read was cancelled"
        );
        match lock.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(std::sync::TryLockError::WouldBlock) => {
                thread::park_timeout(Duration::from_millis(50))
            }
            Err(_) => bail!("The native file operation stopped"),
        }
    }
}
#[cfg(test)]
fn valid_range(offset: i64, length: i64, size: u64) -> bool {
    offset >= 0
        && length > 0
        && (offset as u64).is_multiple_of(4096)
        && (offset as u64)
            .checked_add(length as u64)
            .is_some_and(|end| end <= size && ((length as u64).is_multiple_of(4096) || end == size))
}
fn requested_range(offset: i64, length: i64, size: u64) -> Option<(u64, u64)> {
    if offset < 0 || length == 0 || length < -1 || offset as u64 >= size {
        return None;
    }
    let offset = offset as u64;
    let end = if length == -1 {
        size
    } else {
        offset.checked_add(length as u64)?.min(size)
    };
    (offset.is_multiple_of(4096)
        && end > offset
        && ((end - offset).is_multiple_of(4096) || end == size))
        .then_some((offset, end - offset))
}
fn cancelled_range_covers(offset: i64, length: i64, required: (u64, u64)) -> bool {
    if offset < 0 || length == 0 || length < -1 || offset as u64 > required.0 {
        return false;
    }
    let end = if length == -1 {
        u64::MAX
    } else {
        let Some(end) = (offset as u64).checked_add(length as u64) else {
            return false;
        };
        end
    };
    required
        .0
        .checked_add(required.1)
        .is_some_and(|required_end| end >= required_end)
}
fn hydrate(work: &Work, request: &Request) -> Result<()> {
    let state = &work.root.state;
    let session = state.session(request, None)?;
    let _keepalive = Keepalive::start(work, request, None);
    // Serialize two range requests for the same file; consumers still have
    // independent cancellation and leases, with one shared network transfer.
    let _file_lock = cancellable_lock(&state.file_locks[request.index], &work.cancelled)?;
    work.check()?;
    let size = state.file_size(request.index)?;
    let (start, length) = request.range.context("Missing native data-demand range")?;
    let end = start
        .checked_add(length)
        .context("Native data-demand overflow")?;
    ensure!(end <= size, "The native data demand exceeded its file");
    let mut pinned = state.source(request.index, &session, &work.cancelled)?;
    let source = &mut pinned.as_mut().unwrap().file;
    let path = state.path(&work.root, request.index);
    let (_, before) = attributes(&path)?;
    let expected = *state
        .identities
        .lock()
        .unwrap()
        .get(&request.index)
        .context("The native file was removed")?;
    ensure!(
        before.file == expected.file && before.volume == expected.volume && before.size == size,
        "The native clipboard cache changed"
    );
    source.seek(SeekFrom::Start(start))?;
    let mut data = vec![0u8; BLOCK];
    let mut offset = start;
    while offset < end {
        work.check()?;
        session.wait_unpaused_cancellable(&work.cancelled)?;
        let count = (end - offset).min(BLOCK as u64) as usize;
        source.read_exact(&mut data[..count])?;
        state.transfer(request, offset, &data[..count])?;
        session.report_read(request.index, offset, count)?;
        offset += count as u64;
    }
    // Validation is deliberately separate from TransferData: an unverified
    // placeholder must not become a completed native Move by being renamed.
    source.seek(SeekFrom::Start(start))?;
    let mut received = vec![0u8; BLOCK];
    offset = start;
    while offset < end {
        work.check()?;
        let count = (end - offset).min(BLOCK as u64) as usize;
        source.read_exact(&mut data[..count])?;
        state.retrieve(request, offset, &mut received[..count])?;
        ensure!(
            received[..count] == data[..count],
            "The native clipboard cache failed verification"
        );
        offset += count as u64;
    }
    ensure!(
        identity(&pinned.as_ref().unwrap().file)? == pinned.as_ref().unwrap().identity,
        "The verified transfer cache changed while reading"
    );
    let (_, verified) = attributes(&path)?;
    ensure!(
        verified.volume == expected.volume
            && verified.file == expected.file
            && verified.size == size,
        "The native clipboard cache changed while hydrating"
    );
    work.check()?;
    // Capture the generation before releasing the caller's pending read. Later
    // writes must invalidate Move authorization rather than becoming our data.
    let complete = {
        let mut hydrated = state.hydrated.lock().unwrap();
        let coverage = hydrated.entry(request.index).or_default();
        coverage.add(start, length, size)?;
        coverage.covers(size)
    };
    if complete {
        state
            .verified
            .lock()
            .unwrap()
            .insert(request.index, verified);
    }
    // Count only the filter's required demand from the external consumer.
    // Explicit pre-rename hydration and optional/speculative bytes never prove
    // that the native copy consumer requested a complete file.
    if !request.self_process
        && let Some(consumer) = &request.consumer
    {
        state
            .consumer_reads
            .lock()
            .unwrap()
            .entry((consumer.process_id, consumer.started, request.index))
            .or_default()
            .add(start, length, size)?;
    }
    state.ack_data(request, start, length, STATUS_OK)?;
    Ok(())
}
unsafe extern "system" fn fetch_callback(
    info: *const CF_CALLBACK_INFO,
    raw: *const CF_CALLBACK_PARAMETERS,
) {
    io_callback(info, CF_OPERATION_TYPE_TRANSFER_DATA, || {
        let (root, mut request) = unsafe { snapshot(info) }?;
        let args = unsafe {
            parameters::<CF_CALLBACK_PARAMETERS_0_1>(raw)?
                .Anonymous
                .FetchData
        };
        let range = requested_range(
            args.RequiredFileOffset,
            args.RequiredLength,
            root.state.file_size(request.index)?,
        );
        if range.is_none() {
            return root.state.fail_data(&request, STATUS_FAILED);
        }
        request.range = range;
        let work = match Work::start(root.clone(), &request) {
            Ok(work) => work,
            Err(_) => return root.state.fail_data(&request, STATUS_FAILED),
        };
        thread::spawn(move || {
            if let Err(error) = hydrate(&work, &request) {
                let _ = work.root.state.fail_data(&request, STATUS_FAILED);
                // A range cancellation does not cancel another reader's lease.
                if !work.cancelled.load(Ordering::Acquire)
                    && let Some(consumer) = &request.consumer
                    && let Some(session) = work
                        .root
                        .state
                        .sessions
                        .lock()
                        .unwrap()
                        .get(&(consumer.process_id, consumer.started))
                {
                    let _ = session
                        .handle
                        .native_complete(NativeResult::Failed(error.to_string()));
                }
            }
        });
        Ok(())
    });
}
unsafe extern "system" fn validate_callback(
    info: *const CF_CALLBACK_INFO,
    raw: *const CF_CALLBACK_PARAMETERS,
) {
    io_callback(info, CF_OPERATION_TYPE_ACK_DATA, || {
        let (root, request) = unsafe { snapshot(info) }?;
        let args = unsafe {
            parameters::<CF_CALLBACK_PARAMETERS_0_2>(raw)?
                .Anonymous
                .ValidateData
        };
        let size = root.state.file_size(request.index)?;
        let (offset, length) = requested_range(args.RequiredFileOffset, args.RequiredLength, size)
            .context("Invalid native validation range")?;
        // A hydration worker acknowledges its entire verified file. Validation
        // may be delivered while that worker is still waiting for remote bytes;
        // leave it pending for that ACK rather than racing a second verdict.
        if root
            .state
            .requests
            .lock()
            .unwrap()
            .keys()
            .any(|(transfer, _)| *transfer == request.transfer)
        {
            return Ok(());
        }
        let _file_lock = match root.state.file_locks[request.index].try_lock() {
            Ok(lock) => lock,
            Err(std::sync::TryLockError::WouldBlock) => return Ok(()),
            Err(_) => {
                return root.state.ack_data(&request, offset, length, STATUS_FAILED);
            }
        };
        let state = &root.state;
        let verified = state.verified.lock().unwrap().get(&request.index).copied();
        let current = attributes(&state.path(&root, request.index))
            .ok()
            .map(|(_, id)| id);
        let status = if verified.is_some() && verified == current {
            STATUS_OK
        } else {
            STATUS_FAILED
        };
        state.ack_data(&request, offset, length, status)
    });
}
unsafe extern "system" fn cancel_callback(
    info: *const CF_CALLBACK_INFO,
    raw: *const CF_CALLBACK_PARAMETERS,
) {
    callback(|| {
        let (root, request) = unsafe { snapshot(info) }?;
        ensure!(
            !raw.is_null()
                && unsafe { (*raw).ParamSize } as usize
                    >= size_of::<CF_CALLBACK_PARAMETERS_0_0_0_0>() + 2 * size_of::<u32>(),
            "The Cloud Files cancellation layout is unsupported"
        );
        let args = unsafe { (*raw).Anonymous.Cancel.Anonymous.FetchData };
        let cancelled_session = {
            let mut requests = root.state.requests.lock().unwrap();
            let Some(active) = requests.get_mut(&(request.transfer, request.key)) else {
                return Ok(());
            };
            // Partial cancellation does not retire remaining range readers or
            // cancel the shared network fetch owned by another native lease.
            if !active.range.is_some_and(|required| {
                cancelled_range_covers(args.FileOffset, args.Length, required)
            }) {
                return Ok(());
            }
            active.cancelled.store(true, Ordering::Release);
            let consumer = active.consumer.clone();
            let has_live_range = consumer.as_ref().is_some_and(|consumer| {
                requests.values().any(|other| {
                    other.consumer.as_ref() == Some(consumer)
                        && !other.cancelled.load(Ordering::Acquire)
                })
            });
            (!has_live_range).then_some(consumer).flatten()
        };
        if let Some(consumer) = cancelled_session {
            let session = root
                .state
                .sessions
                .lock()
                .unwrap()
                .get(&(consumer.process_id, consumer.started))
                .map(|session| session.handle.clone());
            if let Some(session) = session {
                let _ = session.native_complete(NativeResult::Cancelled);
            }
        }
        Ok(())
    });
}
unsafe extern "system" fn open_callback(
    info: *const CF_CALLBACK_INFO,
    _raw: *const CF_CALLBACK_PARAMETERS,
) {
    callback(|| {
        let (root, request) = unsafe { snapshot(info) }?;
        if request.self_process || root.state.tearing_down.load(Ordering::Acquire) {
            return Ok(());
        }
        let Some(consumer) = request.consumer else {
            return Ok(());
        };
        *root.state.native_activity.lock().unwrap() = Instant::now();
        let key = (request.file, consumer.process_id, consumer.started);
        let mut opened = root.state.opened.lock().unwrap();
        if let Some(existing) = opened.get_mut(&key) {
            existing.count += 1;
        } else if root.state.accepting.load(Ordering::Acquire) {
            opened.insert(
                key,
                Opened {
                    count: 1,
                    lease: root.state.offer.provider.lease()?,
                },
            );
        }
        Ok(())
    });
}
unsafe extern "system" fn close_callback(
    info: *const CF_CALLBACK_INFO,
    _raw: *const CF_CALLBACK_PARAMETERS,
) {
    callback(|| {
        let (root, request) = unsafe { snapshot(info) }?;
        if request.self_process {
            return Ok(());
        }
        let Some(consumer) = request.consumer else {
            return Ok(());
        };
        *root.state.native_activity.lock().unwrap() = Instant::now();
        let key = (request.file, consumer.process_id, consumer.started);
        let mut opened = root.state.opened.lock().unwrap();
        if let Some(existing) = opened.get_mut(&key) {
            existing.count = existing.count.saturating_sub(1);
            if existing.count == 0 {
                opened.remove(&key);
            }
        }
        // Metadata-only closes must not create a session or declare a Copy or
        // Move completed. Copy's verified stage is owned by the transfer service.
        let has_handles = opened
            .keys()
            .any(|(_, id, started)| *id == consumer.process_id && *started == consumer.started);
        drop(opened);
        if !has_handles {
            root.state.release_idle_sessions();
        }
        Ok(())
    });
}

fn branch(description: &protocol::Description, index: usize) -> Vec<usize> {
    let path = &description.entries[index].path;
    description
        .entries
        .iter()
        .enumerate()
        .filter_map(|(candidate, entry)| entry.path.starts_with(path).then_some(candidate))
        .collect()
}
fn move_target(volume: &Path, relative: &OsString, root: &Path) -> Result<PathBuf> {
    let relative = PathBuf::from(relative);
    // The filter supplies a volume-relative path, not a process-relative path
    // or a caller-chosen drive. Reject traversal and prefix injection outright.
    let mut target = volume.to_path_buf();
    if !target.has_root() {
        target.push(Path::new(r"\"));
    }
    for component in relative.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => target.push(name),
            _ => bail!("The native Move target is invalid"),
        }
    }
    ensure!(
        target.is_absolute() && !inside(&target, root.parent().unwrap_or(root)),
        "The native Move target is still inside the clipboard cache"
    );
    Ok(target)
}
fn verify_cached_file(
    work: &Work,
    index: usize,
    path: &Path,
    session: &Arc<dyn PasteSession>,
    original_identity: bool,
) -> Result<()> {
    work.check()?;
    let state = &work.root.state;
    let expected = *state
        .identities
        .lock()
        .unwrap()
        .get(&index)
        .context("Unknown native Move entry")?;
    let size = state.file_size(index)?;
    let (_leaf, leaf_identity) = attributes(path)?;
    ensure!(
        fs::symlink_metadata(path)?.is_file(),
        "The native Move target is not a regular file"
    );
    let mut cached = File::open(path)?;
    let before = identity(&cached)?;
    ensure!(
        before == leaf_identity,
        "The native Move target redirects to a different file"
    );
    ensure!(
        (!original_identity || (before.file == expected.file && before.volume == expected.volume))
            && before.size == size,
        "The native Move cache identity changed; originals were retained"
    );
    let mut pinned = state.source(index, session, &work.cancelled)?;
    let source = &mut pinned.as_mut().unwrap().file;
    source.seek(SeekFrom::Start(0))?;
    let mut left = vec![0u8; BLOCK];
    let mut right = vec![0u8; BLOCK];
    let mut remaining = size;
    while remaining != 0 {
        work.check()?;
        session.wait_unpaused_cancellable(&work.cancelled)?;
        let count = remaining.min(BLOCK as u64) as usize;
        source.read_exact(&mut left[..count])?;
        cached.read_exact(&mut right[..count])?;
        ensure!(
            left[..count] == right[..count],
            "The native Move data changed; originals were retained"
        );
        remaining -= count as u64;
    }
    let mut extra = [0u8];
    ensure!(
        cached.read(&mut extra)? == 0
            && identity(&cached)? == before
            && attributes(path)?.1 == before,
        "The native Move cache changed while verifying; originals were retained"
    );
    ensure!(
        identity(&pinned.as_ref().unwrap().file)? == pinned.as_ref().unwrap().identity,
        "The verified transfer cache changed while verifying the destination"
    );
    Ok(())
}
fn verified_branch(
    work: &Work,
    index: usize,
    destination: Option<&Path>,
    session: &Arc<dyn PasteSession>,
    original_identity: bool,
) -> Result<Vec<usize>> {
    let state = &work.root.state;
    let entries = branch(&state.offer.description, index);
    let source_path = &state.offer.description.entries[index].path;
    for child in &entries {
        work.check()?;
        let descriptor = &state.offer.description.entries[*child];
        let path = match destination {
            Some(target) => joined(target, &descriptor.path[source_path.len()..]),
            None => state.path(&work.root, *child),
        };
        // A successful earlier copy+delete removes children before a containing
        // directory's Delete notification. Only that exact receipt can stand
        // in for a missing file; generic deletion never counts as delivery.
        if destination.is_none() && state.moved.lock().unwrap().contains(child) {
            continue;
        }
        let (_, actual) = attributes(&path)?;
        let expected = *state
            .identities
            .lock()
            .unwrap()
            .get(child)
            .context("Unknown native Move entry")?;
        ensure!(
            !original_identity
                || (actual.file == expected.file && actual.volume == expected.volume),
            "The native Move entry identity changed; originals were retained"
        );
        match descriptor.kind {
            DescriptorKind::Directory => {
                ensure!(
                    fs::symlink_metadata(&path)?.is_dir(),
                    "The native Move folder changed"
                );
            }
            DescriptorKind::File { size } => {
                if destination.is_none() {
                    let generation = state.verified.lock().unwrap().get(child).copied();
                    ensure!(
                        generation == Some(actual) && actual.size == size,
                        "The native Move file was not fully delivered; originals were retained"
                    );
                }
                verify_cached_file(work, *child, &path, session, original_identity)?;
            }
        }
    }
    Ok(entries)
}
struct ExplicitHydration<'a> {
    state: &'a State,
    entries: Vec<usize>,
    consumer: NativeConsumer,
}
impl Drop for ExplicitHydration<'_> {
    fn drop(&mut self) {
        let mut explicit = self.state.explicit.lock().unwrap();
        for entry in &self.entries {
            if explicit.get(entry) == Some(&self.consumer) {
                explicit.remove(entry);
            }
        }
    }
}
fn prepare_move(work: &Work, request: &Request, target: Option<PathBuf>) -> Result<()> {
    let state = &work.root.state;
    let rename = target.is_some();
    ensure!(
        state.offer.operation == Operation::Move
            && !request.self_process
            && !state.tearing_down.load(Ordering::Acquire),
        "This is not an external Cut operation"
    );
    let consumer = request
        .consumer
        .clone()
        .context("The native Move consumer is unknown")?;
    let entries = branch(&state.offer.description, request.index);
    if target.is_none() {
        let reads = state.consumer_reads.lock().unwrap();
        for index in &entries {
            if let DescriptorKind::File { size } = state.offer.description.entries[*index].kind {
                ensure!(
                    size == 0
                        || reads
                            .get(&(consumer.process_id, consumer.started, *index))
                            .is_some_and(|coverage| coverage.covers(size)),
                    "This native consumer did not read the complete file; originals were retained"
                );
            }
        }
    }
    let kind = if target.is_some() {
        CF_OPERATION_TYPE_ACK_RENAME
    } else {
        CF_OPERATION_TYPE_ACK_DELETE
    };
    let session = state.session(
        request,
        target
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
    )?;
    if rename {
        let folder = state
            .sessions
            .lock()
            .unwrap()
            .get(&(consumer.process_id, consumer.started))
            .and_then(|session| session.destination.clone());
        if let (Some(folder), Some(target)) = (folder, target.as_ref()) {
            let mut actual = target.clone();
            for _ in 0..state.offer.description.entries[request.index].path.len() {
                actual.pop();
            }
            if normalized(&actual) != normalized(&folder) {
                state.ambiguous.store(true, Ordering::Release);
                bail!("The native Move destination changed; originals were retained");
            }
        }
    }
    let target = if rename {
        target
    } else {
        let folder = state.sessions.lock().unwrap().get(&(consumer.process_id, consumer.started))
            .and_then(|session| session.destination.clone())
            .context("The actual Move destination is unknown; files may be copied but originals were retained")?;
        Some(joined(
            &folder,
            &state.offer.description.entries[request.index].path,
        ))
    };
    let _keepalive = Keepalive::start(work, request, Some((kind, session.clone())));
    let _move_lock = cancellable_lock(&state.move_lock, &work.cancelled)?;
    work.check()?;
    ensure!(
        !session.is_terminal() && !state.ambiguous.load(Ordering::Acquire),
        "The native Move has ended; originals were retained"
    );
    session.wait_verified_cancellable(&work.cancelled)?;
    if rename {
        // Same-volume Explorer Move can rename a dehydrated placeholder without
        // a single ReadFile. Materialize every child before acknowledging the
        // out-of-root rename, while the shell still waits for this operation.
        let hydration = ExplicitHydration {
            state,
            entries: entries.clone(),
            consumer: consumer.clone(),
        };
        for index in &entries {
            if let DescriptorKind::File { size } = state.offer.description.entries[*index].kind {
                work.check()?;
                state
                    .explicit
                    .lock()
                    .unwrap()
                    .insert(*index, consumer.clone());
                if size != 0 {
                    let path = state.path(&work.root, *index);
                    let (file, current) = attributes(&path)?;
                    let expected = *state
                        .identities
                        .lock()
                        .unwrap()
                        .get(index)
                        .context("Unknown native Move file")?;
                    ensure!(
                        current.file == expected.file && current.volume == expected.volume,
                        "The native Move file identity changed"
                    );
                    unsafe {
                        state.api.hydrate_placeholder(
                            HANDLE(file.as_raw_handle()),
                            0,
                            -1,
                            CF_HYDRATE_FLAG_NONE,
                            None,
                        )
                    }?;
                }
            }
        }
        drop(hydration);
    }
    let entries = verified_branch(work, request.index, None, &session, true)?;
    work.check()?;
    ensure!(
        !session.is_terminal(),
        "The native Move was cancelled; originals were retained"
    );
    let receipt = MoveReceipt {
        index: request.index,
        entries,
        consumer,
        target,
        session,
        rename,
        created: Instant::now(),
    };
    state
        .moves
        .lock()
        .unwrap()
        .insert((request.file, request.key), receipt);
    if let Err(error) = state.ack_action(request, kind, STATUS_OK) {
        state
            .moves
            .lock()
            .unwrap()
            .remove(&(request.file, request.key));
        return Err(error);
    }
    Ok(())
}
fn start_move(root: Arc<Root>, request: Request, target: Option<PathBuf>) -> Result<()> {
    let kind = if target.is_some() {
        CF_OPERATION_TYPE_ACK_RENAME
    } else {
        CF_OPERATION_TYPE_ACK_DELETE
    };
    let work = match Work::start(root.clone(), &request) {
        Ok(work) => work,
        Err(_) => return root.state.ack_action(&request, kind, STATUS_FAILED),
    };
    thread::spawn(move || {
        if let Err(error) = prepare_move(&work, &request, target) {
            let _ = work.root.state.ack_action(&request, kind, STATUS_FAILED);
            if let Some(consumer) = &request.consumer
                && let Some(session) = work
                    .root
                    .state
                    .sessions
                    .lock()
                    .unwrap()
                    .get(&(consumer.process_id, consumer.started))
            {
                let _ = session.handle.native_complete(NativeResult::Failed(format!(
                    "{error}; originals were retained"
                )));
            }
        }
    });
    Ok(())
}
unsafe extern "system" fn rename_callback(
    info: *const CF_CALLBACK_INFO,
    raw: *const CF_CALLBACK_PARAMETERS,
) {
    io_callback(info, CF_OPERATION_TYPE_ACK_RENAME, || {
        let (root, request) = unsafe { snapshot(info) }?;
        let args = unsafe {
            parameters::<CF_CALLBACK_PARAMETERS_0_10>(raw)?
                .Anonymous
                .Rename
        };
        let scope = args.Flags.contains(CF_CALLBACK_RENAME_FLAG_SOURCE_IN_SCOPE)
            && !args.Flags.contains(CF_CALLBACK_RENAME_FLAG_TARGET_IN_SCOPE);
        if request.self_process && root.state.tearing_down.load(Ordering::Acquire) {
            return root
                .state
                .ack_action(&request, CF_OPERATION_TYPE_ACK_RENAME, STATUS_OK);
        }
        if !scope || root.state.offer.operation != Operation::Move {
            return root
                .state
                .ack_action(&request, CF_OPERATION_TYPE_ACK_RENAME, STATUS_FAILED);
        }
        let target = unsafe { bounded_text(args.TargetPath) }
            .and_then(|target| move_target(&request.volume, &target, &root.path));
        match target {
            Ok(target) => start_move(root, request, Some(target)),
            Err(_) => root
                .state
                .ack_action(&request, CF_OPERATION_TYPE_ACK_RENAME, STATUS_FAILED),
        }
    });
}
unsafe extern "system" fn delete_callback(
    info: *const CF_CALLBACK_INFO,
    raw: *const CF_CALLBACK_PARAMETERS,
) {
    io_callback(info, CF_OPERATION_TYPE_ACK_DELETE, || {
        let (root, request) = unsafe { snapshot(info) }?;
        let args = unsafe {
            parameters::<CF_CALLBACK_PARAMETERS_0_8>(raw)?
                .Anonymous
                .Delete
        };
        if request.self_process && root.state.tearing_down.load(Ordering::Acquire) {
            return root
                .state
                .ack_action(&request, CF_OPERATION_TYPE_ACK_DELETE, STATUS_OK);
        }
        if root.state.offer.operation != Operation::Move
            || args.Flags.contains(CF_CALLBACK_DELETE_FLAG_IS_UNDELETE)
        {
            return root
                .state
                .ack_action(&request, CF_OPERATION_TYPE_ACK_DELETE, STATUS_FAILED);
        }
        start_move(root, request, None)
    });
}
fn finish_move(work: &Work, request: &Request, receipt: MoveReceipt) -> Result<()> {
    let state = &work.root.state;
    ensure!(
        request.consumer.as_ref() == Some(&receipt.consumer)
            && !state.ambiguous.load(Ordering::Acquire)
            && !receipt.session.is_terminal(),
        "The native Move completion is ambiguous; originals were retained"
    );
    if let Some(target) = &receipt.target {
        // Completion is a native success receipt. For an out-of-root rename we
        // additionally prove the selected tree now exists at its actual target.
        verified_branch(
            work,
            receipt.index,
            Some(target),
            &receipt.session,
            receipt.rename,
        )?;
    }
    if let Some(target) = &receipt.target {
        let prefix = state.offer.description.entries[receipt.index].path.len();
        let mut destinations = state.destinations.lock().unwrap();
        for index in &receipt.entries {
            destinations.insert(
                *index,
                (
                    joined(
                        target,
                        &state.offer.description.entries[*index].path[prefix..],
                    ),
                    receipt.rename,
                ),
            );
        }
    }
    let mut moved = state.moved.lock().unwrap();
    moved.extend(receipt.entries);
    if moved.len() == state.offer.description.entries.len() {
        let entries = moved.iter().copied().collect();
        drop(moved);
        // Earlier targets can be edited while later files are still copying.
        // Recheck the complete destination batch immediately before the single
        // remote source-cleanup receipt, not just each hydration or deletion.
        let destinations = state.destinations.lock().unwrap().clone();
        for (index, descriptor) in state.offer.description.entries.iter().enumerate() {
            let (path, original_identity) = destinations
                .get(&index)
                .context("A native Move target is unconfirmed")?;
            if matches!(descriptor.kind, DescriptorKind::Directory) {
                ensure!(
                    fs::symlink_metadata(path)?.is_dir(),
                    "A native Move destination folder changed"
                );
            } else {
                verify_cached_file(work, index, path, &receipt.session, *original_identity)?;
            }
        }
        receipt
            .session
            .native_complete(NativeResult::MoveCommitted {
                consumer: receipt.consumer,
                entries,
            })?;
        state.move_done.store(true, Ordering::Release);
    }
    Ok(())
}
fn complete_move(root: Arc<Root>, request: Request, rename: bool) -> Result<()> {
    if request.self_process || root.state.tearing_down.load(Ordering::Acquire) {
        return Ok(());
    }
    let Some(receipt) = root
        .state
        .moves
        .lock()
        .unwrap()
        .remove(&(request.file, request.key))
    else {
        return Ok(());
    };
    if receipt.rename != rename || request.consumer.as_ref() != Some(&receipt.consumer) {
        return Ok(());
    }
    let work = Work::start(root, &request)?;
    thread::spawn(move || {
        let session = receipt.session.clone();
        if let Err(error) = finish_move(&work, &request, receipt) {
            let _ = session.native_complete(NativeResult::Failed(format!(
                "{error}; originals were retained"
            )));
        }
    });
    Ok(())
}
unsafe extern "system" fn rename_complete_callback(
    info: *const CF_CALLBACK_INFO,
    raw: *const CF_CALLBACK_PARAMETERS,
) {
    callback(|| {
        let (root, request) = unsafe { snapshot(info) }?;
        let _ = unsafe { parameters::<CF_CALLBACK_PARAMETERS_0_11>(raw)? };
        complete_move(root, request, true)
    });
}
unsafe extern "system" fn delete_complete_callback(
    info: *const CF_CALLBACK_INFO,
    raw: *const CF_CALLBACK_PARAMETERS,
) {
    callback(|| {
        let (root, request) = unsafe { snapshot(info) }?;
        let _ = unsafe { parameters::<CF_CALLBACK_PARAMETERS_0_9>(raw)? };
        complete_move(root, request, false)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_bind_root_and_entry_without_ambient_paths() {
        let token = [7u8; 16];
        let identity = source_identity(&token, 4);
        assert_eq!(decode_identity(&identity, &token, 5), Some(4));
        assert_eq!(decode_identity(&identity, &token, 4), None);
        assert_eq!(decode_identity(&identity, &[8u8; 16], 5), None);
        assert_eq!(decode_identity(&identity[..23], &token, 5), None);
        let mut changed = identity;
        changed[0] ^= 1;
        assert_eq!(decode_identity(&changed, &token, 5), None);
    }

    #[test]
    fn identical_provider_paths_remain_current_after_owner_or_serial_rewrite() {
        let original = vec![PathBuf::from(r"\\?\C:\Cache\offer-1\Folder")];
        let manager = vec![PathBuf::from(r"c:\cache\offer-1\Folder")];
        assert!(same_selection(
            &original,
            Operation::Copy,
            &manager,
            Operation::Copy
        ));
        assert!(!same_selection(
            &original,
            Operation::Move,
            &manager,
            Operation::Copy
        ));
        assert!(!same_selection(
            &original,
            Operation::Copy,
            &[PathBuf::from(r"C:\Cache\offer-2\Folder")],
            Operation::Copy
        ));
        assert!(!same_selection(
            &original,
            Operation::Copy,
            &[],
            Operation::Copy
        ));
    }

    #[test]
    fn data_ranges_are_aligned_bounded_and_overflow_checked() {
        assert!(valid_range(0, 4096, 8192));
        assert!(valid_range(4096, 13, 4109));
        assert!(!valid_range(-1, 4096, 8192));
        assert!(!valid_range(1, 4096, 8192));
        assert!(!valid_range(0, -1, 8192));
        assert!(!valid_range(0, 0, 8192));
        assert!(!valid_range(4096, 8192, 8192));
        assert!(!valid_range(i64::MAX - 4095, i64::MAX, u64::MAX));
        assert_eq!(requested_range(4096, -1, 8193), Some((4096, 4097)));
        assert_eq!(requested_range(0, -1, 17), Some((0, 17)));
        assert_eq!(requested_range(4096, 4096, 4109), Some((4096, 13)));
        assert_eq!(requested_range(0, -2, 8192), None);
        assert_eq!(requested_range(0, 1, 8192), None);
    }

    #[test]
    fn tiny_reads_do_not_authorize_move_and_disjoint_ranges_merge() {
        let mut coverage = Coverage::default();
        coverage.add(0, 4096, 16_384).unwrap();
        assert!(!coverage.covers(16_384));
        coverage.add(12_288, 4096, 16_384).unwrap();
        assert!(!coverage.covers(16_384));
        coverage.add(8192, 4096, 16_384).unwrap();
        assert!(!coverage.covers(16_384));
        coverage.add(4096, 4096, 16_384).unwrap();
        assert!(coverage.covers(16_384));
        coverage.add(0, 16_384, 16_384).unwrap();
        assert_eq!(coverage.0, [(0, 16_384)]);
        assert!(coverage.add(u64::MAX, 1, u64::MAX).is_err());
    }

    #[test]
    fn consumer_coverage_does_not_transfer_between_processes_or_entries() {
        let mut reads = BTreeMap::new();
        let mut coverage = Coverage::default();
        coverage.add(0, 8192, 8192).unwrap();
        reads.insert((7u32, 100u64, 3usize), coverage);
        assert!(
            reads
                .get(&(7, 100, 3))
                .is_some_and(|coverage| coverage.covers(8192))
        );
        assert!(
            !reads
                .get(&(8, 100, 3))
                .is_some_and(|coverage| coverage.covers(8192))
        );
        assert!(
            !reads
                .get(&(7, 101, 3))
                .is_some_and(|coverage| coverage.covers(8192))
        );
        assert!(
            !reads
                .get(&(7, 100, 4))
                .is_some_and(|coverage| coverage.covers(8192))
        );
    }

    #[test]
    fn partial_range_cancel_keeps_other_native_reads_alive() {
        assert!(!cancelled_range_covers(0, 4096, (0, 8192)));
        assert!(!cancelled_range_covers(4096, 4096, (0, 8192)));
        assert!(cancelled_range_covers(0, 8192, (0, 8192)));
        assert!(cancelled_range_covers(0, -1, (4096, 8192)));
        assert!(!cancelled_range_covers(-1, 8192, (0, 8192)));
    }

    #[test]
    fn completed_moves_release_native_sessions_beyond_root_limit() {
        assert!(retain_native_session(Operation::Move, false, false));
        assert!(retain_native_session(Operation::Move, true, true));
        assert!(!retain_native_session(Operation::Move, false, true));
        let before = LIVE_ROOTS.load(Ordering::Acquire);
        for _ in 0..ROOT_LIMIT * 4 {
            let permit = LivePermit::acquire().unwrap();
            assert!(!retain_native_session(Operation::Move, false, true));
            drop(permit);
        }
        assert_eq!(LIVE_ROOTS.load(Ordering::Acquire), before);
    }

    #[test]
    fn move_targets_require_out_of_root_volume_relative_paths() {
        let root = Path::new(r"C:\cache\offer-1");
        let volume = Path::new("C:");
        assert_eq!(
            move_target(volume, &OsString::from(r"\Users\Name\file.txt"), root).unwrap(),
            PathBuf::from(r"C:\Users\Name\file.txt")
        );
        assert!(move_target(volume, &OsString::from(r"\cache\offer-1\file"), root).is_err());
        assert!(move_target(volume, &OsString::from(r"\Users\..\file"), root).is_err());
        assert!(move_target(volume, &OsString::from(r"D:\file"), root).is_err());
    }
}
