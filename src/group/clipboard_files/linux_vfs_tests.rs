//! The model tests never mount a filesystem or touch a native clipboard.
use super::*;
use crate::group::file_transfer::protocol::Descriptor;
use std::sync::{Condvar, Weak};

struct Fixture {
    root: PathBuf,
    files: Vec<Option<PathBuf>>,
    description: Description,
    begins: AtomicUsize,
    opens: AtomicUsize,
    verified: AtomicUsize,
    commits: AtomicUsize,
    failures: AtomicUsize,
    active: AtomicBool,
    paused: AtomicBool,
    pause_after_receipt: AtomicBool,
    blocked: AtomicBool,
    sessions: Mutex<Vec<Weak<TestSession>>>,
    changed: Condvar,
}

impl Fixture {
    fn new() -> Arc<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "desk-switch-vfs-model-{}-{nonce}",
            std::process::id()
        ));
        DirBuilder::new().mode(0o700).create(&root).unwrap();
        let values: &[(&[&str], Option<&[u8]>)] = &[
            (&["bundle"], None),
            (&["bundle", "first.txt"], Some(b"abcdefgh")),
            (&["bundle", "second.txt"], Some(b"second")),
            (&["bundle", "empty.txt"], Some(b"")),
            (&["bundle", "empty-folder"], None),
        ];
        let mut entries = Vec::new();
        let mut files = Vec::new();
        let mut total = 0;
        for (index, (path, bytes)) in values.iter().enumerate() {
            let kind = if let Some(bytes) = bytes {
                let file = root.join(format!("verified-{index}"));
                fs::write(&file, bytes).unwrap();
                files.push(Some(file));
                total += bytes.len() as u64;
                DescriptorKind::File {
                    size: bytes.len() as u64,
                }
            } else {
                files.push(None);
                DescriptorKind::Directory
            };
            entries.push(Descriptor {
                path: path.iter().map(|part| (*part).into()).collect(),
                kind,
            });
        }
        Arc::new(Self {
            root,
            files,
            description: Description {
                name: "Synthetic native paste".into(),
                roots: vec!["bundle".into()],
                entries,
                total,
            },
            begins: AtomicUsize::new(0),
            opens: AtomicUsize::new(0),
            verified: AtomicUsize::new(0),
            commits: AtomicUsize::new(0),
            failures: AtomicUsize::new(0),
            active: AtomicBool::new(true),
            paused: AtomicBool::new(false),
            pause_after_receipt: AtomicBool::new(false),
            blocked: AtomicBool::new(false),
            sessions: Mutex::new(Vec::new()),
            changed: Condvar::new(),
        })
    }
    fn state(self: &Arc<Self>, operation: Operation) -> Arc<State> {
        State::new(self.offer(operation)).unwrap()
    }
    fn offer(self: &Arc<Self>, operation: Operation) -> RemoteOffer {
        RemoteOffer {
            id: "0123456789abcdef0123456789abcdef".into(),
            description: self.description.clone(),
            operation,
            provider: Arc::new(TestProvider {
                fixture: self.clone(),
                leased: false,
            }),
        }
    }
    fn session(&self) -> Arc<TestSession> {
        let sessions = lock(&self.sessions);
        sessions.last().and_then(Weak::upgrade).unwrap()
    }
    fn await_session(&self) -> Arc<TestSession> {
        let sessions = lock(&self.sessions);
        let (sessions, timeout) = self
            .changed
            .wait_timeout_while(sessions, Duration::from_secs(3), |sessions| {
                sessions.is_empty()
            })
            .unwrap();
        assert!(
            !timeout.timed_out(),
            "The native read did not start its demand session"
        );
        sessions.last().and_then(Weak::upgrade).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Remove exact synthetic files and their empty private parent only.
        for file in self.files.iter().flatten() {
            let _ = fs::remove_file(file);
        }
        let _ = fs::remove_dir(&self.root);
    }
}

