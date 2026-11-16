use super::*;
use crate::group::file_transfer::Peer;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{BufRead, BufReader},
    net::{Ipv4Addr, SocketAddr, TcpListener},
    path::Path,
    process::{Child, ChildStdin, Command as ProcessCommand, Stdio},
    sync::mpsc as channel,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const CHILD_ENV: &str = "DESK_SWITCH_RESOURCE_TEST_PEER";
const MARKER: &str = "@desk-switch-resource-fixture ";
const WAIT: Duration = Duration::from_secs(30);

#[derive(Deserialize, Serialize)]
struct Launch {
    dir: PathBuf,
    address: SocketAddr,
}
#[derive(Deserialize, Serialize)]
enum Control {
    Trust {
        key: String,
        address: SocketAddr,
        workspace: String,
    },
    Command(Command),
    Snapshot,
    Stop,
}
#[derive(Debug, Deserialize, Serialize)]
struct State {
    key: String,
    settings: Settings,
    catalogs: BTreeMap<String, Catalog>,
    versions: Vec<(String, super::super::storage::Version)>,
    status: String,
    error: Option<String>,
    links: usize,
}
fn state(service: &Service) -> State {
    let view = service.snapshot();
    State {
        key: config::hex(&service.core.identity.public),
        settings: service.core.settings.lock().unwrap().clone(),
        catalogs: view.catalogs,
        versions: view
            .recovery
            .into_iter()
            .map(|v| (v.folder, v.version))
            .collect(),
        status: view.status,
        error: view.error,
        links: service.core.sockets.lock().unwrap().len(),
    }
}
fn emit(service: &Service) {
    println!(
        "{MARKER}{}",
        serde_json::to_string(&state(service)).unwrap()
    );
    std::io::stdout().flush().unwrap();
}

// This fixture entry point runs only in explicitly launched test children. It
// starts the resource service, without Desktop, device services or an updater.
#[test]
fn headless_peer_process() {
    let Some(launch) = std::env::var_os(CHILD_ENV) else {
        return;
    };
    let launch: Launch = serde_json::from_str(launch.to_str().unwrap()).unwrap();
    let intended = std::env::current_dir()
        .unwrap()
        .join("target/resource-fixtures");
    let fixture = launch.dir.parent().unwrap().to_path_buf();
    assert!(launch.dir.is_absolute() && fixture.parent() == Some(intended.as_path()));
    assert!(fixture.file_name().unwrap().to_str().unwrap().len() == 32);
    assert!(matches!(
        launch.dir.file_name().unwrap().to_str().unwrap(),
        "peer-a" | "peer-b"
    ));
    assert!(launch.address.ip().is_loopback());
    if !launch.dir.join("identity").exists() {
        config::create_identity(&launch.dir).unwrap();
    }
    let listener = TcpListener::bind(launch.address).unwrap();
    let service = Service::start_on(launch.dir, egui::Context::default(), listener).unwrap();
    emit(&service);
    for line in std::io::stdin().lock().lines() {
        let control: Control = serde_json::from_str(&line.unwrap()).unwrap();
        match control {
            Control::Trust {
                key,
                address,
                workspace,
            } => {
                assert!(address.ip().is_loopback());
                service.update(TrustSnapshot {
                    own: config::hex(&service.core.identity.public),
                    workspace_id: workspace,
                    peers: BTreeMap::from([(
                        key,
                        Peer {
                            address,
                            online: true,
                        },
                    )]),
                    ..TrustSnapshot::default()
                });
            }
            Control::Command(command) => {
                // The child controller cannot enumerate or operate USB devices.
                assert!(!matches!(
                    command,
                    Command::RefreshUsb
                        | Command::ShareUsb { .. }
                        | Command::UnshareUsb(_)
                        | Command::AttachUsb { .. }
                ));
                if let Command::AddFolder { root, .. } = &command {
                    assert!(root.is_absolute() && root.starts_with(&fixture));
                }
                assert!(!service.snapshot().busy);
                let marker = settings::new_id().unwrap();
                service.core.change(|v| v.status = marker.clone());
                service.submit(command).unwrap();
                let deadline = Instant::now() + WAIT;
                loop {
                    let view = service.snapshot();
                    if !view.busy && view.status != marker {
                        break;
                    }
                    assert!(Instant::now() < deadline, "Resource command timed out");
                    thread::sleep(Duration::from_millis(5));
                }
            }
            Control::Snapshot => {}
            Control::Stop => {
                emit(&service);
                return;
            }
        }
        emit(&service);
    }
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::current_dir()
            .unwrap()
            .join("target/resource-fixtures")
            .join(settings::new_id().unwrap());
        for name in ["peer-a", "peer-b", "folder-a", "folder-b"] {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        Self(root)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let intended = std::env::current_dir()
            .unwrap()
            .join("target/resource-fixtures");
        assert!(
            self.0.is_absolute()
                && self.0.parent() == Some(intended.as_path())
                && self.0.file_name().unwrap().len() == 32
        );
        fs::remove_dir_all(&self.0).unwrap();
    }
}

