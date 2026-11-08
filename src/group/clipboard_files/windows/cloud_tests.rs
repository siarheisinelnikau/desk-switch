//! Native Cloud Files coverage runs only on an explicitly opted-in disposable
//! GitHub-hosted Windows runner. The consumer is a separate test process because
//! production blocks its own implicit placeholder hydration.

use super::super as native;
use super::*;
use crate::group::file_transfer::hosted_source_move as source_move;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    cell::Cell,
    fs::OpenOptions,
    io::Write,
    process::{Child, Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

const CONSUMER_TEST: &str =
    "group::clipboard_files::windows::cloud::native_tests::isolated_ci_cloud_file_consumer";
const PLAN_ENV: &str = "DESK_SWITCH_CLOUD_ACTOR_PLAN";
const FIXTURE_MARKER: &str = ".desk-switch-cloud-ci-owner";
const WAIT: Duration = Duration::from_secs(35);
const EVENTS: [&str; 10] = [
    "ready",
    "go",
    "done",
    "finish",
    "first-deleted",
    "delete-rest",
    "provider-removed",
    "target-reread",
    "delete-denied",
    "delete-finished",
];

fn require_isolated_ci() {
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    assert_eq!(
        std::env::var("RUNNER_ENVIRONMENT").as_deref(),
        Ok("github-hosted"),
        "Cloud Files tests must never run on a self-hosted or user desktop"
    );
    assert_eq!(
        std::env::var("DESK_SWITCH_NATIVE_CLIPBOARD_TEST").as_deref(),
        Ok("1"),
        "Cloud Files tests require the explicit disposable-CI opt-in"
    );
}

fn wait_for(mut condition: impl FnMut() -> Result<bool>, label: &str) -> Result<()> {
    let deadline = Instant::now() + WAIT;
    loop {
        if condition()? {
            return Ok(());
        }
        ensure!(Instant::now() < deadline, "Timed out waiting for {label}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn clipboard_retry<T>(mut action: impl FnMut() -> Result<T>) -> Result<T> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match action() {
            Ok(value) => return Ok(value),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => thread::sleep(Duration::from_millis(10)),
        }
    }
}

fn ordinary_dos_path(path: &Path) -> PathBuf {
    let text = path.as_os_str().to_string_lossy();
    PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text))
}

fn check_bound(root: &Path, path: &Path) -> Result<()> {
    let root = ordinary_dos_path(root);
    let path = ordinary_dos_path(path);
    ensure!(
        path.is_absolute()
            && path.starts_with(&root)
            && path
                .components()
                .all(|part| !matches!(part, Component::ParentDir | Component::CurDir)),
        "A native test path escaped its fixture: {}",
        path.display()
    );
    let mut existing = path.as_path();
    while !existing.try_exists()? {
        existing = existing.parent().context("Missing fixture path ancestor")?;
    }
    ensure!(
        ordinary_dos_path(&existing.canonicalize()?).starts_with(&root),
        "A native test path resolves outside its fixture"
    );
    Ok(())
}

fn validate_fixture(root: &Path, token: &str) -> Result<()> {
    let temporary = PathBuf::from(
        std::env::var_os("RUNNER_TEMP")
            .context("The hosted runner temporary directory is missing")?,
    )
    .canonicalize()?;
    ensure!(
        root.is_absolute()
            && root.parent() == Some(temporary.as_path())
            && root
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("desk-switch-cloud-ci-"))
            && root.canonicalize()? == root,
        "The native test fixture is not its original temporary directory"
    );
    let metadata = fs::symlink_metadata(root)?;
    ensure!(
        metadata.is_dir() && metadata.file_attributes() & REPARSE == 0,
        "The native test fixture was redirected"
    );
    ensure!(
        fs::read(root.join(FIXTURE_MARKER))? == token.as_bytes(),
        "The native test fixture ownership marker changed"
    );
    Ok(())
}

struct OwnedEntry {
    path: PathBuf,
    id: Identity,
    directory: bool,
}

struct Fixture {
    root: PathBuf,
    token: String,
    owned: Vec<OwnedEntry>,
    sequence: Cell<u64>,
    initial_roots: usize,
    cleaned: bool,
}

impl Fixture {
    fn create() -> Result<Self> {
        require_isolated_ci();
        let token = format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        );
        let temporary = PathBuf::from(
            std::env::var_os("RUNNER_TEMP")
                .context("The hosted runner temporary directory is missing")?,
        )
        .canonicalize()?;
        let root = temporary.join(format!("desk-switch-cloud-ci-{token}"));
        fs::create_dir(&root)?;
        let root = root.canonicalize()?;
        fs::write(root.join(FIXTURE_MARKER), token.as_bytes())?;
        let fixture = Self {
            root,
            token,
            owned: Vec::new(),
            sequence: Cell::new(0),
            initial_roots: LIVE_ROOTS.load(Ordering::Acquire),
            cleaned: false,
        };
        validate_fixture(&fixture.root, &fixture.token)?;
        fixture.log("Created isolated native Cloud Files fixture")?;
        eprintln!("Native Cloud Files artifacts: {}", fixture.root.display());
        Ok(fixture)
    }

    fn log(&self, message: &str) -> Result<()> {
        validate_fixture(&self.root, &self.token)?;
        let mut log = OpenOptions::new()
            .append(true)
            .create(true)
            .open(self.root.join("cloud-native.log"))?;
        writeln!(log, "{message}")?;
        Ok(())
    }

    fn directory(&mut self, relative: &str) -> Result<PathBuf> {
        let path = self.root.join(relative);
        check_bound(&self.root, &path)?;
        fs::create_dir_all(&path)?;
        let mut current = path.as_path();
        while current != self.root {
            self.remember(current)?;
            current = current.parent().context("Invalid fixture directory")?;
        }
        Ok(path)
    }

    fn file(&mut self, relative: &str, bytes: &[u8], retain: bool) -> Result<PathBuf> {
        let path = self.root.join(relative);
        let parent = path.parent().context("Missing fixture file parent")?;
        let relative_parent = parent
            .strip_prefix(&self.root)?
            .to_string_lossy()
            .into_owned();
        if !relative_parent.is_empty() {
            self.directory(&relative_parent)?;
        }
        check_bound(&self.root, &path)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if !retain {
            self.remember(&path)?;
        }
        Ok(path)
    }

    fn remember(&mut self, path: &Path) -> Result<()> {
        check_bound(&self.root, path)?;
        if self.owned.iter().any(|entry| entry.path == path) {
            return Ok(());
        }
        let (_, id) = attributes(path)?;
        self.owned.push(OwnedEntry {
            path: path.to_path_buf(),
            id,
            directory: fs::symlink_metadata(path)?.is_dir(),
        });
        Ok(())
    }

    fn remember_root(&mut self, root: &Root) -> Result<()> {
        self.remember(&root.path)?;
        self.remember(&root.path.join(MARKER))?;
        for entry in &root.state.offer.description.entries {
            self.remember(&joined(&root.path, &entry.path))?;
        }
        Ok(())
    }

    fn remember_rejected_roots(
        &mut self,
        base: &Path,
        description: &protocol::Description,
    ) -> Result<()> {
        for entry in fs::read_dir(base)? {
            let path = entry?.path();
            check_bound(&self.root, &path)?;
            ensure!(
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("offer-"))
            );
            if !path.try_exists()? {
                continue;
            }
            self.remember(&path)?;
            for relative in description.entries.iter().map(|entry| &entry.path) {
                let child = joined(&path, relative);
                if child.try_exists()? {
                    self.remember(&child)?;
                }
            }
            let marker = path.join(MARKER);
            if marker.try_exists()? {
                self.remember(&marker)?;
            }
        }
        Ok(())
    }

    fn remember_sequence(&self) -> Result<u64> {
        let sequence = u64::from(unsafe { native::GetClipboardSequenceNumber() });
        ensure!(
            sequence != 0,
            "The isolated clipboard has no sequence number"
        );
        self.sequence.set(sequence);
        Ok(sequence)
    }

    fn clean(&mut self) -> Result<()> {
        if self.cleaned {
            return Ok(());
        }
        validate_fixture(&self.root, &self.token)?;
        let expected = self.sequence.get();
        if expected != 0 {
            clipboard_retry(|| {
                native::OWNER.with(|slot| -> Result<()> {
                    let slot = slot.borrow();
                    let Some(owner) = slot.as_ref() else {
                        return Ok(());
                    };
                    let _clipboard = native::Clipboard::open(owner.0)?;
                    if unsafe { native::GetClipboardOwner() } == owner.0
                        && u64::from(unsafe { native::GetClipboardSequenceNumber() }) == expected
                    {
                        ensure!(
                            unsafe { native::EmptyClipboard() } != 0,
                            "Could not release fixture clipboard"
                        );
                    }
                    Ok(())
                })
            })?;
        }
        ROOTS.with(|registry| {
            let mut registry = registry.borrow_mut();
            registry
                .roots
                .retain(|root| !root.path.starts_with(&self.root));
            registry.bases.retain(|base| !base.starts_with(&self.root));
        });
        wait_for(
            || Ok(LIVE_ROOTS.load(Ordering::Acquire) <= self.initial_roots),
            "owned Cloud Files connections to disconnect",
        )?;
        self.owned
            .sort_by_key(|entry| std::cmp::Reverse(entry.path.components().count()));
        for entry in &self.owned {
            if !entry.path.try_exists()? {
                continue;
            }
            check_bound(&self.root, &entry.path)?;
            let (_, actual) = attributes(&entry.path)?;
            ensure!(
                actual.volume == entry.id.volume && actual.file == entry.id.file,
                "Refusing to remove a replaced fixture entry: {}",
                entry.path.display()
            );
            if entry.directory {
                // A directory containing retained source fixtures or logs is
                // deliberately left in place; never recursively remove it.
                match fs::remove_dir(&entry.path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {}
                    Err(error) => return Err(error.into()),
                }
            } else {
                fs::remove_file(&entry.path)?;
            }
        }
        self.log("Disconnected owned roots; removed only recorded identities; retained Copy/cancel originals, source journals, and logs")?;
        self.cleaned = true;
        Ok(())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = self.clean() {
            eprintln!("Native Cloud Files fixture cleanup retained entries: {error:#}");
            let _ = self.log(&format!("Cleanup retained entries: {error:#}"));
        }
    }
}