struct TestProvider {
    fixture: Arc<Fixture>,
    leased: bool,
}
impl DemandProvider for TestProvider {
    fn lease(&self) -> Result<Arc<dyn DemandProvider>> {
        ensure!(
            self.leased || self.fixture.active.load(Ordering::Acquire),
            "The test offer retired"
        );
        Ok(Arc::new(Self {
            fixture: self.fixture.clone(),
            leased: true,
        }))
    }
    fn begin(&self, intent: PasteIntent) -> Result<Arc<dyn PasteSession>> {
        ensure!(self.leased, "Native demand must use a metadata lease");
        let index = self.fixture.begins.fetch_add(1, Ordering::AcqRel);
        let session = Arc::new(TestSession {
            id: format!("synthetic-native-session-{index}"),
            fixture: self.fixture.clone(),
            intent,
            control: Mutex::new(Control {
                paused: self.fixture.paused.load(Ordering::Acquire),
                cancelled: false,
                committed: false,
            }),
            changed: Condvar::new(),
        });
        lock(&self.fixture.sessions).push(Arc::downgrade(&session));
        self.fixture.changed.notify_all();
        Ok(session)
    }
}

struct Control {
    paused: bool,
    cancelled: bool,
    committed: bool,
}
struct TestSession {
    id: String,
    fixture: Arc<Fixture>,
    intent: PasteIntent,
    control: Mutex<Control>,
    changed: Condvar,
}
impl PasteSession for TestSession {
    fn id(&self) -> &str {
        &self.id
    }
    fn open_verified(&self, index: usize) -> Result<File> {
        if self.fixture.pause_after_receipt.load(Ordering::Acquire)
            && self.fixture.verified.load(Ordering::Acquire) != 0
        {
            lock(&self.control).paused = true;
            self.fixture.blocked.store(true, Ordering::Release);
        }
        self.wait_unpaused()?;
        self.fixture.opens.fetch_add(1, Ordering::AcqRel);
        File::open(
            self.fixture
                .files
                .get(index)
                .and_then(Option::as_ref)
                .context("No synthetic file")?,
        )
        .map_err(Into::into)
    }
    fn wait_verified(&self) -> Result<()> {
        self.wait_unpaused()?;
        self.fixture.verified.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
    fn wait_unpaused(&self) -> Result<()> {
        let mut state = lock(&self.control);
        while state.paused && !state.cancelled {
            state = self.changed.wait(state).unwrap();
        }
        ensure!(!state.cancelled, "The synthetic paste was cancelled");
        Ok(())
    }
    fn report_read(&self, _index: usize, _offset: u64, _count: usize) -> Result<()> {
        self.wait_unpaused()
    }
    fn is_terminal(&self) -> bool {
        let state = lock(&self.control);
        state.cancelled || state.committed
    }
    fn can_retry_move(&self) -> bool {
        let state = lock(&self.control);
        state.cancelled && !state.committed
    }
    fn native_complete(&self, result: NativeResult) -> Result<()> {
        let mut state = lock(&self.control);
        match result {
            NativeResult::Cancelled => state.cancelled = true,
            NativeResult::Failed(_) => {
                state.cancelled = true;
                self.fixture.failures.fetch_add(1, Ordering::AcqRel);
            }
            NativeResult::MoveCommitted { consumer, entries } => {
                ensure!(
                    !state.cancelled && self.intent.operation == Operation::Move,
                    "Invalid synthetic move"
                );
                ensure!(
                    self.intent.consumer.as_ref() == Some(&consumer),
                    "Wrong synthetic move consumer"
                );
                ensure!(
                    entries == (0..self.fixture.description.entries.len()).collect::<Vec<_>>(),
                    "Incomplete synthetic receipt set"
                );
                state.committed = true;
                self.fixture.commits.fetch_add(1, Ordering::AcqRel);
            }
        }
        self.changed.notify_all();
        Ok(())
    }
}

fn consumer() -> Consumer {
    Consumer::resolve(std::process::id(), unsafe { libc::geteuid() }).unwrap()
}
fn handle(state: &State, index: usize, owner: Consumer) -> (u64, Arc<OpenHandle>) {
    let number = state.open(INodeNo(index as u64 + 2), owner).unwrap();
    let handle = lock(&state.opened).get(&number).cloned().unwrap();
    (number, handle)
}
fn remove(state: &State, owner: Consumer, index: usize) -> Result<()> {
    let batch = state.prepare_remove(owner, index)?;
    state.remove_data(owner, &batch, index)
}

#[test]
fn metadata_open_eof_and_empty_reads_do_not_begin_or_start_workers() {
    let fixture = Fixture::new();
    let state = fixture.state(Operation::Copy);
    assert_eq!(
        state.tree.lookup(ROOT, OsStr::new("bundle")),
        Some(INodeNo(2))
    );
    assert_eq!(state.tree.attributes(INodeNo(3)).unwrap().perm, 0o644);
    assert_eq!(state.tree.attributes(INodeNo(2)).unwrap().perm, 0o755);
    assert!(state.tree.attributes(INodeNo(3)).unwrap().mtime > UNIX_EPOCH);
    let (number, file) = handle(&state, 1, consumer());
    assert!(state.read_data(&file, number, 0, 0).unwrap().is_empty());
    assert!(state.read_data(&file, number, 8, 10).unwrap().is_empty());
    assert!(
        state
            .read_data(&file, number, u64::MAX, 1)
            .unwrap()
            .is_empty()
    );
    let (empty_number, empty) = handle(&state, 3, consumer());
    assert!(
        state
            .read_data(&empty, empty_number, 0, 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(fixture.begins.load(Ordering::Acquire), 0);
    assert_eq!(fixture.opens.load(Ordering::Acquire), 0);
    assert!(state.pool.get().is_none());
}

#[test]
fn same_native_uri_operation_reownership_keeps_provider_current_past_retirement_ttl() {
    let fixture = Fixture::new();
    let state = fixture.state(Operation::Move);
    let old = Instant::now()
        .checked_sub(RETIRED_TTL + Duration::from_secs(60))
        .unwrap();
    *lock(&state.activity) = old;
    let paths = vec![PathBuf::from("/run/user/1000/private-provider/bundle")];
    let binding = NativeOwnership {
        serial: AtomicU64::new(1),
        paths: paths.clone(),
        operation: Operation::Move,
    };
    let preserved = super::super::Selection {
        sequence: 55,
        paths: paths.clone(),
        operation: Operation::Move,
    };
    assert!(
        binding.observe(Some(&preserved)),
        "Identical URI/operation metadata belongs to this provider despite a new owner/revision"
    );
    assert_eq!(binding.serial.load(Ordering::Acquire), 55);
    assert!(
        state.retained(),
        "A current native offer has no retirement deadline, even long after its last activity"
    );
    assert_eq!(fixture.begins.load(Ordering::Acquire), 0);
    assert_eq!(fixture.opens.load(Ordering::Acquire), 0);
    let changed_operation = super::super::Selection {
        operation: Operation::Copy,
        sequence: 56,
        ..preserved.clone()
    };
    assert!(
        !binding.observe(Some(&changed_operation)),
        "A new Copy cannot reuse an explicit Cut capability"
    );
    assert_eq!(binding.serial.load(Ordering::Acquire), 55);
    let changed_paths = super::super::Selection {
        paths: vec![PathBuf::from("/tmp/an-actual-new-copy")],
        sequence: 57,
        ..preserved
    };
    assert!(!binding.observe(Some(&changed_paths)));
    assert!(!binding.observe(None));
    *lock(&state.retired) = Some(old);
    assert!(
        !state.retained(),
        "A genuinely replaced inactive offer is subject to the bounded grace period"
    );
}

#[test]
fn content_demand_is_per_native_handle_and_survives_offer_retirement() {
    let fixture = Fixture::new();
    let state = fixture.state(Operation::Copy);
    let (number, file) = handle(&state, 1, consumer());
    fixture.active.store(false, Ordering::Release);
    *lock(&state.retired) = Some(Instant::now());
    assert_eq!(state.read_data(&file, number, 0, 4).unwrap(), b"abcd");
    assert_eq!(state.read_data(&file, number, 4, 10).unwrap(), b"efgh");
    assert_eq!(fixture.begins.load(Ordering::Acquire), 1);
    assert_eq!(fixture.opens.load(Ordering::Acquire), 1);
    let (other_number, other) = handle(&state, 1, consumer());
    assert_eq!(
        state.read_data(&other, other_number, 0, 8).unwrap(),
        b"abcdefgh"
    );
    assert_eq!(fixture.begins.load(Ordering::Acquire), 2);
    assert_ne!(
        lock(&file.session).as_ref().unwrap().id(),
        lock(&other.session).as_ref().unwrap().id()
    );
}

#[test]
fn one_native_move_consumer_owns_all_receipts_before_payload() {
    let fixture = Fixture::new();
    let state = fixture.state(Operation::Move);
    let first = consumer();
    let other = Consumer {
        tgid: first.tgid + 1,
        started: first.started + 1,
        ..first
    };
    let (number, file) = handle(&state, 1, first);
    let (other_number, other_file) = handle(&state, 1, other);
    assert!(
        state.prepare_remove(first, 1).is_err(),
        "Unlink cannot manufacture a content demand receipt"
    );
    assert_eq!(fixture.begins.load(Ordering::Acquire), 0);
    assert_eq!(state.read_data(&file, number, 0, 4).unwrap(), b"abcd");
    assert!(state.read_data(&other_file, other_number, 0, 8).is_err());
    assert!(state.prepare_remove(other, 3).is_err());
    assert!(
        state.prepare_remove(first, 1).is_err(),
        "A read gap is not a completed native copy"
    );
    assert_eq!(fixture.begins.load(Ordering::Acquire), 1);
    assert_eq!(state.read_data(&file, number, 4, 4).unwrap(), b"efgh");
    remove(&state, first, 1).unwrap();
    assert!(
        !state.removed(INodeNo(3)),
        "Provisional receipts must not expose a partial folder to a second paste"
    );
    assert_eq!(
        state.read_data(&file, number, 0, 8).unwrap(),
        b"abcdefgh",
        "An already-open native handle retains normal unlink semantics"
    );
    assert_eq!(
        fixture.commits.load(Ordering::Acquire),
        0,
        "Partial folder success never authorizes source cleanup"
    );
}

#[test]
fn cancelled_partial_move_restores_whole_offer_only_after_old_handles_and_io_close() {
    let fixture = Fixture::new();
    let state = fixture.state(Operation::Move);
    let owner = consumer();
    let (number, file) = handle(&state, 1, owner);
    state.read_data(&file, number, 0, 8).unwrap();
    let old = state.prepare_remove(owner, 1).unwrap();
    state.remove_data(owner, &old, 1).unwrap();
    fixture
        .session()
        .native_complete(NativeResult::Cancelled)
        .unwrap();
    state.reset_cancelled_move();
    assert!(
        lock(&state.movement)
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &old)),
        "Live native handles retain their old generation"
    );
    state.close_handle(number);
    state.pending.store(1, Ordering::Release);
    state.reset_cancelled_move();
    assert!(
        lock(&state.movement)
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &old)),
        "Queued old receipts must drain before retry"
    );
    state.pending.store(0, Ordering::Release);
    state.reset_cancelled_move();
    assert!(!state.removed(INodeNo(3)));
    assert!(lock(&state.movement).is_none());
    assert!(
        state.remove_data(owner, &old, 3).is_err(),
        "A queued receipt cannot attach to a new paste generation"
    );
    let (retry_number, retry) = handle(&state, 1, owner);
    assert!(
        state.prepare_remove(owner, 1).is_err(),
        "Retry must not inherit old read coverage"
    );
    assert_eq!(
        state.read_data(&retry, retry_number, 0, 8).unwrap(),
        b"abcdefgh"
    );
    assert_eq!(fixture.begins.load(Ordering::Acquire), 2);
    assert_eq!(fixture.commits.load(Ordering::Acquire), 0);
}