struct TestPeer {
    child: Child,
    input: ChildStdin,
    output: channel::Receiver<State>,
    reader: Option<JoinHandle<()>>,
}
impl TestPeer {
    fn start(dir: PathBuf, address: SocketAddr) -> (Self, State) {
        let mut command = ProcessCommand::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "group::resources::service::tests::headless_peer_process",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(
                CHILD_ENV,
                serde_json::to_string(&Launch { dir, address }).unwrap(),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().unwrap();
        let input = child.stdin.take().unwrap();
        let output = child.stdout.take().unwrap();
        let (send, receive) = channel::sync_channel(2);
        let reader = thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else {
                    break;
                };
                if let Some((_, json)) = line.split_once(MARKER) {
                    let value = serde_json::from_str(json).unwrap();
                    if send.send(value).is_err() {
                        break;
                    }
                }
            }
        });
        let peer = Self {
            child,
            input,
            output: receive,
            reader: Some(reader),
        };
        let initial = peer
            .output
            .recv_timeout(WAIT)
            .expect("Headless peer did not start");
        (peer, initial)
    }
    fn request(&mut self, control: Control) -> State {
        serde_json::to_writer(&mut self.input, &control).unwrap();
        writeln!(&mut self.input).unwrap();
        self.input.flush().unwrap();
        self.output
            .recv_timeout(WAIT)
            .expect("Headless peer did not respond")
    }
    fn command(&mut self, command: Command) -> State {
        self.request(Control::Command(command))
    }
    fn success(&mut self, command: Command) -> State {
        let state = self.command(command);
        assert!(state.error.is_none(), "{}: {:?}", state.status, state.error);
        state
    }
    fn trust(&mut self, key: &str, address: SocketAddr, workspace: &str) {
        self.request(Control::Trust {
            key: key.into(),
            address,
            workspace: workspace.into(),
        });
    }
    fn idle_links(&mut self) {
        let deadline = Instant::now() + WAIT;
        while self.request(Control::Snapshot).links != 0 {
            assert!(
                Instant::now() < deadline,
                "Resource connection leaked after completion"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn stop(&mut self) {
        self.request(Control::Stop);
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline, "Headless peer did not stop");
            thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for TestPeer {
    fn drop(&mut self) {
        // Only this fixture-owned child is terminated, including panic cleanup.
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[derive(Default)]
struct Traffic {
    up: AtomicU64,
    down: AtomicU64,
    cut: AtomicBool,
    remaining: AtomicU64,
    direction: AtomicU64,
}
struct Proxy {
    traffic: Arc<Traffic>,
    stop: Arc<AtomicBool>,
    sockets: Arc<Mutex<Vec<TcpStream>>>,
    worker: Option<JoinHandle<()>>,
}
fn forward(mut from: TcpStream, mut to: TcpStream, traffic: Arc<Traffic>, up: bool) {
    let mut bytes = [0; 16 * 1024];
    while let Ok(count) = from.read(&mut bytes) {
        if count == 0 {
            break;
        }
        let fault = traffic.direction.load(Ordering::Acquire) == if up { 1 } else { 2 };
        let count = if fault {
            count.min(traffic.remaining.load(Ordering::Acquire) as usize)
        } else {
            count
        };
        if to.write_all(&bytes[..count]).is_err() {
            break;
        }
        if up {
            traffic.up.fetch_add(count as u64, Ordering::Relaxed);
        } else {
            traffic.down.fetch_add(count as u64, Ordering::Relaxed);
        }
        if fault && traffic.remaining.fetch_sub(count as u64, Ordering::AcqRel) == count as u64 {
            traffic.cut.store(true, Ordering::Release);
            break;
        }
    }
    let _ = from.shutdown(Shutdown::Both);
    let _ = to.shutdown(Shutdown::Both);
}
impl Proxy {
    fn start(listener: TcpListener, target: SocketAddr) -> Self {
        listener.set_nonblocking(true).unwrap();
        let traffic = Arc::new(Traffic::default());
        let stop = Arc::new(AtomicBool::new(false));
        let sockets = Arc::new(Mutex::new(Vec::new()));
        let (data, stopping, active) = (traffic.clone(), stop.clone(), sockets.clone());
        let worker = thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                let client = match listener.accept() {
                    Ok((socket, _)) => socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("Loopback proxy: {error}"),
                };
                client.set_nonblocking(false).unwrap();
                let server = TcpStream::connect_timeout(&target, Duration::from_secs(3)).unwrap();
                for socket in [&client, &server] {
                    socket.set_nodelay(true).unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    socket
                        .set_write_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                }
                *active.lock().unwrap() =
                    vec![client.try_clone().unwrap(), server.try_clone().unwrap()];
                let (back, incoming, stats) = (
                    server.try_clone().unwrap(),
                    client.try_clone().unwrap(),
                    data.clone(),
                );
                let receiver = thread::spawn(move || forward(back, incoming, stats, false));
                forward(client, server, data.clone(), true);
                receiver.join().unwrap();
                active.lock().unwrap().clear();
            }
        });
        Self {
            traffic,
            stop,
            sockets,
            worker: Some(worker),
        }
    }
    fn arm(&self, up: bool) {
        self.traffic.cut.store(false, Ordering::Release);
        self.traffic.remaining.store(256 * 1024, Ordering::Release);
        self.traffic
            .direction
            .store(if up { 1 } else { 2 }, Ordering::Release);
    }
    fn resume(&self) {
        self.traffic.direction.store(0, Ordering::Release);
        self.traffic.up.store(0, Ordering::Release);
        self.traffic.down.store(0, Ordering::Release);
    }
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for socket in self
            .sockets
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
        {
            let _ = socket.shutdown(Shutdown::Both);
        }
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !thread::panicking() {
                result.expect("Loopback proxy worker failed");
            }
        }
    }
}

