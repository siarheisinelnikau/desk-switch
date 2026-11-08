//! Temporary state, in-memory queues and authenticated loopback only. These
//! fixtures never publish a native clipboard, mount a provider or open a GUI.
use super::*;
use std::fs;

struct Harness {
    root: PathBuf,
    core: Arc<Core>,
    commands: mpsc::Receiver<Command>,
    peer: config::Identity,
}

impl Harness {
    fn new(capacity: usize) -> Self {
        let root = std::env::temp_dir().join(format!(
            "desk-switch-lazy-core-test-{}-{}",
            std::process::id(),
            new_id().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        fs::create_dir(root.join("own")).unwrap();
        fs::create_dir(root.join("peer")).unwrap();
        let identity = config::create_identity(&root.join("own")).unwrap();
        let peer = config::create_identity(&root.join("peer")).unwrap();
        let trust = TrustSnapshot {
            workspace_id: "ef".repeat(32),
            own: config::hex(&identity.public),
            peers: BTreeMap::from([(
                config::hex(&peer.public),
                Peer {
                    address: SocketAddr::from(([127, 0, 0, 1], 45880)),
                    online: true,
                },
            )]),
            clipboard_enabled: true,
        };
        let (commands, receiver) = mpsc::channel(capacity);
        let core = Arc::new(Core {
            dir: root.join("own"),
            downloads: root.join("downloads"),
            identity,
            trust: Mutex::new(trust),
            jobs: Mutex::new(BTreeMap::new()),
            clipboard: Mutex::new(VecDeque::new()),
            latest: Mutex::new(None),
            ctx: egui::Context::default(),
            slots: Arc::new(Semaphore::new(ACTIVE)),
            prepare: Arc::new(Semaphore::new(1)),
            stopping: AtomicBool::new(false),
            error: Mutex::new(None),
            notice: Mutex::new(None),
            collected: Mutex::new(BTreeSet::new()),
            garbage: Mutex::new(Vec::new()),
            reservations: Mutex::new(BTreeMap::new()),
            events: Mutex::new(VecDeque::new()),
            lazy: Mutex::new(State::default()),
            pending_results: Mutex::new(VecDeque::new()),
            commands,
        });
        Self {
            root,
            core,
            commands: receiver,
            peer,
        }
    }

    fn peer_key(&self) -> String {
        config::hex(&self.peer.public)
    }

    fn provider(&self) -> Provider {
        let id = "12".repeat(16);
        let peer = self.peer_key();
        let handle = job_id(&id, &peer);
        let remote = Arc::new(RemoteState {
            id,
            workspace: self.core.trust.lock().unwrap().workspace_id.clone(),
            peer: peer.clone(),
            stamp: Stamp {
                counter: 1,
                author: peer,
            },
            operation: Operation::Copy,
            description: description(),
            active: AtomicBool::new(true),
            retired: Mutex::new(None),
            fetch: Mutex::new(None),
        });
        {
            let mut state = self.core.lazy.lock().unwrap();
            state.remote = Some(handle.clone());
            state.remotes.insert(handle, remote.clone());
        }
        Provider {
            core: Arc::downgrade(&self.core),
            remote,
            lease: None,
        }
    }

    fn take_begin(&mut self) -> Arc<Fetch> {
        match self
            .commands
            .try_recv()
            .expect("Expected a native paste demand")
        {
            Command::NativeBegin(fetch) => fetch,
            _ => panic!("Expected a native paste demand"),
        }
    }

    fn take_result(&mut self) -> (Arc<Session>, NativeResult) {
        match self
            .commands
            .try_recv()
            .expect("Expected a native paste result")
        {
            Command::NativeResult(session, result) => (session, result),
            _ => panic!("Expected a native paste result"),
        }
    }

    fn assert_no_bytes_or_jobs(&self) {
        assert!(self.core.jobs.lock().unwrap().is_empty());
        assert!(self.core.events.lock().unwrap().is_empty());
        assert!(!self.core.dir.join("file-transfer").exists());
        assert!(!self.core.downloads.exists());
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.core.stopping.store(true, Ordering::Release);
        let temporary = std::env::temp_dir().canonicalize().unwrap();
        assert!(
            self.root.is_absolute()
                && self.root.starts_with(temporary)
                && self
                    .root
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("desk-switch-lazy-core-test-")
        );
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn description() -> Description {
    Description {
        name: "Folder".into(),
        roots: vec!["Folder".into()],
        entries: vec![
            protocol::Descriptor {
                path: vec!["Folder".into()],
                kind: DescriptorKind::Directory,
            },
            protocol::Descriptor {
                path: vec!["Folder".into(), "data.txt".into()],
                kind: DescriptorKind::File { size: 7 },
            },
        ],
        total: 7,
    }
}

fn intent() -> PasteIntent {
    PasteIntent {
        operation: Operation::Copy,
        completion: CompletionSupport::Reported,
        destination: Some("Test destination".into()),
        consumer: None,
    }
}

fn session(fetch: &Fetch, id: &str) -> Arc<Session> {
    fetch
        .sessions
        .lock()
        .unwrap()
        .get(id)
        .and_then(Weak::upgrade)
        .unwrap()
}

#[test]
fn a_full_begin_queue_can_retry_without_reusing_the_failed_fetch() {
    let mut harness = Harness::new(1);
    let provider = harness.provider();
    assert!(
        harness
            .core
            .commands
            .try_send(Command::Pause("queue sentinel".into()))
            .is_ok()
    );
    assert!(provider.begin(intent()).is_err());
    let failed = provider
        .remote
        .fetch
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .clone();
    assert!(failed.state.lock().unwrap().error.is_some());
    assert_eq!(
        harness.core.jobs.lock().unwrap()[&failed.id]
            .record
            .lock()
            .unwrap()
            .view
            .phase,
        Phase::Failed
    );
    assert!(matches!(harness.commands.try_recv(), Ok(Command::Pause(_))));

    let reader = provider.begin(intent()).unwrap();
    let retry = harness.take_begin();
    assert_ne!(retry.id, failed.id);
    assert_ne!(retry.wire, failed.wire);
    assert!(retry.state.lock().unwrap().error.is_none());
    assert!(!retry.state.lock().unwrap().cancelled);
    assert_eq!(session(&retry, reader.id()).id, reader.id());
    assert!(harness.commands.try_recv().is_err());
    assert!(!harness.core.dir.join("file-transfer/exports").exists());
}

#[test]
fn a_full_completion_queue_leaves_the_result_retryable_and_queues_it_once() {
    let mut harness = Harness::new(1);
    let provider = harness.provider();
    let reader = provider.begin(intent()).unwrap();
    let fetch = provider
        .remote
        .fetch
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .clone();
    let native = session(&fetch, reader.id());
    // NativeBegin occupies the only slot until the synthetic service drains it.
    reader
        .native_complete(NativeResult::Failed("queue full test".into()))
        .unwrap();
    assert!(native.reported.load(Ordering::Acquire));
    assert!(!native.reporting.load(Ordering::Acquire));
    assert_eq!(harness.core.pending_results.lock().unwrap().len(), 1);
    assert!(Arc::ptr_eq(&harness.take_begin(), &fetch));

    harness.core.flush_pending_results();
    let (queued, result) = harness.take_result();
    assert!(Arc::ptr_eq(&queued, &native));
    assert!(matches!(result, NativeResult::Failed(_)));
    assert!(harness.core.pending_results.lock().unwrap().is_empty());
    reader
        .native_complete(NativeResult::Failed("queue full test".into()))
        .unwrap();
    assert!(harness.commands.try_recv().is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn offline_peers_are_not_announced_and_never_make_file_sharing_fatal() {
    let harness = Harness::new(4);
    for peer in harness.core.trust.lock().unwrap().peers.values_mut() {
        peer.online = false;
    }
    let path = harness.root.join("own/source.txt");
    fs::write(&path, b"copied").unwrap();
    let generation = begin_capture(&harness.core);
    capture(
        harness.core.clone(),
        Some(Selection {
            sequence: 1,
            paths: vec![path],
            operation: Operation::Copy,
        }),
        generation,
    )
    .await
    .unwrap();
    assert!(harness.core.error.lock().unwrap().is_none());
    assert!(harness.core.notice.lock().unwrap().is_none());
    assert_eq!(harness.core.lazy.lock().unwrap().locals.len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn capturing_local_metadata_creates_neither_a_transfer_nor_a_spool() {
    let harness = Harness::new(4);
    // Announcements are irrelevant to this local capture assertion and should
    // not create a background network task in the fixture.
    harness.core.trust.lock().unwrap().peers.clear();
    let path = harness.root.join("source.txt");
    fs::write(&path, b"payload").unwrap();
    let generation = begin_capture(&harness.core);
    capture(
        harness.core.clone(),
        Some(Selection {
            sequence: 17,
            paths: vec![path.clone()],
            operation: Operation::Copy,
        }),
        generation,
    )
    .await
    .unwrap();
    let offer = {
        let state = harness.core.lazy.lock().unwrap();
        state.locals[state.local.as_ref().unwrap()].clone()
    };
    assert_eq!(offer.description.total, 7);
    assert_eq!(offer.description.entries.len(), 1);
    assert!(offer.selection.lock().unwrap().is_some());
    assert!(offer.active.load(Ordering::Acquire));
    harness.assert_no_bytes_or_jobs();
    assert_eq!(fs::read(path).unwrap(), b"payload");
}

#[tokio::test(flavor = "current_thread")]
async fn publishing_an_authenticated_metadata_offer_creates_no_data_job() {
    let mut harness = Harness::new(4);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let workspace = harness.core.trust.lock().unwrap().workspace_id.clone();
    let own = config::hex(&harness.core.identity.public);
    let peer = harness.peer_key();
    let peers = BTreeSet::from([peer.clone()]);
    let connect = transport::connect(address, &harness.peer, &own, &workspace);
    let accept = async {
        let (stream, _) = listener.accept().await?;
        transport::accept(stream, &harness.core.identity, &workspace, &peers).await
    };
    let (mut outgoing, (authenticated, mut incoming)) =
        tokio::time::timeout(Duration::from_secs(10), async {
            tokio::try_join!(connect, accept)
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(authenticated, peer);
    let id = "34".repeat(16);
    let metadata = description();
    incoming_offer(
        harness.core.clone(),
        peer.clone(),
        id.clone(),
        metadata.clone(),
        Stamp {
            counter: 1,
            author: peer.clone(),
        },
        Operation::Copy,
        &mut incoming,
    )
    .await
    .unwrap();
    assert!(matches!(outgoing.receive().await.unwrap(), Packet::Queued));
    let offer = match harness.core.clipboard.lock().unwrap().pop_front().unwrap() {
        ClipboardEvent::Offer(offer) => offer,
        _ => panic!("Expected metadata-only clipboard publication"),
    };
    assert_eq!(offer.id, job_id(&id, &peer));
    assert_eq!(offer.description, metadata);
    assert_eq!(offer.operation, Operation::Copy);
    published(&harness.core, &offer.id, true);
    harness.assert_no_bytes_or_jobs();
    assert!(harness.commands.try_recv().is_err());
    let state = harness.core.lazy.lock().unwrap();
    assert!(state.remotes[&offer.id].fetch.lock().unwrap().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_one_native_session_preserves_its_active_sibling() {
    let mut harness = Harness::new(4);
    let provider = harness.provider();
    let first = provider.begin(intent()).unwrap();
    let fetch = harness.take_begin();
    let sibling = provider.begin(intent()).unwrap();
    assert!(harness.commands.try_recv().is_err());
    assert_eq!(fetch.sessions.lock().unwrap().len(), 2);
    first.native_complete(NativeResult::Cancelled).unwrap();
    let (cancelled, result) = harness.take_result();
    native_result(harness.core.clone(), cancelled, result)
        .await
        .unwrap();
    assert!(first.is_terminal());
    assert!(!sibling.is_terminal());
    assert!(first.wait_unpaused().is_err());
    sibling.wait_unpaused().unwrap();
    let state = fetch.state.lock().unwrap();
    assert!(!state.cancelled && state.error.is_none());
    let jobs = harness.core.jobs.lock().unwrap();
    let job = &jobs[&fetch.id];
    assert!(!job.cancel.load(Ordering::Acquire));
    assert_eq!(job.record.lock().unwrap().view.phase, Phase::Preparing);
    assert!(
        !harness
            .core
            .dir
            .join("file-transfer/cancellations")
            .exists()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_a_fetch_allows_a_fresh_demand_without_a_poisoned_tombstone() {
    let mut harness = Harness::new(4);
    let provider = harness.provider();
    let old_reader = provider.begin(intent()).unwrap();
    let old = harness.take_begin();
    let job = harness.core.jobs.lock().unwrap()[&old.id].clone();
    // Keep this a local durable cancellation; no peer control task is spawned.
    cancel_job(harness.core.clone(), job, false).await.unwrap();
    assert!(old_reader.is_terminal());
    assert!(old.state.lock().unwrap().cancelled);
    assert!(
        storage::was_cancelled(
            &harness.core.dir,
            &provider.remote.workspace,
            &provider.remote.peer,
            &old.wire,
        )
        .unwrap()
    );

    let retry_reader = provider.begin(intent()).unwrap();
    let retry = harness.take_begin();
    assert_ne!(retry.id, old.id);
    assert_ne!(retry.wire, old.wire);
    assert!(!retry_reader.is_terminal());
    assert!(!retry.state.lock().unwrap().cancelled);
    assert!(retry.state.lock().unwrap().error.is_none());
    for unpoisoned in [&retry.wire, &provider.remote.id] {
        assert!(
            !storage::was_cancelled(
                &harness.core.dir,
                &provider.remote.workspace,
                &provider.remote.peer,
                unpoisoned,
            )
            .unwrap()
        );
    }
    assert_eq!(
        harness.core.jobs.lock().unwrap()[&retry.id]
            .record
            .lock()
            .unwrap()
            .view
            .phase,
        Phase::Preparing
    );
    assert!(!harness.core.dir.join("file-transfer/exports").exists());
}

#[test]
fn off_rejects_new_demand_even_from_a_previously_leased_provider() {
    let mut harness = Harness::new(4);
    let provider = harness.provider();
    let leased = provider.lease().unwrap();
    assert!(matches!(
        harness.commands.try_recv(),
        Ok(Command::AcquireLease { .. })
    ));
    harness.core.trust.lock().unwrap().clipboard_enabled = false;
    disable(&harness.core);
    assert!(provider.begin(intent()).is_err());
    assert!(provider.lease().is_err());
    assert!(leased.begin(intent()).is_err());
    harness.assert_no_bytes_or_jobs();
    assert!(harness.commands.try_recv().is_err());
}