struct Source {
    path: PathBuf,
    id: Identity,
    digest: [u8; 32],
    size: u64,
}

struct SourceAuthority {
    selection: source_move::SourceSelection,
    cut_roots: String,
    manifest: Option<protocol::Manifest>,
    state: PathBuf,
    originals: Vec<PathBuf>,
}

struct Mock {
    root: PathBuf,
    sources: BTreeMap<usize, Source>,
    source_authority: Mutex<Option<SourceAuthority>>,
    begins: AtomicUsize,
    leases: AtomicUsize,
    retired: AtomicBool,
    terminal: AtomicBool,
    paused: Mutex<bool>,
    changed: Condvar,
    waiting: AtomicUsize,
    intents: Mutex<Vec<PasteIntent>>,
    reads: Mutex<Vec<(usize, u64, usize)>>,
    results: Mutex<Vec<NativeResult>>,
}

impl Mock {
    fn check_live(&self) -> Result<()> {
        ensure!(
            !self.terminal.load(Ordering::Acquire),
            "The fixture paste was cancelled"
        );
        Ok(())
    }

    fn release(&self) {
        *self.paused.lock().unwrap() = false;
        self.changed.notify_all();
    }

    fn wait_gate(&self, cancel: &AtomicBool) -> Result<()> {
        struct Waiting<'a>(&'a AtomicUsize);
        impl Drop for Waiting<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::AcqRel);
            }
        }
        self.waiting.fetch_add(1, Ordering::AcqRel);
        let _waiting = Waiting(&self.waiting);
        let mut paused = self.paused.lock().unwrap();
        let deadline = Instant::now() + WAIT;
        loop {
            ensure!(
                !cancel.load(Ordering::Acquire),
                "The native fixture range was cancelled"
            );
            self.check_live()?;
            if !*paused {
                return Ok(());
            }
            ensure!(Instant::now() < deadline, "The mock read gate timed out");
            // Native CancelFetch changes its AtomicBool without owning this
            // mock's Condvar. A short bounded wait must still notice it.
            paused = self
                .changed
                .wait_timeout(paused, Duration::from_millis(20))
                .unwrap()
                .0;
        }
    }

    fn verify_originals(&self) -> Result<()> {
        for source in self.sources.values() {
            check_bound(&self.root, &source.path)?;
            let (_, actual) = attributes(&source.path)?;
            ensure!(
                actual == source.id,
                "A mock original changed or was deleted"
            );
            ensure!(<[u8; 32]>::from(Sha256::digest(fs::read(&source.path)?)) == source.digest);
        }
        Ok(())
    }

    fn prepare_move_manifest(&self) -> Result<()> {
        let mut authority = self.source_authority.lock().unwrap();
        let Some(authority) = authority.as_mut() else {
            return Ok(());
        };
        if authority.manifest.is_some() {
            return Ok(());
        }
        let cancel = AtomicBool::new(false);
        let export = authority.selection.prepare(
            &authority.state,
            &"44".repeat(16),
            protocol::MAX_TOTAL,
            &cancel,
            |_| {},
        )?;
        ensure!(export.manifest.description() == authority.selection.description);
        for (index, entry) in export.manifest.entries.iter().enumerate() {
            if let protocol::Kind::File { size, sha256 } = &entry.kind {
                let source = self
                    .sources
                    .get(&index)
                    .context("The bound manifest index changed")?;
                ensure!(
                    *size == source.size && *sha256 == crate::config::hex(&source.digest),
                    "The source cleanup manifest differs from provider bytes"
                );
            }
        }
        authority.manifest = Some(export.manifest.clone());
        Ok(())
    }

    fn finalize_sources(&self, fixture: &Fixture) -> Result<()> {
        ensure!(
            self.results
                .lock()
                .unwrap()
                .iter()
                .any(|result| matches!(result, NativeResult::MoveCommitted { .. })),
            "Source cleanup requires a real full-batch native receipt"
        );
        self.verify_originals()?;
        let SourceAuthority {
            selection,
            cut_roots,
            manifest,
            state,
            originals,
        } = self
            .source_authority
            .lock()
            .unwrap()
            .take()
            .context("No captured source Cut authority")?;
        let manifest = manifest.context("Native demand did not prepare a bound source manifest")?;
        let binding = source_move::SourceMoveBinding {
            workspace: "hosted-native-fixture".into(),
            peer: "11".repeat(32),
            offer: "22".repeat(16),
            paste: "33".repeat(16),
            export: manifest.id.clone(),
            manifest_digest: manifest.digest()?,
            cut_grant: "55".repeat(16),
            cut_roots,
        };
        let outcome = source_move::with_hosted_native_trash(&fixture.root, || {
            selection.finalize_move(&state, &manifest, &binding)
        })??;
        ensure!(
            outcome.state == source_move::SourceMoveState::Moved,
            "The native source cleanup did not reach Moved: {outcome:?}"
        );
        ensure!(outcome.locations.len() == originals.len());
        for original in &originals {
            check_bound(&fixture.root, original)?;
            ensure!(
                !original.try_exists()?,
                "A committed source original was not recycled"
            );
        }
        for location in &outcome.locations {
            let item = location
                .recovery
                .as_ref()
                .context("A moved source lacks a Recycle Bin receipt")?;
            ensure!(
                item.try_exists()?,
                "The native Recycle Bin receipt item is missing"
            );
            for source in self.sources.values() {
                let Ok(relative) = source.path.strip_prefix(&location.original) else {
                    continue;
                };
                let recycled = if relative.as_os_str().is_empty() {
                    item.clone()
                } else {
                    item.join(relative)
                };
                let bytes = fs::read(&recycled)?;
                ensure!(
                    bytes.len() as u64 == source.size
                        && <[u8; 32]>::from(Sha256::digest(&bytes)) == source.digest,
                    "The native Recycle Bin receipt bytes differ from the verified source"
                );
            }
        }
        fixture.log(&format!(
            "Real source cleanup after native Move: {}",
            serde_json::to_string(&outcome)?
        ))?;
        Ok(())
    }

    fn assert_committed(&self, entries: usize, consumer: u32) -> Result<bool> {
        let results = self.results.lock().unwrap();
        let commits = results
            .iter()
            .filter_map(|result| {
                if let NativeResult::MoveCommitted { consumer, entries } = result {
                    Some((consumer, entries))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        ensure!(
            commits.len() <= 1,
            "The native Move was committed more than once"
        );
        if let Some((receipt, actual)) = commits.first() {
            ensure!(receipt.process_id == consumer && receipt.started != 0);
            ensure!(
                actual.as_slice() == (0..entries).collect::<Vec<_>>().as_slice(),
                "The Move receipt omitted part of its batch"
            );
            return Ok(true);
        }
        for result in results.iter() {
            if let NativeResult::Failed(message) = result {
                bail!("The fixture native Move failed: {message}");
            }
        }
        Ok(false)
    }
}

struct MockProvider {
    shared: Arc<Mock>,
    pinned: bool,
}

impl DemandProvider for MockProvider {
    fn lease(&self) -> Result<Arc<dyn DemandProvider>> {
        ensure!(
            self.pinned || !self.shared.retired.load(Ordering::Acquire),
            "The fixture offer was retired"
        );
        self.shared.leases.fetch_add(1, Ordering::AcqRel);
        Ok(Arc::new(Self {
            shared: self.shared.clone(),
            pinned: true,
        }))
    }

    fn begin(&self, intent: PasteIntent) -> Result<Arc<dyn PasteSession>> {
        ensure!(
            self.pinned || !self.shared.retired.load(Ordering::Acquire),
            "An unleased retired fixture was started"
        );
        self.shared.check_live()?;
        self.shared.begins.fetch_add(1, Ordering::AcqRel);
        self.shared.intents.lock().unwrap().push(intent);
        // Preparing the real source manifest is demand-only. Metadata capture
        // before publication did not read or spool any source file contents.
        self.shared.prepare_move_manifest()?;
        Ok(Arc::new(MockSession {
            shared: self.shared.clone(),
        }))
    }
}

struct MockSession {
    shared: Arc<Mock>,
}

impl PasteSession for MockSession {
    fn id(&self) -> &str {
        "isolated-ci-verified-session"
    }

    fn open_verified(&self, index: usize) -> Result<File> {
        self.open_verified_cancellable(index, &AtomicBool::new(false))
    }

    fn open_verified_cancellable(&self, index: usize, cancel: &AtomicBool) -> Result<File> {
        self.shared.wait_gate(cancel)?;
        self.shared.check_live()?;
        let source = self
            .shared
            .sources
            .get(&index)
            .context("The mock entry is not a file")?;
        check_bound(&self.shared.root, &source.path)?;
        let mut file = File::open(&source.path)?;
        ensure!(
            identity(&file)? == source.id,
            "The mock verified storage identity changed"
        );
        let mut bytes = Vec::new();
        let mut block = [0u8; 8192];
        loop {
            ensure!(
                !cancel.load(Ordering::Acquire),
                "The native fixture range was cancelled"
            );
            let count = file.read(&mut block)?;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&block[..count]);
        }
        ensure!(
            bytes.len() as u64 == source.size
                && <[u8; 32]>::from(Sha256::digest(&bytes)) == source.digest,
            "The mock verified storage digest changed"
        );
        ensure!(identity(&file)? == source.id);
        file.seek(SeekFrom::Start(0))?;
        Ok(file)
    }

    fn wait_verified(&self) -> Result<()> {
        self.shared.check_live()
    }

    fn wait_verified_cancellable(&self, cancel: &AtomicBool) -> Result<()> {
        self.shared.wait_gate(cancel)
    }

    fn wait_unpaused(&self) -> Result<()> {
        self.shared.wait_gate(&AtomicBool::new(false))
    }

    fn wait_unpaused_cancellable(&self, cancel: &AtomicBool) -> Result<()> {
        self.shared.wait_gate(cancel)
    }

    fn report_read(&self, index: usize, offset: u64, count: usize) -> Result<()> {
        self.shared.check_live()?;
        let source = self
            .shared
            .sources
            .get(&index)
            .context("Unknown mock source")?;
        ensure!(
            count != 0
                && offset
                    .checked_add(count as u64)
                    .is_some_and(|end| end <= source.size)
        );
        self.shared
            .reads
            .lock()
            .unwrap()
            .push((index, offset, count));
        Ok(())
    }

    fn is_terminal(&self) -> bool {
        self.shared.terminal.load(Ordering::Acquire)
    }

    fn can_retry_move(&self) -> bool {
        !self
            .shared
            .results
            .lock()
            .unwrap()
            .iter()
            .any(|result| matches!(result, NativeResult::MoveCommitted { .. }))
    }

    fn native_complete(&self, result: NativeResult) -> Result<()> {
        let terminal = matches!(
            result,
            NativeResult::MoveCommitted { .. } | NativeResult::Cancelled | NativeResult::Failed(_)
        );
        self.shared.results.lock().unwrap().push(result);
        if terminal {
            self.shared.terminal.store(true, Ordering::Release);
        }
        self.shared.changed.notify_all();
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct ActorFile {
    path: Vec<String>,
    size: u64,
    digest: [u8; 32],
}

#[derive(Serialize, Deserialize)]
struct ActorPlan {
    root: PathBuf,
    token: String,
    mode: String,
    source_base: PathBuf,
    selected: Vec<PathBuf>,
    destination: PathBuf,
    control: PathBuf,
    files: Vec<ActorFile>,
    directories: Vec<Vec<String>>,
}

enum Shape {
    File,
    Tree,
    Pair,
}

fn make_offer(
    fixture: &mut Fixture,
    name: &str,
    operation: Operation,
    shape: Shape,
    paused: bool,
) -> Result<(RemoteOffer, Arc<Mock>, Vec<ActorFile>)> {
    let specs: Vec<(Vec<&str>, Option<usize>)> = match shape {
        Shape::File => vec![(vec!["paste file.bin"], Some(BLOCK + 123))],
        Shape::Pair => vec![
            (vec!["first.bin"], Some(BLOCK + 123)),
            (vec!["second.bin"], Some(BLOCK / 2 + 31)),
        ],
        Shape::Tree => vec![
            (vec!["Cut Folder"], None),
            (vec!["Cut Folder", "first file.bin"], Some(BLOCK + 123)),
            (vec!["Cut Folder", "empty folder"], None),
            (vec!["Cut Folder", "nested"], None),
            (
                vec!["Cut Folder", "nested", "second — пример.txt"],
                Some(9137),
            ),
            (vec!["Cut Folder", "zero.txt"], Some(0)),
        ],
    };
    let original_base = fixture.directory(&format!("sources/{name}"))?;
    let mut root_names = Vec::new();
    let mut created = BTreeMap::new();
    for (index, (components, size)) in specs.into_iter().enumerate() {
        let path = components
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if path.len() == 1 {
            root_names.push(path[0].clone());
        }
        let relative = joined(&PathBuf::from("sources").join(name), &path);
        if let Some(size) = size {
            let bytes = (0..size)
                .map(|offset| ((offset * 37 + index * 53 + name.len()) % 251) as u8)
                .collect::<Vec<_>>();
            let digest = Sha256::digest(&bytes).into();
            // Reads and the mock receipt recorder never remove originals. The
            // successful-Move test separately invokes real, bound source cleanup.
            let source = fixture.file(&relative.to_string_lossy(), &bytes, true)?;
            let (_, id) = attributes(&source)?;
            created.insert(
                path,
                Source {
                    path: source,
                    id,
                    digest,
                    size: size as u64,
                },
            );
        } else {
            fixture.directory(&relative.to_string_lossy())?;
        }
    }
    let originals = root_names
        .iter()
        .map(|name| original_base.join(name))
        .collect::<Vec<_>>();
    let selection = source_move::SourceSelection::capture(&originals, &AtomicBool::new(false))?;
    let description = selection.description.clone();
    let mut sources = BTreeMap::new();
    let mut files = Vec::new();
    for (index, entry) in description.entries.iter().enumerate() {
        if let DescriptorKind::File { size } = entry.kind {
            let source = created
                .remove(&entry.path)
                .context("Source metadata changed its selected path")?;
            ensure!(source.size == size);
            files.push(ActorFile {
                path: entry.path.clone(),
                size,
                digest: source.digest,
            });
            sources.insert(index, source);
        }
    }
    ensure!(created.is_empty());
    let source_authority = if operation == Operation::Move {
        let cut_roots = selection.cut_root_digest()?;
        Some(SourceAuthority {
            selection,
            cut_roots,
            manifest: None,
            state: fixture.root.join(format!("source-state-{name}")),
            originals,
        })
    } else {
        None
    };
    let shared = Arc::new(Mock {
        root: fixture.root.clone(),
        sources,
        source_authority: Mutex::new(source_authority),
        begins: AtomicUsize::new(0),
        leases: AtomicUsize::new(0),
        retired: AtomicBool::new(false),
        terminal: AtomicBool::new(false),
        paused: Mutex::new(paused),
        changed: Condvar::new(),
        waiting: AtomicUsize::new(0),
        intents: Mutex::new(Vec::new()),
        reads: Mutex::new(Vec::new()),
        results: Mutex::new(Vec::new()),
    });
    let offer = RemoteOffer {
        id: format!("isolated-ci-{name}"),
        description,
        operation,
        provider: Arc::new(MockProvider {
            shared: shared.clone(),
            pinned: false,
        }),
    };
    offer.description.validate()?;
    Ok((offer, shared, files))
}

fn raw_clipboard_paths() -> Result<Vec<PathBuf>> {
    let _clipboard = native::Clipboard::open(ptr::null_mut())?;
    let memory = unsafe { native::GetClipboardData(native::CF_HDROP) };
    ensure!(!memory.is_null(), "The real CF_HDROP fixture is missing");
    let size = unsafe { native::GlobalSize(memory) };
    ensure!((native::DROP_HEADER + 2..=native::MAX_METADATA).contains(&size));
    let (_lock, data) = native::Locked::new(memory)?;
    native::decode_drop(unsafe { std::slice::from_raw_parts(data, size) })
}

fn publish_fixture(
    fixture: &mut Fixture,
    name: &str,
    offer: RemoteOffer,
    shared: &Mock,
) -> Result<(Box<dyn PublishedOffer>, Arc<Root>)> {
    let seed = fixture.file(
        &format!("local/{name}.txt"),
        b"A local selection replaced only in disposable CI",
        false,
    )?;
    clipboard_retry(|| native::write(std::slice::from_ref(&seed)))?;
    let expected = fixture.remember_sequence()?;
    let base = fixture.directory(&format!("cache/{name}"))?;
    let binding = publish_at(offer.clone(), expected, &base)?
        .context("The initial isolated publication unexpectedly lost its clipboard serial")?;
    fixture.remember_sequence()?;
    let root = ROOTS
        .with(|registry| {
            registry
                .borrow()
                .roots
                .iter()
                .find(|root| root.state.offer.id == offer.id)
                .cloned()
        })
        .context("The published native root was not retained")?;
    fixture.remember_root(&root)?;
    ensure!(binding.is_current()?);
    ensure!(clipboard_retry(raw_clipboard_paths)? == binding.local_paths());
    let selected =
        clipboard_retry(native::read)?.context("The real file clipboard selection is missing")?;
    ensure!(selected.paths.is_empty() && selected.operation == offer.operation);
    ensure!(Some(selected.sequence) == binding.sequence());
    for entry in &offer.description.entries {
        let path = joined(&root.path, &entry.path);
        check_bound(&fixture.root, &path)?;
        let metadata = fs::symlink_metadata(&path)?;
        match entry.kind {
            DescriptorKind::Directory => {
                ensure!(metadata.is_dir());
                let _ = fs::read_dir(&path)?.collect::<std::io::Result<Vec<_>>>()?;
            }
            DescriptorKind::File { size } => ensure!(metadata.is_file() && metadata.len() == size),
        }
    }
    let previous = binding
        .sequence()
        .context("Missing pre-rewrite clipboard serial")?;
    clipboard_retry(|| native::publish_paths(&binding.local_paths(), None, offer.operation))?
        .context("The identical metadata rewrite was not published")?;
    let rewritten = fixture.remember_sequence()?;
    ensure!(
        rewritten != previous && binding.is_current()?,
        "An identical CF_HDROP rewrite invalidated its metadata binding"
    );
    ensure!(binding.sequence() == Some(rewritten));
    ensure!(
        shared.begins.load(Ordering::Acquire) == 0,
        "Metadata or CF_HDROP started a remote paste"
    );
    if let Some(authority) = shared.source_authority.lock().unwrap().as_ref() {
        ensure!(
            authority.manifest.is_none() && !authority.state.try_exists()?,
            "Metadata prepared source transfer storage"
        );
    }
    fixture.log(&format!("{name}: created placeholders, published and read real CF_HDROP, enumerated metadata; begin=0"))?;
    Ok((binding, root))
}

fn event_path(control: &Path, name: &str) -> Result<PathBuf> {
    ensure!(EVENTS.contains(&name), "Unknown native actor event");
    Ok(control.join(name))
}

fn signal(root: &Path, control: &Path, name: &str) -> Result<()> {
    let path = event_path(control, name)?;
    check_bound(root, &path)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(b"isolated native test event\n")?;
    Ok(())
}

struct Actor {
    child: Child,
    root: PathBuf,
    control: PathBuf,
    logs: PathBuf,
}

impl Actor {
    fn spawn(fixture: &Fixture, plan: &ActorPlan) -> Result<Self> {
        validate_fixture(&fixture.root, &fixture.token)?;
        let logs = fixture.root.join(format!("consumer-{}", plan.mode));
        let output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(logs.with_extension("stdout.log"))?;
        let errors = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(logs.with_extension("stderr.log"))?;
        let child = Command::new(std::env::current_exe()?)
            .args([
                "--ignored",
                "--exact",
                CONSUMER_TEST,
                "--nocapture",
                "--test-threads=1",
            ])
            .env(PLAN_ENV, serde_json::to_string(plan)?)
            .stdout(Stdio::from(output))
            .stderr(Stdio::from(errors))
            .spawn()?;
        Ok(Self {
            child,
            root: fixture.root.clone(),
            control: plan.control.clone(),
            logs,
        })
    }

    fn wait_event(&mut self, event: &str) -> Result<()> {
        let path = event_path(&self.control, event)?;
        check_bound(&self.root, &path)?;
        let logs = self.logs.clone();
        wait_for(
            || {
                if path.try_exists()? {
                    return Ok(true);
                }
                if let Some(status) = self.child.try_wait()? {
                    bail!(
                        "Native consumer exited before {event}: {status}; logs: {}",
                        logs.display()
                    );
                }
                Ok(false)
            },
            event,
        )
    }

    fn signal(&self, event: &str) -> Result<()> {
        signal(&self.root, &self.control, event)
    }

    fn finish(mut self, fixture: &mut Fixture) -> Result<()> {
        self.signal("finish")?;
        wait_for(
            || {
                let Some(status) = self.child.try_wait()? else {
                    return Ok(false);
                };
                ensure!(
                    status.success(),
                    "Native consumer failed: {status}; logs: {}",
                    self.logs.display()
                );
                Ok(true)
            },
            "native consumer exit",
        )?;
        for event in EVENTS {
            let path = event_path(&self.control, event)?;
            if path.try_exists()? {
                fixture.remember(&path)?;
            }
        }
        Ok(())
    }

    fn abort_pending_read(mut self, fixture: &mut Fixture) -> Result<()> {
        self.child.kill()?;
        wait_for(
            || Ok(self.child.try_wait()?.is_some()),
            "the cancelled native consumer to exit",
        )?;
        for event in EVENTS {
            let path = event_path(&self.control, event)?;
            if path.try_exists()? {
                fixture.remember(&path)?;
            }
        }
        Ok(())
    }
}

impl Drop for Actor {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn actor_plan(
    fixture: &mut Fixture,
    mode: &str,
    root: &Root,
    files: Vec<ActorFile>,
) -> Result<ActorPlan> {
    let control = fixture.directory(&format!("controls/{mode}"))?;
    let destination_parent = fixture.directory(&format!("destinations/{mode}"))?;
    let destination = if mode == "rename-tree" {
        destination_parent.join("Committed Folder")
    } else {
        destination_parent
    };
    check_bound(&fixture.root, &destination)?;
    Ok(ActorPlan {
        root: fixture.root.clone(),
        token: fixture.token.clone(),
        mode: mode.to_owned(),
        source_base: root.path.clone(),
        selected: root.paths(),
        destination,
        control,
        files,
        directories: root
            .state
            .offer
            .description
            .entries
            .iter()
            .filter_map(|entry| {
                matches!(entry.kind, DescriptorKind::Directory).then_some(entry.path.clone())
            })
            .collect(),
    })
}

fn verify_actor_bytes(path: &Path, expected: &ActorFile) -> Result<()> {
    let bytes = fs::read(path)?;
    ensure!(
        bytes.len() as u64 == expected.size
            && <[u8; 32]>::from(Sha256::digest(&bytes)) == expected.digest,
        "Native destination bytes differ: {}",
        path.display()
    );
    Ok(())
}

fn verify_move_destination(plan: &ActorPlan) -> Result<()> {
    for file in &plan.files {
        let path = if plan.mode == "rename-tree" {
            joined(&plan.destination, &file.path[1..])
        } else {
            plan.destination
                .join(file.path.last().context("Missing native file name")?)
        };
        check_bound(&plan.root, &path)?;
        verify_actor_bytes(&path, file)?;
    }
    Ok(())
}

fn run_held_read(fixture: &mut Fixture, cancel: bool) -> Result<()> {
    let name = if cancel {
        "cancel-read"
    } else {
        "retired-read"
    };
    let (offer, shared, files) = make_offer(fixture, name, Operation::Copy, Shape::File, true)?;
    let (binding, root) = publish_fixture(fixture, name, offer.clone(), &shared)?;
    let stale_sequence = binding.sequence().context("Missing publication sequence")?;
    let plan = actor_plan(fixture, name, &root, files)?;
    let mut actor = Actor::spawn(fixture, &plan)?;
    actor.wait_event("ready")?;
    wait_for(
        || Ok(!root.state.opened.lock().unwrap().is_empty()),
        "a metadata-only native handle lease",
    )?;
    ensure!(
        shared.leases.load(Ordering::Acquire) != 0 && shared.begins.load(Ordering::Acquire) == 0
    );

    let newer = fixture.file(
        &format!("local/{name}-newer.txt"),
        b"A newer local clipboard copy",
        false,
    )?;
    clipboard_retry(|| native::write(std::slice::from_ref(&newer)))?;
    let newer_sequence = fixture.remember_sequence()?;
    ensure!(newer_sequence != stale_sequence && !binding.is_current()?);
    drop(binding);
    shared.retired.store(true, Ordering::Release);
    ensure!(!root.state.accepting.load(Ordering::Acquire));
    ensure!(
        offer.provider.lease().is_err(),
        "A retired unpinned provider still accepted a new lease"
    );
    if !cancel {
        let (stale, stale_provider, _) =
            make_offer(fixture, "stale", Operation::Copy, Shape::File, false)?;
        let base = fixture.directory("cache/stale")?;
        ensure!(
            publish_at(stale.clone(), stale_sequence, &base)?.is_none(),
            "A stale remote publication replaced a newer local copy"
        );
        fixture.remember_rejected_roots(&base, &stale.description)?;
        ensure!(stale_provider.begins.load(Ordering::Acquire) == 0);
        let selection =
            clipboard_retry(native::read)?.context("The newer local copy disappeared")?;
        ensure!(
            selection.paths == [ordinary_dos_path(&newer)] && selection.sequence == newer_sequence
        );
        ensure!(u64::from(unsafe { native::GetClipboardSequenceNumber() }) == newer_sequence);
    }

    ensure!(shared.begins.load(Ordering::Acquire) == 0);
    actor.signal("go")?;
    wait_for(
        || {
            Ok(shared.begins.load(Ordering::Acquire) == 1
                && shared.waiting.load(Ordering::Acquire) != 0)
        },
        "actual native Read to reach the provider read gate",
    )?;
    ensure!(shared.reads.lock().unwrap().is_empty());
    ensure!(
        !event_path(&plan.control, "done")?.try_exists()?,
        "The native read bypassed the paused provider"
    );
    if cancel {
        shared.terminal.store(true, Ordering::Release);
    }
    shared.release();
    actor.wait_event("done")?;
    ensure!(
        shared.begins.load(Ordering::Acquire) == 1,
        "One native consumer started multiple paste sessions"
    );
    if cancel {
        ensure!(shared.reads.lock().unwrap().is_empty());
    } else {
        ensure!(!shared.reads.lock().unwrap().is_empty());
        fixture.remember(&plan.destination.join("copied.bin"))?;
    }
    ensure!(
        !shared
            .results
            .lock()
            .unwrap()
            .iter()
            .any(|result| matches!(result, NativeResult::MoveCommitted { .. }))
    );
    let intents = shared.intents.lock().unwrap();
    ensure!(
        intents.len() == 1
            && intents[0].operation == Operation::Copy
            && intents[0].completion == CompletionSupport::Unavailable
    );
    ensure!(
        intents[0]
            .consumer
            .as_ref()
            .is_some_and(|consumer| consumer.process_id == actor.child.id())
    );
    drop(intents);
    actor.finish(fixture)?;
    shared.verify_originals()?;
    fixture.log(&format!("{name}: external handle pinned before retirement; metadata begin=0; real read begin=1; paused read {}; originals retained", if cancel { "cancelled" } else { "resumed and matched source digest" }))?;
    Ok(())
}

fn run_move(fixture: &mut Fixture, rename: bool) -> Result<()> {
    let name = if rename { "rename-tree" } else { "copy-delete" };
    let shape = if rename { Shape::Tree } else { Shape::Pair };
    let (offer, shared, files) = make_offer(fixture, name, Operation::Move, shape, false)?;
    let (binding, root) = publish_fixture(fixture, name, offer.clone(), &shared)?;
    let plan = actor_plan(fixture, name, &root, files)?;
    let mut actor = Actor::spawn(fixture, &plan)?;
    if !rename {
        // A CLI filesystem consumer has no Explorer folder view. This is an
        // explicitly injected, PID-bound test capability for the real fixture
        // destination; it is not evidence of Explorer's automatic capture.
        *root.state.test_destination.lock().unwrap() =
            Some((actor.child.id(), plan.destination.clone()));
    }
    actor.wait_event("ready")?;
    ensure!(shared.begins.load(Ordering::Acquire) == 0 && shared.reads.lock().unwrap().is_empty());
    actor.signal("go")?;
    if !rename {
        actor.wait_event("first-deleted")?;
        ensure!(
            !shared.assert_committed(offer.description.entries.len(), actor.child.id())?,
            "A partial native batch was committed"
        );
        actor.signal("delete-rest")?;
    }
    actor.wait_event("done")?;
    wait_for(
        || shared.assert_committed(offer.description.entries.len(), actor.child.id()),
        "a full native Move completion receipt",
    )?;
    ensure!(shared.begins.load(Ordering::Acquire) == 1);
    let intents = shared.intents.lock().unwrap();
    ensure!(
        intents.len() == 1
            && intents[0].operation == Operation::Move
            && intents[0].completion == CompletionSupport::Reported
    );
    if rename {
        ensure!(intents[0].destination.is_some());
    }
    drop(intents);
    if rename {
        fixture.remember(&plan.destination)?;
        for components in &plan.directories {
            let path = joined(&plan.destination, &components[1..]);
            fixture.remember(&path)?;
        }
        for file in &plan.files {
            let path = joined(&plan.destination, &file.path[1..]);
            verify_actor_bytes(&path, file)?;
            fixture.remember(&path)?;
        }
    } else {
        for file in &plan.files {
            let path = plan
                .destination
                .join(file.path.last().context("Missing native file name")?);
            verify_actor_bytes(&path, file)?;
            fixture.remember(&path)?;
        }
    }
    for path in &plan.selected {
        ensure!(
            !path.try_exists()?,
            "Native Move left its source placeholder behind"
        );
    }
    shared.verify_originals()?;
    let removed_root = root.path.clone();
    // For these successful Moves the root contains only its ownership marker.
    // Cleanup removes that directory only after successful disconnect and
    // unregister; disappearance is stronger evidence than a dropped Rust Arc.
    drop(binding);
    ROOTS.with(|registry| {
        registry
            .borrow_mut()
            .roots
            .retain(|registered| !Arc::ptr_eq(registered, &root));
    });
    drop(root);
    wait_for(
        || Ok(!removed_root.try_exists()?),
        "the Move provider to disconnect, unregister, and remove its empty root",
    )?;
    actor.signal("provider-removed")?;
    actor.wait_event("target-reread")?;
    verify_move_destination(&plan)?;
    actor.finish(fixture)?;
    shared.finalize_sources(fixture)?;
    fixture.log(&format!("{name}: native filesystem operation committed all {} entries once; begin=1; destination re-read after provider disconnect/unregister; bound source cleanup returned actual native Recycle Bin receipts", offer.description.entries.len()))?;
    Ok(())
}

fn run_native_range_cancel(fixture: &mut Fixture) -> Result<()> {
    let (offer, shared, files) =
        make_offer(fixture, "native-cancel", Operation::Copy, Shape::File, true)?;
    let (binding, root) = publish_fixture(fixture, "native-cancel", offer, &shared)?;
    let plan = actor_plan(fixture, "native-cancel", &root, files.clone())?;
    let mut actor = Actor::spawn(fixture, &plan)?;
    actor.wait_event("ready")?;
    ensure!(shared.begins.load(Ordering::Acquire) == 0);
    actor.signal("go")?;
    wait_for(
        || {
            Ok(shared.begins.load(Ordering::Acquire) == 1
                && shared.waiting.load(Ordering::Acquire) != 0)
        },
        "a native read paused inside its cancellable provider",
    )?;
    let cancellation = root
        .state
        .requests
        .lock()
        .unwrap()
        .values()
        .next()
        .map(|request| request.cancelled.clone())
        .context("The pending native range has no cancellation token")?;
    let retry_plan = actor_plan(fixture, "native-retry", &root, files)?;
    let mut retry = Actor::spawn(fixture, &retry_plan)?;
    retry.wait_event("ready")?;
    ensure!(shared.begins.load(Ordering::Acquire) == 1);
    retry.signal("go")?;
    wait_for(
        || Ok(shared.begins.load(Ordering::Acquire) == 2),
        "a second consumer to hold an independent pending native request",
    )?;
    actor.abort_pending_read(fixture)?;
    // Do not release the pause gate and do not cancel the shared mock session.
    // Native I/O cancellation must free this request's worker and file lock.
    wait_for(
        || {
            Ok(cancellation.load(Ordering::Acquire)
                && !root
                    .state
                    .requests
                    .lock()
                    .unwrap()
                    .values()
                    .any(|request| Arc::ptr_eq(&request.cancelled, &cancellation))
                && shared.waiting.load(Ordering::Acquire) != 0)
        },
        "native cancellation to release a paused Work slot",
    )?;
    ensure!(*shared.paused.lock().unwrap() && !shared.terminal.load(Ordering::Acquire));
    ensure!(shared.reads.lock().unwrap().is_empty());
    shared.verify_originals()?;
    shared.release();
    retry.wait_event("done")?;
    ensure!(shared.begins.load(Ordering::Acquire) == 2 && !shared.terminal.load(Ordering::Acquire));
    fixture.remember(&retry_plan.destination.join("copied.bin"))?;
    retry.finish(fixture)?;
    shared.verify_originals()?;
    drop(binding);
    fixture.log("native-cancel: terminated one actual blocked external read while two consumers were pending; its request and file lock released while the other paused consumer stayed live and subsequently read matching bytes")?;
    Ok(())
}

fn run_tiny_read_delete(fixture: &mut Fixture) -> Result<()> {
    let (offer, shared, files) = make_offer(
        fixture,
        "partial-delete",
        Operation::Move,
        Shape::File,
        false,
    )?;
    // This negative case has no source-cleanup grant. A content read or failed
    // native Delete may not prepare or consume source cleanup authority.
    drop(shared.source_authority.lock().unwrap().take());
    let (binding, root) = publish_fixture(fixture, "partial-delete", offer, &shared)?;
    let plan = actor_plan(fixture, "partial-delete", &root, files)?;
    let mut actor = Actor::spawn(fixture, &plan)?;
    *root.state.test_destination.lock().unwrap() =
        Some((actor.child.id(), plan.destination.clone()));
    actor.wait_event("ready")?;
    ensure!(shared.begins.load(Ordering::Acquire) == 0);
    actor.signal("go")?;
    actor.wait_event("done")?;
    ensure!(shared.begins.load(Ordering::Acquire) == 1 && !shared.reads.lock().unwrap().is_empty());
    wait_for(
        || {
            Ok(shared
                .results
                .lock()
                .unwrap()
                .iter()
                .any(|result| matches!(result, NativeResult::Failed(_))))
        },
        "the native Delete callback to report its rejected destination",
    )?;
    ensure!(
        !shared
            .results
            .lock()
            .unwrap()
            .iter()
            .any(|result| matches!(result, NativeResult::MoveCommitted { .. })),
        "A tiny read and failed Delete authorized source cleanup"
    );
    ensure!(
        event_path(&plan.control, "delete-denied")?.try_exists()?
            || event_path(&plan.control, "delete-finished")?.try_exists()?
    );
    shared.verify_originals()?;
    actor.finish(fixture)?;
    drop(binding);
    fixture.log("partial-delete: one-byte external read plus cache Delete with no committed destination produced Failed and no MoveCommitted/source cleanup; originals retained")?;
    Ok(())
}

fn write_proof(fixture: &Fixture) -> Result<()> {
    let requested = std::env::var_os("DESK_SWITCH_NATIVE_FILE_PASTE_PROOF")
        .map(PathBuf::from)
        .unwrap_or_else(|| fixture.root.join("windows-file-paste-proof.json"));
    let workspace = std::env::current_dir()?.canonicalize()?;
    let path = if requested.is_absolute() {
        requested
    } else {
        workspace.join(requested)
    };
    let target = workspace.join("target");
    let mut allowed_roots = vec![fixture.root.clone()];
    if target.try_exists()? {
        allowed_roots.push(target.canonicalize()?);
    }
    if let Some(proof_root) = std::env::var_os("DESK_SWITCH_NATIVE_TRASH_PROOF_DIR") {
        let proof_root = PathBuf::from(proof_root).canonicalize()?;
        let runner_temp = PathBuf::from(
            std::env::var_os("RUNNER_TEMP").context("Missing hosted runner temporary directory")?,
        )
        .canonicalize()?;
        let metadata = fs::symlink_metadata(&proof_root)?;
        ensure!(
            proof_root.parent() == Some(runner_temp.as_path())
                && proof_root.file_name().is_some_and(|name| {
                    let name = name.to_string_lossy();
                    name.starts_with("desk-switch-native-proof-")
                        && name
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                })
                && metadata.is_dir()
                && metadata.file_attributes() & REPARSE == 0,
            "The native launcher proof directory is not an owned hosted-runner fixture"
        );
        allowed_roots.push(proof_root);
    }
    ensure!(
        path.components()
            .all(|part| !matches!(part, Component::ParentDir | Component::CurDir))
            && allowed_roots.iter().any(|root| inside(&path, root)),
        "The proof output must stay in the fixture, workspace target, or guarded launcher proof directory"
    );
    let allowed = allowed_roots
        .iter()
        .find(|root| inside(&path, root))
        .context("Missing allowed proof directory")?;
    let parent = path.parent().context("Missing proof output directory")?;
    let mut existing = parent;
    while !existing.try_exists()? {
        existing = existing.parent().context("Missing proof output ancestor")?;
    }
    ensure!(
        inside(&existing.canonicalize()?, allowed),
        "The proof output parent was redirected"
    );
    fs::create_dir_all(parent)?;
    let path = parent
        .canonicalize()?
        .join(path.file_name().context("Missing proof output file name")?);
    check_bound(allowed, &path)?;
    let coverage = serde_json::json!({
        "metadata_zero_demand": true,
        "real_read": true,
        "retired_handle": true,
        "newer_copy_preserved": true,
        "rename_move": true,
        "copy_delete_move": true,
        "provider_removed_target_read": true,
        "cancel_retains_original": true,
        "source_cleanup_verified": true,
        "native_consumer_cancel_releases_work": true,
        "zero_and_eof_zero_demand": true,
        "tiny_read_missing_destination_retains_original": true
    });
    let proof = serde_json::json!({
        "schema_version": 1,
        "platform": "windows",
        "backend": "Cloud Files",
        "passed": true,
        "fixture": fixture.root,
        "test": "group::clipboard_files::windows::cloud::native_tests::isolated_ci_cloud_file_paste_roundtrip",
        "coverage": coverage,
        "copy_delete_destination_capture": "Explicit PID-bound test capability for the CLI actor's canonical fixture folder; Explorer automatic destination capture is not exercised by this harness.",
        "tiny_read_case_detail": "The actual native cache Delete may be denied or complete, but a missing real destination must produce Failed and no MoveCommitted receipt; captured source originals remain unchanged.",
        "copy_and_cancel_originals_retained_and_rehashed": true,
        "move_originals_rehashed_before_recycle": true,
        "source_cleanup_verified": true,
        "source_cleanup_detail": "After real full-batch NativeResult::MoveCommitted and standalone destination re-reads, the test invokes SourceSelection::finalize_move with its demand-prepared bound manifest under the guarded hosted-native Trash context; it requires Moved, absent originals, and existing native Recycle Bin receipt items."
    });
    fs::write(&path, serde_json::to_vec_pretty(&proof)?)?;
    fixture.log(&format!("Wrote native proof: {}", path.display()))?;
    Ok(())
}

#[test]
#[ignore = "Requires explicitly opted-in disposable GitHub-hosted Windows CI"]
fn isolated_ci_cloud_file_paste_roundtrip() -> Result<()> {
    // Keep this first: no API, filesystem, owner-window, provider, or child
    // setup may happen before all three independent CI markers are checked.
    require_isolated_ci();
    let mut fixture = Fixture::create()?;
    run_held_read(&mut fixture, false)?;
    run_held_read(&mut fixture, true)?;
    run_native_range_cancel(&mut fixture)?;
    run_tiny_read_delete(&mut fixture)?;
    run_move(&mut fixture, true)?;
    run_move(&mut fixture, false)?;
    fixture.clean()?;
    write_proof(&fixture)?;
    fixture.log("All isolated native Cloud Files assertions passed")?;
    Ok(())
}

#[test]
#[ignore = "Child actor used only by the isolated Cloud Files roundtrip"]
fn isolated_ci_cloud_file_consumer() -> Result<()> {
    // This guard also protects direct/accidental execution of the actor test.
    require_isolated_ci();
    let plan: ActorPlan = serde_json::from_str(
        &std::env::var(PLAN_ENV)
            .context("The parent native fixture did not supply an actor plan")?,
    )?;
    validate_fixture(&plan.root, &plan.token)?;
    ensure!(matches!(
        plan.mode.as_str(),
        "retired-read"
            | "cancel-read"
            | "native-cancel"
            | "native-retry"
            | "partial-delete"
            | "rename-tree"
            | "copy-delete"
    ));
    ensure!(!plan.selected.is_empty() && !plan.files.is_empty());
    for path in plan
        .selected
        .iter()
        .chain([&plan.source_base, &plan.destination, &plan.control])
    {
        check_bound(&plan.root, path)?;
    }
    for file in &plan.files {
        let path = joined(&plan.source_base, &file.path);
        check_bound(&plan.root, &path)?;
        ensure!(fs::symlink_metadata(&path)?.len() == file.size);
    }
    for components in &plan.directories {
        let path = joined(&plan.source_base, components);
        check_bound(&plan.root, &path)?;
        ensure!(fs::symlink_metadata(&path)?.is_dir());
        let _ = fs::read_dir(&path)?.collect::<std::io::Result<Vec<_>>>()?;
    }

    // Open carries no byte demand. Keep the handle alive across the parent's
    // clipboard replacement and binding retirement, then read from that handle.
    let mut held = if matches!(
        plan.mode.as_str(),
        "retired-read" | "cancel-read" | "native-cancel" | "native-retry" | "partial-delete"
    ) {
        Some(File::open(&plan.selected[0])?)
    } else {
        None
    };
    if let Some(file) = held.as_mut() {
        // Zero-length and EOF reads are metadata-only probes. Reset the same
        // native handle before the parent permits any actual byte read.
        ensure!(file.read(&mut [])? == 0);
        file.seek(SeekFrom::End(0))?;
        ensure!(file.read(&mut [0u8; 1])? == 0);
        file.seek(SeekFrom::Start(0))?;
    }
    signal(&plan.root, &plan.control, "ready")?;
    wait_for(
        || {
            event_path(&plan.control, "go")?
                .try_exists()
                .map_err(Into::into)
        },
        "the parent to permit actual native I/O",
    )?;
    match plan.mode.as_str() {
        "retired-read" | "cancel-read" | "native-cancel" | "native-retry" => {
            let mut bytes = Vec::new();
            let result = held
                .as_mut()
                .context("Missing pinned native handle")?
                .read_to_end(&mut bytes);
            if plan.mode == "cancel-read" {
                ensure!(
                    result.is_err(),
                    "A cancelled provider exposed native file contents"
                );
            } else {
                result?;
                let expected = &plan.files[0];
                ensure!(
                    bytes.len() as u64 == expected.size
                        && <[u8; 32]>::from(Sha256::digest(&bytes)) == expected.digest
                );
                let copied = plan.destination.join("copied.bin");
                check_bound(&plan.root, &copied)?;
                ensure!(!copied.try_exists()?);
                fs::copy(&plan.selected[0], &copied)?;
                verify_actor_bytes(&copied, expected)?;
                let transient = plan.destination.join("owned-delete-probe.bin");
                check_bound(&plan.root, &transient)?;
                ensure!(!transient.try_exists()?);
                fs::copy(&copied, &transient)?;
                fs::remove_file(&transient)?;
            }
        }
        "partial-delete" => {
            let mut one = [0u8; 1];
            held.as_mut()
                .context("Missing one-byte native reader")?
                .read_exact(&mut one)?;
            check_bound(&plan.root, &plan.selected[0])?;
            let event = if fs::remove_file(&plan.selected[0]).is_err() {
                "delete-denied"
            } else {
                "delete-finished"
            };
            signal(&plan.root, &plan.control, event)?;
        }
        "rename-tree" => {
            ensure!(plan.selected.len() == 1 && !plan.destination.try_exists()?);
            // This is intentionally the first content-affecting operation on
            // the offered tree: same-volume Move may never issue ReadFile first.
            check_bound(&plan.root, &plan.selected[0])?;
            check_bound(&plan.root, &plan.destination)?;
            fs::rename(&plan.selected[0], &plan.destination)?;
            check_bound(&plan.root, &plan.destination)?;
            for file in &plan.files {
                let path = joined(&plan.destination, &file.path[1..]);
                check_bound(&plan.root, &path)?;
                verify_actor_bytes(&path, file)?;
            }
            for components in &plan.directories {
                ensure!(joined(&plan.destination, &components[1..]).is_dir());
            }
        }
        "copy-delete" => {
            ensure!(plan.selected.len() == 2 && plan.files.len() == 2);
            // Reproduce the filesystem operations of a cross-volume Move while
            // retaining the same disposable volume for predictable CI setup.
            for file in &plan.files {
                let source = joined(&plan.source_base, &file.path);
                let target = plan
                    .destination
                    .join(file.path.last().context("Missing selected file name")?);
                check_bound(&plan.root, &source)?;
                check_bound(&plan.root, &target)?;
                ensure!(!target.try_exists()?);
                fs::copy(&source, &target)?;
                verify_actor_bytes(&target, file)?;
            }
            check_bound(&plan.root, &plan.selected[0])?;
            fs::remove_file(&plan.selected[0])?;
            signal(&plan.root, &plan.control, "first-deleted")?;
            wait_for(
                || {
                    event_path(&plan.control, "delete-rest")?
                        .try_exists()
                        .map_err(Into::into)
                },
                "the parent to verify a partial Move is uncommitted",
            )?;
            check_bound(&plan.root, &plan.selected[1])?;
            fs::remove_file(&plan.selected[1])?;
        }
        _ => unreachable!(),
    }
    signal(&plan.root, &plan.control, "done")?;
    if matches!(plan.mode.as_str(), "rename-tree" | "copy-delete") {
        wait_for(
            || Ok(event_path(&plan.control, "provider-removed")?.try_exists()?),
            "the parent to remove the Cloud Files provider",
        )?;
        verify_move_destination(&plan)?;
        signal(&plan.root, &plan.control, "target-reread")?;
    }
    // Stay identifiable until completion callbacks have been authenticated and
    // observed by the parent, rather than exiting immediately after the I/O.
    wait_for(
        || {
            event_path(&plan.control, "finish")?
                .try_exists()
                .map_err(Into::into)
        },
        "the parent to acknowledge native completion",
    )?;
    drop(held);
    Ok(())
}