#[test]
fn native_move_requires_every_file_and_directory_receipt_and_zero_items_are_verified() {
    let fixture = Fixture::new();
    let state = fixture.state(Operation::Move);
    let owner = consumer();
    for index in [1, 2] {
        let (number, file) = handle(&state, index, owner);
        assert!(!state.read_data(&file, number, 0, 20).unwrap().is_empty());
        state.close_handle(number);
        remove(&state, owner, index).unwrap();
        assert_eq!(fixture.commits.load(Ordering::Acquire), 0);
    }
    remove(&state, owner, 3).unwrap();
    remove(&state, owner, 4).unwrap();
    assert_eq!(
        fixture.begins.load(Ordering::Acquire),
        1,
        "All folder entries share one explicit native move batch"
    );
    assert_eq!(
        fixture.opens.load(Ordering::Acquire),
        2,
        "Zero-byte items never request a content file"
    );
    assert_eq!(fixture.verified.load(Ordering::Acquire), 4);
    assert_eq!(fixture.commits.load(Ordering::Acquire), 0);
    remove(&state, owner, 0).unwrap();
    assert_eq!(fixture.commits.load(Ordering::Acquire), 1);
    state.reset_cancelled_move();
    assert!(
        state.removed(INodeNo(2)),
        "Committed native moves cannot be replayed by retrying the clipboard"
    );
}

