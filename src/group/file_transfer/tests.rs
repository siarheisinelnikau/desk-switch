//! These fixtures never open the native clipboard or a GUI window.
use super::*;
use std::{
    fs,
    path::Path,
    process::{Child, Command as ProcessCommand, Stdio},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "desk-switch-file-service-test-{}-{}",
            std::process::id(),
            new_id().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        Self(fs::canonicalize(root).unwrap())
    }
    fn path(&self, path: &str) -> PathBuf {
        self.0.join(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let parent = fs::canonicalize(std::env::temp_dir()).unwrap();
        assert!(
            self.0.is_absolute()
                && self.0.starts_with(parent)
                && self
                    .0
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("desk-switch-file-service-test-")
        );
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Actor(Child);
impl Drop for Actor {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[cfg(windows)]
#[test]
fn the_file_transfer_listener_refuses_to_share_an_occupied_port() {
    let first = listener(0).unwrap();
    let occupied = first.local_addr().unwrap().port();
    assert!(
        listener(occupied).is_err(),
        "a second profile must not share the authenticated file-transfer port"
    );
}

fn port() -> u16 {
    loop {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        if port > 1 {
            return port;
        }
    }
}
fn wait(mut predicate: impl FnMut() -> bool) {
    let start = Instant::now();
    while !predicate() {
        assert!(
            start.elapsed() < Duration::from_secs(45),
            "Synthetic transfer actor timed out"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn spawn_actor(fixture: &Fixture, role: &str, own_port: u16, peer_port: u16, peer: &str) -> Actor {
    Actor(
        ProcessCommand::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "group::file_transfer::tests::subprocess_fixture",
                "--nocapture",
            ])
            .env("DESK_SWITCH_TRANSFER_FIXTURE", &fixture.0)
            .env("DESK_SWITCH_TRANSFER_ROLE", role)
            .env("DESK_SWITCH_TRANSFER_PORT", own_port.to_string())
            .env("DESK_SWITCH_TRANSFER_PEER_PORT", peer_port.to_string())
            .env("DESK_SWITCH_TRANSFER_PEER", peer)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    )
}
fn read_view(path: &Path) -> Option<Transfer> {
    fs::read(path)
        .ok()
        .and_then(|v| serde_json::from_slice(&v).ok())
}

#[test]
fn authenticated_folder_transfer_resumes_after_both_processes_restart() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.path("alice")).unwrap();
    fs::create_dir(fixture.path("bob")).unwrap();
    let alice = config::create_identity(&fixture.path("alice")).unwrap();
    let bob = config::create_identity(&fixture.path("bob")).unwrap();
    let a = config::hex(&alice.public);
    let b = config::hex(&bob.public);
    fs::create_dir_all(fixture.path("source/Folder/Empty folder")).unwrap();
    let bytes = (0..CHUNK * 12 + 713)
        .map(|n| (n % 251) as u8)
        .collect::<Vec<_>>();
    fs::write(fixture.path("source/Folder/payload.bin"), &bytes).unwrap();
    fs::write(fixture.path("source/Folder/empty.bin"), []).unwrap();
    let alice_port = port();
    let mut bob_port = port();
    while bob_port == alice_port {
        bob_port = port();
    }
    let mut receiver = spawn_actor(&fixture, "receiver-crash", bob_port, alice_port, &a);
    wait(|| fixture.path("receiver-crash.ready").exists());
    let sender = spawn_actor(&fixture, "sender-first", alice_port, bob_port, &b);
    wait(|| receiver.0.try_wait().unwrap().is_some());
    assert!(receiver.0.wait().unwrap().success());
    wait(|| {
        read_view(&fixture.path("sender-first.view"))
            .is_some_and(|v| v.phase == Phase::Paused && v.bytes >= 3 * CHUNK as u64)
    });
    drop(sender);
    drop(receiver);
    // A new process, new TCP connection and reloaded journals must retain the
    // verified prefix rather than retransferring or changing the destination.
    let _receiver = spawn_actor(&fixture, "receiver-final", bob_port, alice_port, &a);
    wait(|| fixture.path("receiver-final.ready").exists());
    let _sender = spawn_actor(&fixture, "sender-resume", alice_port, bob_port, &b);
    wait(|| {
        read_view(&fixture.path("sender-resume.view")).is_some_and(|v| v.phase == Phase::Completed)
    });
    wait(|| {
        read_view(&fixture.path("receiver-final.view")).is_some_and(|v| v.phase == Phase::Completed)
    });
    let received = read_view(&fixture.path("receiver-final.view")).unwrap();
    assert_eq!(received.paths.len(), 1);
    let root = &received.paths[0];
    assert!(root.starts_with(fixture.path("downloads")));
    assert_eq!(fs::read(root.join("payload.bin")).unwrap(), bytes);
    assert!(root.join("Empty folder").is_dir());
    assert_eq!(fs::metadata(root.join("empty.bin")).unwrap().len(), 0);
    assert!(!received.clipboard_ready && !received.clipboard_pending);
}

#[test]
fn cancelling_while_the_receiver_is_offline_survives_restart_and_cleans_its_partials() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.path("alice")).unwrap();
    fs::create_dir(fixture.path("bob")).unwrap();
    let alice = config::create_identity(&fixture.path("alice")).unwrap();
    let bob = config::create_identity(&fixture.path("bob")).unwrap();
    let a = config::hex(&alice.public);
    let b = config::hex(&bob.public);
    fs::create_dir_all(fixture.path("source/Folder")).unwrap();
    fs::write(
        fixture.path("source/Folder/payload.bin"),
        vec![9; CHUNK * 12],
    )
    .unwrap();
    let alice_port = port();
    let mut bob_port = port();
    while bob_port == alice_port {
        bob_port = port();
    }
    let mut receiver = spawn_actor(&fixture, "receiver-crash", bob_port, alice_port, &a);
    wait(|| fixture.path("receiver-crash.ready").exists());
    let sender = spawn_actor(&fixture, "sender-first", alice_port, bob_port, &b);
    wait(|| receiver.0.try_wait().unwrap().is_some());
    assert!(receiver.0.wait().unwrap().success());
    wait(|| {
        read_view(&fixture.path("sender-first.view"))
            .is_some_and(|v| v.phase == Phase::Paused && v.bytes >= 3 * CHUNK as u64)
    });
    let view = read_view(&fixture.path("sender-first.view")).unwrap();
    let export = view.id.split('.').next().unwrap();
    assert!(
        fixture
            .path(&format!("downloads/.partial-{export}"))
            .exists()
    );
    fs::write(fixture.path("sender-first.cancel"), b"cancel").unwrap();
    let record_path = fixture.path(&format!("alice/file-transfer/jobs/{}.json", view.id));
    wait(|| {
        fs::read(&record_path)
            .ok()
            .and_then(|v| serde_json::from_slice::<Record>(&v).ok())
            .is_some_and(|r| r.view.phase == Phase::Cancelled && !r.cancel_notified)
    });
    drop(sender);
    drop(receiver);
    let _receiver = spawn_actor(&fixture, "receiver-final", bob_port, alice_port, &a);
    wait(|| fixture.path("receiver-final.ready").exists());
    let _sender = spawn_actor(&fixture, "sender-resume", alice_port, bob_port, &b);
    wait(|| {
        read_view(&fixture.path("receiver-final.view")).is_some_and(|v| v.phase == Phase::Cancelled)
    });
    wait(|| {
        !fixture
            .path(&format!("downloads/.partial-{export}"))
            .exists()
    });
    wait(|| {
        fs::read(&record_path)
            .ok()
            .and_then(|v| serde_json::from_slice::<Record>(&v).ok())
            .is_some_and(|r| r.cancel_notified)
    });
    assert!(
        fixture
            .path(&format!("downloads/.cancelled-{export}.json"))
            .is_file()
    );
    assert_eq!(
        fs::read(fixture.path("source/Folder/payload.bin")).unwrap(),
        vec![9; CHUNK * 12]
    );
}