fn unchanged_pair(state: &State) -> &Pair {
    &state.settings.pairs[0]
}
fn equal_folders(a: &Path, b: &Path) {
    let cancel = AtomicBool::new(false);
    assert_eq!(
        Root::open(a).unwrap().manifest(&cancel).unwrap(),
        Root::open(b).unwrap().manifest(&cancel).unwrap()
    );
}
fn partial(path: &Path) -> (PathBuf, Vec<u8>) {
    let mut files = fs::read_dir(path.join(sync::META).join("partial"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| fs::metadata(path).unwrap().len() > 0)
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 1, "Expected one durable partial file");
    let path = files.pop().unwrap();
    let bytes = fs::read(&path).unwrap();
    (path, bytes)
}

#[test]
fn two_headless_processes_resume_both_directions_after_network_loss_and_restart() {
    let fixture = Fixture::new();
    let a_root = fixture.path("folder-a");
    let b_root = fixture.path("folder-b");
    fs::create_dir_all(a_root.join("nested/empty")).unwrap();
    fs::write(a_root.join("nested/local.txt"), b"from a").unwrap();
    fs::write(a_root.join("rename.txt"), b"rename payload").unwrap();
    fs::write(a_root.join("delete.txt"), b"delete payload").unwrap();
    for root in [&a_root, &b_root] {
        fs::write(root.join("shared.txt"), b"base").unwrap();
    }
    fs::write(b_root.join("from-b.txt"), b"from b").unwrap();
    let large: Vec<u8> = (0..6 * 1024 * 1024).map(|n| (n % 251) as u8).collect();
    fs::write(a_root.join("large.bin"), &large).unwrap();
    fs::write(b_root.join("large.bin"), b"previous large").unwrap();

    let reserve_a = TcpListener::bind((Ipv4Addr::new(127, 0, 0, 1), 0)).unwrap();
    let a_address = reserve_a.local_addr().unwrap();
    let b_address = SocketAddr::from((Ipv4Addr::new(127, 0, 0, 2), a_address.port()));
    let proxy_address = SocketAddr::from((Ipv4Addr::new(127, 0, 0, 3), a_address.port()));
    let reserve_b = TcpListener::bind(b_address).unwrap();
    let proxy_listener = TcpListener::bind(proxy_address).unwrap();
    drop((reserve_a, reserve_b));
    let (mut a, a_initial) = TestPeer::start(fixture.path("peer-a"), a_address);
    let (mut b, b_initial) = TestPeer::start(fixture.path("peer-b"), b_address);
    assert_ne!(a.child.id(), b.child.id());
    let proxy = Proxy::start(proxy_listener, b_address);
    let workspace = settings::new_id().unwrap();
    a.trust(&b_initial.key, proxy_address, &workspace);
    b.trust(&a_initial.key, a_address, &workspace);
    let shared_a = a.success(Command::AddFolder {
        name: "A documents".into(),
        peer: b_initial.key.clone(),
        root: a_root.clone(),
    });
    let shared_b = b.success(Command::AddFolder {
        name: "B documents".into(),
        peer: a_initial.key.clone(),
        root: b_root.clone(),
    });
    let a_grant = shared_a.settings.folders[0].id.clone();
    let b_grant = shared_b.settings.folders[0].id.clone();
    let catalog = a.success(Command::RefreshPeer(b_initial.key.clone()));
    assert_eq!(catalog.catalogs[&b_initial.key].folders[0].id, b_grant);
    let paired = a.success(Command::AddPair {
        name: "Synthetic documents".into(),
        local: a_grant.clone(),
        remote: b_grant.clone(),
    });
    let pair = unchanged_pair(&paired).id.clone();
    let compared = a.success(Command::Compare(pair.clone()));
    assert_eq!(
        unchanged_pair(&compared)
            .pending
            .as_ref()
            .unwrap()
            .conflicts,
        ["large.bin"]
    );
    assert!(
        !unchanged_pair(&compared)
            .pending
            .as_ref()
            .unwrap()
            .operations
            .iter()
            .any(|op| op.change == Change::Delete)
    );
    a.success(Command::Resolve {
        pair: pair.clone(),
        choices: BTreeMap::from([("large.bin".into(), Resolution::Local)]),
    });

    for up in [true, false] {
        let mut expected = large.clone();
        if !up {
            expected.reverse();
            fs::write(b_root.join("large.bin"), &expected).unwrap();
            let compared = a.success(Command::Compare(pair.clone()));
            assert!(
                unchanged_pair(&compared)
                    .pending
                    .as_ref()
                    .unwrap()
                    .conflicts
                    .is_empty()
            );
        }
        proxy.arm(up);
        let interrupted = a.command(Command::Run(pair.clone()));
        assert!(
            interrupted.error.is_some(),
            "The injected connection loss was not reported"
        );
        assert!(proxy.traffic.cut.load(Ordering::Acquire));
        let saved = unchanged_pair(&interrupted).clone();
        assert!(saved.attempted && saved.pending.is_some());
        let receiver = if up { &b_root } else { &a_root };
        let (staged, prefix) = partial(receiver);
        assert!(prefix.len() >= super::super::safe::CHUNK && prefix.len() < large.len());
        assert_eq!(prefix, expected[..prefix.len()]);
        assert_eq!(
            fs::read(receiver.join("large.bin")).unwrap(),
            if up {
                b"previous large".to_vec()
            } else {
                large.clone()
            }
        );
        b.idle_links();

        // Kill the sender/coordinator and cleanly restart the receiver as two
        // separate OS processes. Neither in-memory plan nor journal survives.
        drop(a);
        b.stop();
        drop(b);
        let (next_a, restarted_a) = TestPeer::start(fixture.path("peer-a"), a_address);
        let (next_b, restarted_b) = TestPeer::start(fixture.path("peer-b"), b_address);
        a = next_a;
        b = next_b;
        assert_eq!(restarted_a.key, a_initial.key);
        assert_eq!(restarted_b.key, b_initial.key);
        let loaded = unchanged_pair(&restarted_a);
        assert_eq!(loaded.run_id, saved.run_id);
        assert_eq!(loaded.pending, saved.pending);
        assert_eq!(loaded.baseline, saved.baseline);
        assert!(loaded.attempted);
        assert_eq!(fs::read(&staged).unwrap(), prefix);
        a.trust(&b_initial.key, proxy_address, &workspace);
        b.trust(&a_initial.key, a_address, &workspace);
        proxy.resume();
        let complete = a.success(Command::Run(pair.clone()));
        assert!(unchanged_pair(&complete).pending.is_none());
        assert!(unchanged_pair(&complete).baseline.is_some());
        assert!(!unchanged_pair(&complete).attempted);
        b.idle_links();
        equal_folders(&a_root, &b_root);
        assert_eq!(fs::read(a_root.join("large.bin")).unwrap(), expected);
        assert_eq!(fs::read(b_root.join("large.bin")).unwrap(), expected);
        let transferred = if up {
            proxy.traffic.up.load(Ordering::Acquire)
        } else {
            proxy.traffic.down.load(Ordering::Acquire)
        };
        assert!(
            transferred < (large.len() - prefix.len() / 2) as u64,
            "Resume retransmitted the complete file: {transferred} bytes, saved {}",
            prefix.len()
        );
        println!(
            "resumed {} bytes after process restart, {} bytes remained on the wire",
            prefix.len(),
            transferred
        );
    }
    assert!(a_root.join("nested/empty").is_dir() && b_root.join("nested/empty").is_dir());
    assert_eq!(fs::read(a_root.join("from-b.txt")).unwrap(), b"from b");
    fs::rename(a_root.join("rename.txt"), a_root.join("renamed.txt")).unwrap();
    fs::remove_file(a_root.join("delete.txt")).unwrap();
    fs::write(a_root.join("nested/local.txt"), b"second a").unwrap();
    fs::write(b_root.join("from-b.txt"), b"second b").unwrap();
    fs::write(a_root.join("shared.txt"), b"independent a").unwrap();
    fs::write(b_root.join("shared.txt"), b"independent b").unwrap();
    let compared = a.success(Command::Compare(pair.clone()));
    let plan = unchanged_pair(&compared).pending.as_ref().unwrap();
    assert_eq!(plan.conflicts, ["shared.txt"]);
    assert!(
        plan.operations
            .iter()
            .any(|op| matches!(&op.change, Change::Rename { from, .. } if from == "rename.txt"))
    );
    assert!(
        plan.operations
            .iter()
            .any(|op| op.path == "delete.txt" && op.change == Change::Delete)
    );
    let blocked = a.command(Command::Run(pair.clone()));
    assert!(blocked.error.is_some() && !unchanged_pair(&blocked).attempted);
    a.success(Command::Resolve {
        pair: pair.clone(),
        choices: BTreeMap::from([("shared.txt".into(), Resolution::Local)]),
    });
    a.success(Command::Run(pair.clone()));
    b.idle_links();
    equal_folders(&a_root, &b_root);
    assert!(!b_root.join("rename.txt").exists() && !b_root.join("delete.txt").exists());
    assert_eq!(
        fs::read(b_root.join("renamed.txt")).unwrap(),
        b"rename payload"
    );
    assert_eq!(fs::read(a_root.join("from-b.txt")).unwrap(), b"second b");
    let versions = b.success(Command::Versions(b_grant.clone()));
    let restore = versions
        .versions
        .iter()
        .find(|(_, v)| v.path == "shared.txt")
        .unwrap()
        .1
        .id
        .clone();
    for (name, bytes) in [
        ("large.bin", b"previous large".as_slice()),
        ("delete.txt", b"delete payload".as_slice()),
    ] {
        let version = &versions
            .versions
            .iter()
            .find(|(_, v)| v.path == name)
            .unwrap()
            .1;
        assert_eq!(
            fs::read(b_root.join(sync::META).join("versions").join(&version.id)).unwrap(),
            bytes
        );
    }
    let local_versions = a.success(Command::Versions(a_grant));
    let before_download = &local_versions
        .versions
        .iter()
        .find(|(_, v)| v.path == "large.bin")
        .unwrap()
        .1;
    assert_eq!(
        fs::read(
            a_root
                .join(sync::META)
                .join("versions")
                .join(&before_download.id)
        )
        .unwrap(),
        large
    );
    b.success(Command::Restore {
        folder: b_grant,
        version: restore,
    });
    assert_eq!(
        fs::read(b_root.join("shared.txt")).unwrap(),
        b"independent b"
    );
    a.success(Command::Compare(pair.clone()));
    a.success(Command::Run(pair.clone()));
    b.idle_links();
    equal_folders(&a_root, &b_root);
    let unchanged = a.success(Command::Compare(pair));
    assert!(
        unchanged_pair(&unchanged)
            .pending
            .as_ref()
            .unwrap()
            .operations
            .is_empty()
    );
    a.idle_links();
    b.idle_links();
    a.stop();
    b.stop();
}