#[test]
fn duplicate_same_process_readers_make_move_receipts_ambiguous() {
    let fixture = Fixture::new();
    let state = fixture.state(Operation::Move);
    let owner = consumer();
    let (first_number, first) = handle(&state, 1, owner);
    let (next_number, next) = handle(&state, 1, owner);
    state.read_data(&first, first_number, 0, 8).unwrap();
    state.read_data(&next, next_number, 0, 8).unwrap();
    assert!(
        remove(&state, owner, 1).is_err(),
        "Concurrent operations in one process cannot authorize source deletion"
    );
    assert!(!state.removed(INodeNo(3)));
    assert_eq!(fixture.commits.load(Ordering::Acquire), 0);
}

#[test]
fn provisional_names_remain_visible_and_duplicate_receipts_poison_the_whole_move() {
    let fixture = Fixture::new();
    let state = fixture.state(Operation::Move);
    let owner = consumer();
    let (number, first) = handle(&state, 1, owner);
    state.read_data(&first, number, 0, 8).unwrap();
    remove(&state, owner, 1).unwrap();
    state.close_handle(number);
    assert!(
        state.tree.nodes[&INodeNo(2)]
            .children
            .values()
            .all(|ino| !state.removed(*ino)),
        "A second same-process directory scan must see the complete original batch"
    );
    assert!(remove(&state, owner, 1).is_err());
    let batch = lock(&state.movement).clone().unwrap();
    assert!(
        lock(&batch.record).ambiguous,
        "A duplicate provisional unlink must fence later receipt branches"
    );
    for index in [2, 3, 4, 0] {
        assert!(state.remove_data(owner, &batch, index).is_err());
    }
    assert_eq!(fixture.commits.load(Ordering::Acquire), 0);
}