/// Separate actor fixture is inert in an ordinary test run. The parent provides
/// only a newly-created private temp directory and localhost ports.
#[test]
fn subprocess_fixture() {
    let Some(root) = std::env::var_os("DESK_SWITCH_TRANSFER_FIXTURE").map(PathBuf::from) else {
        return;
    };
    assert!(
        root.is_absolute() && root.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap())
    );
    assert!(
        root.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("desk-switch-file-service-test-")
    );
    let role = std::env::var("DESK_SWITCH_TRANSFER_ROLE").unwrap();
    let port = std::env::var("DESK_SWITCH_TRANSFER_PORT")
        .unwrap()
        .parse::<u16>()
        .unwrap();
    let peer_port = std::env::var("DESK_SWITCH_TRANSFER_PEER_PORT")
        .unwrap()
        .parse::<u16>()
        .unwrap();
    let peer = std::env::var("DESK_SWITCH_TRANSFER_PEER").unwrap();
    let workspace = "ef".repeat(32);
    if role == "receiver-crash" {
        crate::runtime()
            .unwrap()
            .block_on(crash_receiver(&root, port, &peer, &workspace))
            .unwrap();
        return;
    }
    let state = root.join(if role.starts_with("sender") {
        "alice"
    } else {
        "bob"
    });
    let own = config::hex(&config::load_identity(&state).unwrap().public);
    let service = Service::start_at(
        state,
        root.join("downloads"),
        port,
        egui::Context::default(),
    )
    .unwrap();
    service.update(TrustSnapshot {
        workspace_id: workspace.clone(),
        own,
        peers: BTreeMap::from([(
            peer.clone(),
            Peer {
                address: SocketAddr::from(([127, 0, 0, 1], peer_port - 1)),
                online: true,
            },
        )]),
        clipboard_enabled: false,
    });
    wait(|| service.core.trust.lock().unwrap().workspace_id == workspace);
    fs::write(root.join(format!("{role}.ready")), b"ready").unwrap();
    if role == "sender-first" {
        service
            .send(peer, vec![root.join("source/Folder")])
            .unwrap();
    }
    let started = Instant::now();
    let mut last = None;
    let mut cancelled = false;
    loop {
        if let Some(error) = service.error() {
            panic!("Synthetic transfer service failed: {error}");
        }
        if let Some(view) = service.snapshot().into_iter().next() {
            if !cancelled && root.join(format!("{role}.cancel")).exists() {
                service.cancel(&view.id).unwrap();
                cancelled = true;
            }
            let bytes = serde_json::to_vec(&view).unwrap();
            if last.as_ref() != Some(&bytes) {
                fs::write(root.join(format!("{role}.view")), &bytes).unwrap();
                last = Some(bytes);
            }
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "Synthetic actor did not finish"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
async fn crash_receiver(root: &Path, port: u16, peer: &str, workspace: &str) -> Result<()> {
    let listener = TcpListener::from_std(listener(port)?)?;
    fs::write(root.join("receiver-crash.ready"), b"ready")?;
    let identity = config::load_identity(&root.join("bob"))?;
    let (source, mut pipe) = transport::accept(
        listener.accept().await?.0,
        &identity,
        workspace,
        &BTreeSet::from([peer.into()]),
    )
    .await?;
    let (manifest, origin, stamp) = match pipe.receive().await? {
        Packet::Offer {
            manifest,
            origin,
            stamp,
        } => (manifest, origin, stamp),
        _ => anyhow::bail!("Expected an explicit fixture export"),
    };
    let record = Record {
        workspace: workspace.into(),
        export: manifest.id.clone(),
        stamp,
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
            id: job_id(&manifest.id, &source),
            peer: source.clone(),
            direction: Direction::Receiving,
            origin,
            name: manifest.name.clone(),
            phase: Phase::Transferring,
            bytes: 0,
            total: manifest.total,
            files: 2,
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
    config::write_atomic(
        &root
            .join("bob/file-transfer/jobs")
            .join(format!("{}.json", record.view.id)),
        &serde_json::to_string(&record)?,
    )?;
    let mut receiving =
        storage::Receive::open(&root.join("downloads"), manifest.clone(), peer, workspace)?;
    pipe.send(&Packet::Accepted).await?;
    let mut chunks = 0;
    for (index, entry) in manifest.entries.iter().enumerate() {
        let Kind::File { size, .. } = entry.kind else {
            continue;
        };
        let (mut offset, prefix) = receiving.position(index as u32)?;
        pipe.send(&Packet::Ready {
            index: index as u32,
            offset,
            prefix,
        })
        .await?;
        while offset < size {
            match pipe.receive().await? {
                Packet::Working => continue,
                Packet::Chunk {
                    index: i,
                    offset: position,
                    bytes,
                    digest,
                } => {
                    ensure!(
                        i == index as u32 && position == offset,
                        "Unexpected fixture chunk"
                    );
                    offset = receiving.write(i, position, &bytes, &digest)?;
                    pipe.send(&Packet::Ack { index: i, offset }).await?;
                    chunks += 1;
                    if chunks == 3 {
                        return Ok(());
                    }
                }
                _ => anyhow::bail!("Expected a fixture chunk"),
            }
        }
    }
    anyhow::bail!("Fixture ended before the injected interruption")
}
#[test]
fn incoming_quota_is_reserved_atomically_before_manifest_registration() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.path("alice")).unwrap();
    config::create_identity(&fixture.path("alice")).unwrap();
    let service = Service::start_at(
        fixture.path("alice"),
        fixture.path("downloads"),
        0,
        egui::Context::default(),
    )
    .unwrap();
    let start = Arc::new(std::sync::Barrier::new(3));
    let release = Arc::new(std::sync::Barrier::new(3));
    let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    std::thread::scope(|scope| {
        for suffix in ["ab", "cd"] {
            let core = service.core.clone();
            let start = start.clone();
            let release = release.clone();
            let accepted = accepted.clone();
            scope.spawn(move || {
                start.wait();
                let reservation = reserve_incoming(
                    &core,
                    &format!("{}.{}", suffix.repeat(16), "ef".repeat(32)),
                    40 * 1024 * 1024 * 1024,
                );
                if reservation.is_ok() {
                    accepted.fetch_add(1, Ordering::SeqCst);
                }
                release.wait();
                drop(reservation);
            });
        }
        start.wait();
        release.wait();
    });
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    assert!(service.core.reservations.lock().unwrap().is_empty());
}

#[test]
fn cancellation_between_the_first_marker_read_and_registration_rejects_the_new_job() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.path("bob")).unwrap();
    let bob = config::create_identity(&fixture.path("bob")).unwrap();
    let peer = "ab".repeat(32);
    let workspace = "ef".repeat(32);
    let service = Service::start_at(
        fixture.path("bob"),
        fixture.path("downloads"),
        0,
        egui::Context::default(),
    )
    .unwrap();
    service.update(TrustSnapshot {
        workspace_id: workspace.clone(),
        own: config::hex(&bob.public),
        peers: BTreeMap::from([(
            peer.clone(),
            Peer {
                address: SocketAddr::from(([127, 0, 0, 1], 45880)),
                online: true,
            },
        )]),
        clipboard_enabled: true,
    });
    wait(|| service.core.trust.lock().unwrap().workspace_id == workspace);
    for (id, origin) in [
        ("12".repeat(16), Origin::Send),
        ("34".repeat(16), Origin::Clipboard),
    ] {
        assert!(!storage::was_cancelled(&fixture.path("bob"), &workspace, &peer, &id).unwrap());
        // CancelTransfer durably records an unseen export and acknowledges it
        // while this already-validated Offer is suspended before insertion.
        storage::remember_cancel(&fixture.path("bob"), &workspace, &peer, &id).unwrap();
        let manifest = Manifest {
            version: protocol::MANIFEST_VERSION,
            id: id.clone(),
            name: "Empty folder".into(),
            roots: vec!["Empty folder".into()],
            entries: vec![protocol::Entry {
                path: vec!["Empty folder".into()],
                kind: Kind::Directory,
            }],
            total: 0,
        };
        let job = service
            .core
            .insert(Record {
                workspace: workspace.clone(),
                export: id.clone(),
                stamp: (origin == Origin::Clipboard).then(|| Stamp {
                    counter: 1,
                    author: peer.clone(),
                }),
                created: millis(),
                manifest: if origin == Origin::Send {
                    Some(manifest)
                } else {
                    None
                },
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
                    id: job_id(&id, &peer),
                    peer: peer.clone(),
                    direction: Direction::Receiving,
                    origin,
                    name: "Empty folder".into(),
                    phase: Phase::Connecting,
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
            })
            .unwrap();
        assert!(
            crate::runtime()
                .unwrap()
                .block_on(reject_cancelled_registration(
                    service.core.clone(),
                    job.clone()
                ))
                .unwrap()
        );
        assert_eq!(job.record.lock().unwrap().view.phase, Phase::Cancelled);
        assert!(job.cancel.load(Ordering::Acquire));
        assert!(
            service
                .core
                .clipboard
                .lock()
                .unwrap()
                .iter()
                .all(|event| matches!(event, ClipboardEvent::Cancel(_)))
        );
        assert!(!fixture.path(&format!("downloads/.partial-{id}")).exists());
    }
}