#[test]
fn paused_native_read_sleeps_and_cancel_returns_an_error_without_fake_eof() {
    let fixture = Fixture::new();
    fixture.paused.store(true, Ordering::Release);
    let state = fixture.state(Operation::Copy);
    let (number, file) = handle(&state, 1, consumer());
    let (send, receive) = mpsc::sync_channel(1);
    let worker = {
        let state = state.clone();
        std::thread::spawn(move || {
            send.send(state.read_data(&file, number, 0, 8)).unwrap();
        })
    };
    let session = fixture.await_session();
    assert!(receive.recv_timeout(Duration::from_millis(25)).is_err());
    assert_eq!(fixture.opens.load(Ordering::Acquire), 0);
    session.native_complete(NativeResult::Cancelled).unwrap();
    assert!(
        receive
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .is_err()
    );
    worker.join().unwrap();
}

#[test]
fn native_process_identity_resolves_threads_to_one_started_process() {
    let expected = consumer();
    let actual = std::thread::spawn(|| {
        let tid = unsafe { libc::syscall(libc::SYS_gettid) as u32 };
        Consumer::resolve(tid, unsafe { libc::geteuid() }).unwrap()
    })
    .join()
    .unwrap();
    assert_eq!(actual, expected);
    assert!(Consumer::resolve(0, expected.uid).is_err());
}

#[test]
fn move_read_coverage_merges_overlap_without_accepting_gaps_or_overflow() {
    let mut coverage = Coverage::default();
    coverage.record(4, 2, 8).unwrap();
    coverage.record(0, 3, 8).unwrap();
    coverage.record(6, 2, 8).unwrap();
    assert!(!coverage.complete(8));
    coverage.record(2, 3, 8).unwrap();
    assert!(coverage.complete(8));
    assert!(coverage.record(u64::MAX, 2, 8).is_err());
    assert!(coverage.record(7, 2, 8).is_err());
}