#[test]
fn a_newer_clipboard_copy_during_preparation_is_not_renewed_by_a_repeated_announcement() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.path("alice")).unwrap();
    fs::create_dir(fixture.path("bob")).unwrap();
    let alice = config::create_identity(&fixture.path("alice")).unwrap();
    let bob = config::create_identity(&fixture.path("bob")).unwrap();
    let a = config::hex(&alice.public);
    let b = config::hex(&bob.public);
    let workspace = "ef".repeat(32);
    let receiver_port = port();
    let service = Service::start_at(
        fixture.path("bob"),
        fixture.path("downloads"),
        receiver_port,
        egui::Context::default(),
    )
    .unwrap();
    service.update(TrustSnapshot {
        workspace_id: workspace.clone(),
        own: b.clone(),
        peers: BTreeMap::from([(
            a.clone(),
            Peer {
                address: SocketAddr::from(([127, 0, 0, 1], 45880)),
                online: true,
            },
        )]),
        clipboard_enabled: true,
    });
    wait(|| service.core.trust.lock().unwrap().workspace_id == workspace);
    let export_id = "ab".repeat(16);
    let handle = job_id(&export_id, &a);
    let stamp = Stamp {
        counter: millis(),
        author: a,
    };
    crate::runtime().unwrap().block_on(async {
        let address = SocketAddr::from(([127, 0, 0, 1], receiver_port));
        let mut pipe = transport::connect(address, &alice, &b, &workspace)
            .await
            .unwrap();
        pipe.send(&Packet::Announce {
            id: export_id.clone(),
            name: "Files".into(),
            stamp: stamp.clone(),
        })
        .await
        .unwrap();
        assert!(matches!(pipe.receive().await.unwrap(), Packet::Queued));
        let events = service
            .core
            .clipboard
            .lock()
            .unwrap()
            .drain(..)
            .collect::<Vec<_>>();
        assert!(matches!(events.as_slice(),[ClipboardEvent::Begin(id)] if id==&handle));
        // The native worker rejects the original reservation after a newer local
        // text/file copy; this is the public bridge acknowledgement, not a fake
        // delivery flag invented by the transport.
        service
            .commands
            .try_send(Command::ClipboardReady(handle.clone(), false))
            .unwrap();
        wait(|| {
            service
                .snapshot()
                .iter()
                .any(|v| v.id == handle && v.clipboard_skipped)
        });
        fs::write(
            fixture.path("copied.txt"),
            b"copied before the newer local clipboard action",
        )
        .unwrap();
        let export = storage::Export::prepare(
            &fixture.path("alice"),
            &export_id,
            &[fixture.path("copied.txt")],
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        let mut repeated = transport::connect(address, &alice, &b, &workspace)
            .await
            .unwrap();
        repeated
            .send(&Packet::Announce {
                id: export_id,
                name: "Files".into(),
                stamp: stamp.clone(),
            })
            .await
            .unwrap();
        assert!(matches!(repeated.receive().await.unwrap(), Packet::Queued));
        assert!(
            service.core.clipboard.lock().unwrap().is_empty(),
            "A reconnect must not renew the old clipboard reservation"
        );
        let mut pipe = transport::connect(address, &alice, &b, &workspace)
            .await
            .unwrap();
        pipe.send(&Packet::Offer {
            manifest: export.manifest.clone(),
            origin: Origin::Clipboard,
            stamp: Some(stamp),
        })
        .await
        .unwrap();
        loop {
            if !matches!(pipe.receive().await.unwrap(), Packet::Working) {
                break;
            }
        }
        let ready = loop {
            let packet = pipe.receive().await.unwrap();
            if !matches!(packet, Packet::Working) {
                break packet;
            }
        };
        assert!(matches!(
            ready,
            Packet::Ready {
                index: 0,
                offset: 0,
                ..
            }
        ));
        let bytes = export.read(0, 0, CHUNK).unwrap();
        let digest = protocol::hash(&bytes);
        pipe.send(&Packet::Chunk {
            index: 0,
            offset: 0,
            bytes: bytes.clone(),
            digest,
        })
        .await
        .unwrap();
        let ack = loop {
            let packet = pipe.receive().await.unwrap();
            if !matches!(packet, Packet::Working) {
                break packet;
            }
        };
        assert!(matches!(ack,Packet::Ack{index:0,offset} if offset==bytes.len()as u64));
        pipe.send(&Packet::Finish).await.unwrap();
        let done = loop {
            let packet = pipe.receive().await.unwrap();
            if !matches!(packet, Packet::Working) {
                break packet;
            }
        };
        assert!(matches!(done, Packet::Complete));
        let received = service
            .snapshot()
            .into_iter()
            .find(|v| v.id == handle)
            .unwrap();
        assert_eq!(received.phase, Phase::Completed);
        assert!(received.clipboard_skipped);
        assert!(!received.clipboard_pending && !received.clipboard_ready);
        assert_eq!(fs::read(&received.paths[0]).unwrap(), bytes);
        assert!(
            service.core.clipboard.lock().unwrap().is_empty(),
            "Saved stale files must not publish an old native selection"
        );
    });
}