/// This gate deliberately fails, rather than skips, when explicitly requested
/// without a disposable hosted runner, private display/bus, or working FUSE.
fn hosted_native_gate() -> PathBuf {
    assert_eq!(
        std::env::var("DESK_SWITCH_NATIVE_FILE_PASTE_TEST").as_deref(),
        Ok("1")
    );
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    assert_eq!(
        std::env::var("RUNNER_ENVIRONMENT").as_deref(),
        Ok("github-hosted")
    );
    let parent_display = std::env::var("DESK_SWITCH_NATIVE_PARENT_DISPLAY").unwrap_or_default();
    let display = std::env::var("DISPLAY").expect("A private Xvfb display is required");
    assert!(display.starts_with(':') && display != parent_display);
    assert!(std::env::var_os("WAYLAND_DISPLAY").is_none());
    let bus =
        std::env::var("DBUS_SESSION_BUS_ADDRESS").expect("A private D-Bus session is required");
    assert_ne!(
        bus,
        std::env::var("DESK_SWITCH_NATIVE_PARENT_BUS").unwrap_or_default()
    );
    let runner =
        fs::canonicalize(std::env::var_os("RUNNER_TEMP").expect("RUNNER_TEMP is required"))
            .unwrap();
    let runtime = fs::canonicalize(
        std::env::var_os("XDG_RUNTIME_DIR").expect("A private runtime directory is required"),
    )
    .unwrap();
    let expected = fs::canonicalize(
        std::env::var_os("DESK_SWITCH_NATIVE_TEST_RUNTIME")
            .expect("A dedicated runtime marker is required"),
    )
    .unwrap();
    assert_eq!(runtime, expected);
    assert!(
        runtime.starts_with(&runner) && runtime != runner,
        "The runtime must be disposable runner storage"
    );
    verify_private_directory(&runtime, unsafe { libc::geteuid() }).unwrap();
    assert!(
        Path::new("/dev/fuse").exists(),
        "The requested FUSE test needs /dev/fuse"
    );
    runtime
}

struct NativeChild(std::process::Child);
impl Drop for NativeChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn native_child(command: &str, arguments: &[&OsStr]) -> NativeChild {
    use std::process::{Command, Stdio};
    NativeChild(
        Command::new(command)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    )
}
fn native_output(command: &str, arguments: &[&OsStr]) -> std::process::Output {
    use std::process::{Command, Stdio};
    let mut child = NativeChild(
        Command::new(command)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if child.0.try_wait().unwrap().is_some() {
            break;
        }
        assert!(
            Instant::now() < until,
            "A bounded native test command timed out: {command}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    use std::io::Read as _;
    child
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut stdout)
        .unwrap();
    child
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_end(&mut stderr)
        .unwrap();
    std::process::Output {
        status: child.0.try_wait().unwrap().unwrap(),
        stdout,
        stderr,
    }
}
fn native_ok(command: &str, arguments: &[&OsStr]) -> Vec<u8> {
    let result = native_output(command, arguments);
    assert!(
        result.status.success(),
        "Native command {command} failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}
fn eventually(timeout: Duration, message: &str, mut ready: impl FnMut() -> bool) {
    let until = Instant::now() + timeout;
    while !ready() {
        assert!(Instant::now() < until, "{message}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn process_cpu() -> Duration {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut value) },
        0
    );
    Duration::new(value.tv_sec as u64, value.tv_nsec as u32)
}
fn native_window(title: &str) -> String {
    let mut window = None;
    eventually(
        Duration::from_secs(5),
        "The private Nautilus window did not appear",
        || {
            let output = native_output(
                "xdotool",
                &[
                    OsStr::new("search"),
                    OsStr::new("--onlyvisible"),
                    OsStr::new("--name"),
                    OsStr::new(title),
                ],
            );
            window = output.status.success().then(|| {
                String::from_utf8(output.stdout)
                    .unwrap()
                    .lines()
                    .next()
                    .unwrap()
                    .to_owned()
            });
            window.is_some()
        },
    );
    window.unwrap()
}
fn paste_into(title: &str) {
    let window = native_window(title);
    native_ok(
        "xdotool",
        &[
            OsStr::new("windowactivate"),
            OsStr::new("--sync"),
            OsStr::new(&window),
        ],
    );
    // Let GTK refresh its clipboard actions after the private window maps.
    std::thread::sleep(Duration::from_millis(150));
    native_ok(
        "xdotool",
        &[
            OsStr::new("key"),
            OsStr::new("--clearmodifiers"),
            OsStr::new("ctrl+v"),
        ],
    );
}
fn publish_native(fixture: &Arc<Fixture>, operation: Operation) -> Box<dyn PublishedOffer> {
    super::super::read().unwrap();
    let expected = super::super::sequence().unwrap();
    super::publish_offer_if_current(fixture.offer(operation), expected)
        .unwrap()
        .expect("The isolated selection unexpectedly changed")
}
fn check_copied_folder(destination: &Path) -> bool {
    fs::read(destination.join("bundle/first.txt"))
        .ok()
        .as_deref()
        == Some(b"abcdefgh")
        && fs::read(destination.join("bundle/second.txt"))
            .ok()
            .as_deref()
            == Some(b"second")
        && fs::metadata(destination.join("bundle/empty.txt"))
            .ok()
            .is_some_and(|info| info.len() == 0)
        && destination.join("bundle/empty-folder").is_dir()
}

#[test]
#[ignore = "Requires a disposable hosted Linux runner with private Xvfb, D-Bus and FUSE; never run against a user desktop"]
fn isolated_ci_native_deferred_file_paste() {
    let runtime = hosted_native_gate();
    native_ok("fusermount3", &[OsStr::new("--version")]);
    let _window_manager = native_child("openbox", &[OsStr::new("--sm-disable")]);
    let copy = Fixture::new();
    let copied = runtime.join("DeskSwitch-native-copy-destination");
    fs::create_dir(&copied).unwrap();
    let _nautilus = native_child(
        "nautilus",
        &[OsStr::new("--new-window"), copied.as_os_str()],
    );
    native_window("DeskSwitch-native-copy-destination");
    copy.paused.store(true, Ordering::Release);
    let copied_offer = publish_native(&copy, Operation::Copy);
    let published_revision = copied_offer.sequence().unwrap();
    // A real, distinct X11 owner models a clipboard manager preserving URI data.
    // This is metadata publication only, not a content-read or paste operation.
    let mut clipboard_manager = super::super::x11::X11::new().unwrap();
    clipboard_manager
        .write(&copied_offer.local_paths())
        .unwrap();
    assert!(copied_offer.is_current().unwrap());
    assert_ne!(copied_offer.sequence().unwrap(), published_revision);
    MOUNTS.with(|mounts| {
        let mounts = mounts.borrow();
        let state = &mounts.last().unwrap().state;
        *lock(&state.activity) = Instant::now()
            .checked_sub(RETIRED_TTL + Duration::from_secs(60))
            .unwrap();
        assert!(
            state.retained(),
            "The currently copied URI capability must survive idle time and native reownership"
        );
    });
    let virtual_root = copied_offer.local_paths().remove(0);
    assert_eq!(fs::read_dir(&virtual_root).unwrap().count(), 4);
    assert_eq!(
        fs::metadata(virtual_root.join("first.txt")).unwrap().len(),
        8
    );
    drop(File::open(virtual_root.join("first.txt")).unwrap());
    std::thread::sleep(Duration::from_millis(350));
    assert_eq!(
        copy.begins.load(Ordering::Acquire),
        0,
        "Nautilus eager clipboard reads, stat, readdir and OPEN must remain metadata-only"
    );
    assert_eq!(copy.opens.load(Ordering::Acquire), 0);
    let idle_cpu = process_cpu();
    std::thread::sleep(Duration::from_millis(300));
    let idle_used = process_cpu().saturating_sub(idle_cpu);
    assert!(
        idle_used < Duration::from_millis(80),
        "Metadata-only provider did not settle idle"
    );
    paste_into("DeskSwitch-native-copy-destination");
    let session = copy.await_session();
    assert_eq!(
        copy.opens.load(Ordering::Acquire),
        0,
        "Paused native demand cannot read source payload"
    );
    let paused_cpu = process_cpu();
    std::thread::sleep(Duration::from_millis(300));
    let paused_used = process_cpu().saturating_sub(paused_cpu);
    assert!(
        paused_used < Duration::from_millis(80),
        "Paused native READ must sleep without busy polling"
    );
    copy.paused.store(false, Ordering::Release);
    lock(&session.control).paused = false;
    session.changed.notify_all();
    eventually(
        Duration::from_secs(10),
        "Real Nautilus Copy did not produce the complete folder",
        || check_copied_folder(&copied),
    );
    assert_ne!(
        fs::metadata(copied.join("bundle/first.txt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o200,
        0,
        "Native copies must remain writable"
    );
    assert_ne!(
        fs::metadata(copied.join("bundle"))
            .unwrap()
            .permissions()
            .mode()
            & 0o200,
        0
    );
    assert_eq!(
        copy.commits.load(Ordering::Acquire),
        0,
        "Copy delivery cannot invent a native completion receipt"
    );

    let moved = Fixture::new();
    let move_destination = runtime.join("DeskSwitch-native-move-destination");
    fs::create_dir(&move_destination).unwrap();
    native_ok(
        "nautilus",
        &[OsStr::new("--new-window"), move_destination.as_os_str()],
    );
    native_window("DeskSwitch-native-move-destination");
    let moved_offer = publish_native(&moved, Operation::Move);
    std::thread::sleep(Duration::from_millis(350));
    assert_eq!(
        moved.begins.load(Ordering::Acquire),
        0,
        "Cut only publishes metadata"
    );
    paste_into("DeskSwitch-native-move-destination");
    eventually(
        Duration::from_secs(10),
        "Real Nautilus Move did not produce all final native receipts",
        || moved.commits.load(Ordering::Acquire) == 1,
    );
    assert!(check_copied_folder(&move_destination));
    assert!(
        moved.files.iter().flatten().all(|path| path.exists()),
        "The native provider never deletes source storage directly"
    );

    let cancelled = Fixture::new();
    cancelled.pause_after_receipt.store(true, Ordering::Release);
    let cancel_destination = runtime.join("DeskSwitch-native-cancel-destination");
    fs::create_dir(&cancel_destination).unwrap();
    native_ok(
        "nautilus",
        &[OsStr::new("--new-window"), cancel_destination.as_os_str()],
    );
    native_window("DeskSwitch-native-cancel-destination");
    let cancelled_offer = publish_native(&cancelled, Operation::Move);
    std::thread::sleep(Duration::from_millis(350));
    assert_eq!(cancelled.begins.load(Ordering::Acquire), 0);
    paste_into("DeskSwitch-native-cancel-destination");
    let cancelled_session = cancelled.await_session();
    eventually(
        Duration::from_secs(5),
        "The native partial move did not pause after a completed item",
        || cancelled.blocked.load(Ordering::Acquire),
    );
    assert!(cancelled.verified.load(Ordering::Acquire) != 0);
    assert_eq!(
        cancelled.commits.load(Ordering::Acquire),
        0,
        "A partial native destination cannot delete originals"
    );
    cancelled_session
        .native_complete(NativeResult::Cancelled)
        .unwrap();
    eventually(
        Duration::from_secs(3),
        "Native cancellation did not terminate the pending READ with an error",
        || cancelled.failures.load(Ordering::Acquire) != 0,
    );
    assert!(
        !check_copied_folder(&cancel_destination),
        "Cancellation must not silently paste fake empty files"
    );
    assert_eq!(cancelled.commits.load(Ordering::Acquire), 0);
    assert!(cancelled.files.iter().flatten().all(|path| path.exists()));
    let proof = serde_json::json!({
        "scope": "Real Nautilus Ctrl+V on hosted private X11/Xvfb/D-Bus with actual FUSE; GNOME Wayland companion is covered separately by mocks",
        "metadata_only_payload_demands": 0,
        "native_uri_reownership_preserves_current_offer": true,
        "copy_complete_folder": true,
        "copy_destination_writable": true,
        "native_move_complete_batch_receipts": moved.commits.load(Ordering::Acquire),
        "partial_native_move_cancel_preserves_source_storage": true,
        "cancel_returns_native_error_instead_of_empty_eof": true,
        "provider_idle_cpu_ms_over_300ms": idle_used.as_millis(),
        "provider_paused_cpu_ms_over_300ms": paused_used.as_millis(),
        "source_cleanup_scope": "Native provider does not delete originals; authenticated transfer-service quarantine is verified in separate tests"
    });
    fs::write(
        runtime.join("native-file-paste-proof.json"),
        serde_json::to_vec_pretty(&proof).unwrap(),
    )
    .unwrap();
    drop(_nautilus);
    drop(_window_manager);
    drop((cancelled_offer, moved_offer, copied_offer));
    MOUNTS.with(|mounts| mounts.borrow_mut().clear());
}
