// Included inside net::tests, using its real authenticated Service harness.
// These fixtures never start native input, audio, clipboard, or monitor workers.

struct FailoverWorkspace {
    root: PathBuf,
    computers: Vec<Computer>,
    contexts: Vec<egui::Context>,
    keys: Vec<String>,
    identities: Vec<Vec<u8>>,
    events: Vec<(usize, Event)>,
    checked_from: usize,
}

impl FailoverWorkspace {
    fn start(count: usize) -> Self {
        Self::start_fixture(count, false, false)
    }

    fn start_with_pending_monitor(count: usize) -> Self {
        Self::start_fixture(count, true, false)
    }

    fn start_large(count: usize) -> Self {
        Self::start_fixture(count, false, true)
    }

    fn start_fixture(count: usize, pending_monitor: bool, large_roster: bool) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!(
                "qa-coordinator-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        let fixture = model::tests::fixture(count);
        let mut members = Vec::new();
        let mut directories = Vec::new();
        let mut identities = Vec::new();
        for (index, template) in fixture.members.values().enumerate() {
            let dir = root.join(index.to_string());
            std::fs::create_dir_all(&dir).unwrap();
            let identity = config::create_identity(&dir).unwrap();
            identities.push(std::fs::read(dir.join("identity")).unwrap());
            let mut member = template.clone();
            member.node.key = config::hex(&identity.public);
            member.address = "127.0.0.1:9".parse().unwrap();
            member.online = false;
            member.input_ready = 9;
            if large_roster {
                member.node.devices.speakers = (0..32)
                    .map(|device| crate::audio::AudioDevice {
                        id: format!("pc-{index}-output-{device:02}-{}", "a".repeat(1800)),
                        name: format!("Synthetic output {device:02}"),
                        default: device == 0,
                    })
                    .collect();
            }
            member.node.management = Some(workspace::management::State {
                application: workspace::management::Application {
                    theme: "dark".into(),
                    language: "en".into(),
                    autostart: false,
                    auto_connect: false,
                    auto_update: false,
                    show_remote_cursor: true,
                },
                update: Default::default(),
            });
            members.push(member);
            directories.push(dir);
        }
        // The original coordinator has no special physical input role. The
        // surviving peers can keep using their remembered source and target.
        let keyboards = std::mem::take(&mut members[0].node.devices.keyboards);
        let mice = std::mem::take(&mut members[0].node.devices.mice);
        members[0].node.hardware.keyboard = false;
        members[0].node.hardware.mouse = false;
        members[1].node.devices.keyboards = keyboards;
        members[1].node.devices.mice = mice;
        members[1].node.hardware.keyboard = true;
        members[1].node.hardware.mouse = true;
        if pending_monitor {
            members[count - 1].input_ready = 0;
        }
        let keys = members
            .iter()
            .map(|member| member.node.key.clone())
            .collect::<Vec<_>>();
        let target = keys[count - 1].clone();
        let mut stored = fixture;
        stored.hub = keys[0].clone();
        stored.revision = 40;
        stored.activation = 9;
        stored.input_request = 9;
        stored.active_input = Some(target.clone());
        stored.input_intent = Some(target.clone());
        stored.active_monitor = Some(target.clone());
        stored.confirmed_monitor = Some(if pending_monitor {
            keys[1].clone()
        } else {
            target.clone()
        });
        if pending_monitor {
            stored.monitor_command = Some(model::MonitorCommand {
                id: 9,
                writer: keys[1].clone(),
                destination: target,
                connection: members[1].node.monitor.id.clone(),
                input: members[count - 1].node.monitor.input.unwrap(),
                deadline_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64
                    + 60_000,
                interrupted: None,
                result: None,
            });
        }
        stored.settings.input = workspace::Selection::Computer(keys[1].clone());
        stored.settings.monitor_enabled = true;
        stored.members = members
            .iter()
            .map(|member| (member.node.key.clone(), member.clone()))
            .collect();
        stored.validate().unwrap();
        if large_roster {
            assert!(serde_json::to_vec(&stored).unwrap().len() > usize::from(u16::MAX));
        }
        for dir in &directories {
            save(dir, &stored).unwrap();
        }
        let contexts = (0..count)
            .map(|_| egui::Context::default())
            .collect::<Vec<_>>();
        let computers = members
            .into_iter()
            .zip(directories)
            .enumerate()
            .map(|(index, (member, dir))| Computer {
                service: Service::start(dir.clone(), member.clone(), 0, contexts[index].clone())
                    .unwrap(),
                state: None,
                dir,
                member,
            })
            .collect();
        let mut result = Self {
            root,
            computers,
            contexts,
            keys,
            identities,
            events: Vec::new(),
            checked_from: 0,
        };
        result.wait(|computers, _| computers.iter().all(|computer| computer.state.is_some()));
        // Existing trusted peers can be connected by address before the first
        // majority election. This does not admit any new identity to the roster.
        for index in 1..count {
            let address = result.state(index).members[&result.keys[index]].address;
            result.computers[0]
                .service
                .action(Action::Invite {
                    address,
                    key: result.keys[index].clone(),
                })
                .unwrap();
        }
        let alive = (0..count).collect::<Vec<_>>();
        result.wait_ready(&alive);
        result
    }

    fn state(&self, index: usize) -> &Workspace {
        self.computers[index].state.as_ref().unwrap()
    }

    fn wait(&mut self, condition: impl Fn(&[Computer], &[(usize, Event)]) -> bool) {
        let checked_from = self.checked_from;
        until(
            &mut self.computers,
            &mut self.events,
            |computers, events| {
                failover_assert_no_errors(&events[checked_from..]);
                condition(computers, events)
            },
        );
    }

    fn wait_ready(&mut self, alive: &[usize]) {
        let keys = self.keys.clone();
        self.wait(|computers, events| {
            let Some(first) = computers[alive[0]].state.as_ref() else {
                return false;
            };
            let Some(leader) = keys.iter().position(|key| key == &first.hub) else {
                return false;
            };
            alive.contains(&leader)
                && failover_coordination(events, leader).is_some_and(|(hub, writable, _, _)| {
                    hub.as_deref() == Some(first.hub.as_str()) && writable
                })
                && alive.iter().all(|index| {
                    computers[*index].state.as_ref().is_some_and(|state| {
                        state.authority.is_some()
                            && state.authority == first.authority
                            && state.activation == first.activation
                            && state.workspace_id() == keys[0]
                            && state.hub == first.hub
                            && state.members.len() == keys.len()
                            && alive.iter().all(|peer| state.members[&keys[*peer]].online)
                            // Writable leadership precedes automatic local
                            // fail-open/initial intent commits. Sequential CAS
                            // edits wait for that input state to converge.
                            && state.active_input.as_ref().is_none_or(|target| {
                                state.members[target].compatible()
                            })
                            // A reachable remembered target may have just
                            // returned in raw telemetry. Its automatic restore
                            // must commit before this snapshot is a CAS base.
                            && state.input_intent.as_ref().is_none_or(|target| {
                                !state.members[target].compatible()
                                    || state.members[target].node.monitor_error.is_some()
                                    || state.active_input.as_ref() == Some(target)
                            })
                            && (state.plan().input.is_none()
                                || (state.active_input.is_some() && state.input_intent.is_some()))
                    })
                })
        });
    }

    fn stop(&self, index: usize) {
        self.computers[index].service.stop();
    }

    fn restart(&mut self, index: usize) {
        self.computers[index].service.stop();
        self.wait(|computers, _| computers[index].service.commands.is_closed());
        // A normal application restart has automatic reconnect enabled. The
        // initial disabled setting above only keeps fixture discovery explicit.
        let computer = &mut self.computers[index];
        computer
            .member
            .node
            .management
            .as_mut()
            .unwrap()
            .application
            .auto_connect = true;
        computer.state = None;
        self.contexts[index] = egui::Context::default();
        computer.service = Service::start(
            computer.dir.clone(),
            computer.member.clone(),
            0,
            self.contexts[index].clone(),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(computer.dir.join("identity")).unwrap(),
            self.identities[index],
            "A returning coordinator must retain its pinned identity"
        );
    }

    fn clipboard(&mut self, sender: usize, value: bool, alive: &[usize]) {
        let mut settings = self.state(sender).settings.clone();
        settings.clipboard_enabled = value;
        self.computers[sender]
            .service
            .action(Action::Settings {
                expected: self.state(sender).revision,
                value: settings,
            })
            .unwrap();
        self.wait(|computers, _| {
            alive.iter().all(|index| {
                computers[*index]
                    .state
                    .as_ref()
                    .is_some_and(|state| state.settings.clipboard_enabled == value)
            })
        });
    }

    fn observe(&mut self, duration: Duration, condition: impl Fn(&Self) -> bool) {
        let start = Instant::now();
        loop {
            pump(&mut self.computers, &mut self.events);
            failover_assert_no_errors(&self.events[self.checked_from..]);
            assert!(
                condition(self),
                "State changed unexpectedly: {:?}",
                self.events
            );
            if start.elapsed() >= duration {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn finish(&mut self) {
        for computer in &self.computers {
            computer.service.stop();
        }
        drop(std::mem::take(&mut self.computers));
        let start = Instant::now();
        loop {
            match std::fs::remove_dir_all(&self.root) {
                Ok(()) => return,
                Err(_) if start.elapsed() < Duration::from_secs(2) => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("Could not remove the isolated fixture directory: {error}"),
            }
        }
    }
}

impl Drop for FailoverWorkspace {
    fn drop(&mut self) {
        for computer in &self.computers {
            computer.service.stop();
        }
    }
}

fn failover_assert_no_errors(events: &[(usize, Event)]) {
    assert!(
        !events
            .iter()
            .any(|(_, event)| matches!(event, Event::Error(_))),
        "Normal trusted-peer recovery produced a hard error: {events:?}"
    );
}

fn failover_coordination(
    events: &[(usize, Event)],
    computer: usize,
) -> Option<(Option<String>, bool, usize, usize)> {
    events.iter().rev().find_map(|(index, event)| {
        if *index == computer
            && let Event::Coordination {
                leader,
                writable,
                online,
                quorum,
            } = event
        {
            Some((leader.clone(), *writable, *online, *quorum))
        } else {
            None
        }
    })
}

fn failover_cas_conflicts(events: &[(usize, Event)], writers: &[usize]) -> usize {
    events
        .iter()
        .filter(|(index, event)| {
            if let Event::Error(error) = event {
                assert!(
                    writers.contains(index) && error.to_ascii_lowercase().contains("changed"),
                    "Concurrent edits produced an unrelated error: {event:?}"
                );
                true
            } else {
                false
            }
        })
        .count()
}

struct PendingAdmissionFixture {
    root: PathBuf,
    directories: Vec<PathBuf>,
    members: Vec<Member>,
    identities: Vec<Vec<u8>>,
}

impl PendingAdmissionFixture {
    fn prepare() -> Self {
        use crate::group::consensus::{Engine, HardState};
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!(
                "qa-pending-admission-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        let templates = model::tests::fixture(2);
        let mut directories = Vec::new();
        let mut members = Vec::new();
        let mut identities = Vec::new();
        for (index, template) in templates.members.values().enumerate() {
            let dir = root.join(index.to_string());
            std::fs::create_dir_all(&dir).unwrap();
            let identity = config::create_identity(&dir).unwrap();
            identities.push(std::fs::read(dir.join("identity")).unwrap());
            let mut member = template.clone();
            member.node.key = config::hex(&identity.public);
            member.address = "127.0.0.1:9".parse().unwrap();
            member.online = false;
            member.node.management = Some(workspace::management::State {
                application: workspace::management::Application {
                    theme: "dark".into(),
                    language: "en".into(),
                    autostart: false,
                    auto_connect: false,
                    auto_update: false,
                    show_remote_cursor: true,
                },
                update: Default::default(),
            });
            let mut cache = model::tests::fixture(1);
            cache.hub = member.node.key.clone();
            cache.members = [(member.node.key.clone(), member.clone())].into();
            cache.validate().unwrap();
            save(&dir, &cache).unwrap();
            members.push(member);
            directories.push(dir);
        }
        let holder = members[0].node.key.clone();
        let learner = members[1].node.key.clone();
        let initial: Workspace =
            serde_json::from_slice(&std::fs::read(directories[0].join("workspace.json")).unwrap())
                .unwrap();
        let mut engine = Engine::new(
            holder.clone(),
            HardState::bootstrap(
                holder.clone(),
                [holder.clone()].into(),
                serde_json::to_vec(&initial).unwrap(),
            )
            .unwrap(),
            0,
        )
        .unwrap();
        engine.tick(0).unwrap();
        assert!(engine.writable());
        let prefix = engine.export();
        let mut proposed = initial;
        proposed.revision += 1;
        proposed.members.insert(learner.clone(), members[1].clone());
        proposed.validate().unwrap();
        // No RPC from begin_add is delivered. These images represent both
        // processes crashing before the learner saw its first joint entry.
        engine
            .begin_add(learner.clone(), serde_json::to_vec(&proposed).unwrap(), 1)
            .unwrap();
        assert!(engine.busy());
        assert!(engine.export_pending_joint().is_some());
        assert_eq!(engine.hard_state().commit_index, prefix.commit_index);
        assert_eq!(
            engine.hard_state().entries.last().unwrap().index,
            prefix.commit_index + 1
        );
        assert!(!prefix.configuration.members().contains(&learner));
        for (index, hard) in [engine.hard_state(), &prefix].into_iter().enumerate() {
            // Consent/trust is recorded before the first foreign voting image,
            // matching the production ordering independently of the UI cache.
            config::write_atomic(
                &directories[index].join("workspace-admission.json"),
                &serde_json::json!({"own":members[index].node.key,"workspace_id":holder})
                    .to_string(),
            )
            .unwrap();
            config::write_atomic(
                &directories[index].join("workspace-consensus.json"),
                &serde_json::to_string(hard).unwrap(),
            )
            .unwrap();
        }
        Self {
            root,
            directories,
            members,
            identities,
        }
    }

    fn start_all(self) -> FailoverWorkspace {
        let contexts = (0..2).map(|_| egui::Context::default()).collect::<Vec<_>>();
        let keys = self
            .members
            .iter()
            .map(|member| member.node.key.clone())
            .collect::<Vec<_>>();
        let computers = self
            .members
            .into_iter()
            .zip(self.directories)
            .enumerate()
            .map(|(index, (member, dir))| Computer {
                service: Service::start(dir.clone(), member.clone(), 0, contexts[index].clone())
                    .unwrap(),
                state: None,
                dir,
                member,
            })
            .collect();
        FailoverWorkspace {
            root: self.root,
            computers,
            contexts,
            keys,
            identities: self.identities,
            events: Vec::new(),
            checked_from: 0,
        }
    }
}

#[test]
fn restarted_holder_and_learner_heal_an_admission_lost_before_first_joint_delivery() {
    use crate::group::consensus::HardState;
    let mut group = PendingAdmissionFixture::prepare().start_all();
    group.wait(|computers, _| computers.iter().all(|computer| computer.state.is_some()));
    group.observe(Duration::from_millis(400), |group| {
        group
            .computers
            .iter()
            .all(|computer| computer.state.as_ref().unwrap().members.len() == 1)
            && group
                .events
                .iter()
                .all(|(_, event)| !matches!(event, Event::Coordination { writable: true, .. }))
    });
    let address = group.state(1).members[&group.keys[1]].address;
    group.computers[0]
        .service
        .action(Action::Invite {
            address,
            key: group.keys[1].clone(),
        })
        .unwrap();
    group.wait_ready(&[0, 1]);
    assert!(group.state(0).active_input.is_some());
    assert!(group.state(0).input_intent.is_some());
    group.clipboard(1, true, &[0, 1]);
    for index in 0..2 {
        let hard: HardState = serde_json::from_slice(
            &std::fs::read(group.computers[index].dir.join("workspace-consensus.json")).unwrap(),
        )
        .unwrap();
        hard.validate().unwrap();
        assert_eq!(hard.workspace_id, group.keys[0]);
        assert_eq!(
            hard.configuration.members(),
            group.keys.iter().cloned().collect()
        );
        assert!(hard.configuration.joint.is_none());
        assert!(hard.commit_index >= 2);
        assert_eq!(
            std::fs::read(group.computers[index].dir.join("identity")).unwrap(),
            group.identities[index]
        );
        assert!(group.state(index).settings.clipboard_enabled);
        assert!(group.state(index).monitor_command.is_none());
    }
    group.finish();
}

#[test]
fn uncommitted_learner_keeps_foreign_vote_state_and_missing_vote_record_fails_closed() {
    use crate::group::consensus::HardState;
    let fixture = PendingAdmissionFixture::prepare();
    let dir = &fixture.directories[1];
    let vote = dir.join("workspace-consensus.json");
    let marker = dir.join("workspace-admission.json");
    let cache = dir.join("workspace.json");
    let original_vote = std::fs::read(&vote).unwrap();
    let original_marker = std::fs::read(&marker).unwrap();
    let original_cache = std::fs::read(&cache).unwrap();
    let hard: HardState = serde_json::from_slice(&original_vote).unwrap();
    assert_eq!(hard.workspace_id, fixture.members[0].node.key);
    assert!(
        !hard
            .configuration
            .members()
            .contains(&fixture.members[1].node.key)
    );
    assert!(
        serde_json::from_slice::<Workspace>(&original_cache)
            .unwrap()
            .authority
            .is_none()
    );
    for _ in 0..2 {
        let service = Service::start(
            dir.clone(),
            fixture.members[1].clone(),
            0,
            egui::Context::default(),
        )
        .unwrap();
        let began = Instant::now();
        let mut saw_cache = false;
        while began.elapsed() < Duration::from_millis(400) || !saw_cache {
            assert!(began.elapsed() < Duration::from_secs(5));
            for event in service.poll() {
                match event {
                    Event::State(state) => {
                        assert!(state.authority.is_none());
                        assert_eq!(state.hub, fixture.members[1].node.key);
                        assert_eq!(state.members.len(), 1);
                        saw_cache = true;
                    }
                    Event::Coordination { writable, .. } => assert!(!writable),
                    other => panic!("An isolated uncommitted learner changed authority: {other:?}"),
                }
            }
            assert!(!service.commands.is_closed());
            std::thread::sleep(Duration::from_millis(10));
        }
        service.stop();
        let stopped = Instant::now();
        while !service.commands.is_closed() {
            assert!(stopped.elapsed() < Duration::from_secs(5));
            assert!(
                !service
                    .poll()
                    .iter()
                    .any(|event| matches!(event, Event::Error(_)))
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(service);
        assert_eq!(std::fs::read(&vote).unwrap(), original_vote);
        assert_eq!(std::fs::read(&marker).unwrap(), original_marker);
        assert_eq!(std::fs::read(&cache).unwrap(), original_cache);
        assert_eq!(
            std::fs::read(dir.join("identity")).unwrap(),
            fixture.identities[1]
        );
    }
    std::fs::remove_file(&vote).unwrap();
    let service = Service::start(
        dir.clone(),
        fixture.members[1].clone(),
        0,
        egui::Context::default(),
    )
    .unwrap();
    let began = Instant::now();
    let mut errors = Vec::new();
    loop {
        for event in service.poll() {
            match event {
                Event::Error(error) => errors.push(error),
                other => panic!("A missing voting record created a new workspace: {other:?}"),
            }
        }
        if service.commands.is_closed() && !errors.is_empty() {
            break;
        }
        assert!(began.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0]
            .to_ascii_lowercase()
            .contains("voting record is missing")
    );
    assert!(!vote.exists());
    assert_eq!(std::fs::read(&marker).unwrap(), original_marker);
    assert_eq!(std::fs::read(&cache).unwrap(), original_cache);
    assert_eq!(
        std::fs::read(dir.join("identity")).unwrap(),
        fixture.identities[1]
    );
    drop(service);
    std::fs::remove_dir_all(fixture.root).unwrap();
}

#[test]
fn steady_three_member_mesh_has_no_gui_wakes_events_or_durable_vote_writes() {
    use crate::group::consensus::HardState;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let mut group = FailoverWorkspace::start(3);
    group.wait(|computers, _| {
        let authority = computers[0].state.as_ref().unwrap().authority.as_ref();
        authority.is_some()
            && computers
                .iter()
                .all(|computer| computer.state.as_ref().unwrap().authority.as_ref() == authority)
    });
    // Allow the last topology observations and automatic input reconciliation
    // to settle before measuring the independent control heartbeat traffic.
    group.observe(Duration::from_millis(800), |_| true);
    pump(&mut group.computers, &mut group.events);
    let counters = group
        .contexts
        .iter()
        .map(|ctx| {
            let counter = Arc::new(AtomicUsize::new(0));
            let waking = counter.clone();
            ctx.set_request_repaint_callback(move |_| {
                waking.fetch_add(1, Ordering::SeqCst);
            });
            // egui0.35 starts with one outstanding repaint. A single ordinary
            // pass leaves its delay at zero and would suppress later callbacks.
            // One finite run with a second empty bookkeeping pass clears both
            // fields; the measurement below never runs another frame.
            let mut first_pass = true;
            let output = ctx.run_ui(Default::default(), |ui| {
                if first_pass {
                    first_pass = false;
                    ui.ctx()
                        .request_discard("Consume initial repaint bookkeeping");
                }
            });
            assert_eq!(output.platform_output.num_completed_passes, 2);
            assert!(
                !ctx.has_requested_repaint(),
                "Initial repaint was not consumed"
            );
            assert!(
                counter.load(Ordering::SeqCst) > 0,
                "The wake callback was not armed"
            );
            counter
        })
        .collect::<Vec<_>>();
    // State notifications queued before the measurement are deliberately
    // drained after the finite setup pass, never during a measured UI frame.
    pump(&mut group.computers, &mut group.events);
    failover_assert_no_errors(&group.events[group.checked_from..]);
    let baselines = group
        .computers
        .iter()
        .map(|computer| {
            let path = computer.dir.join("workspace-consensus.json");
            let bytes = std::fs::read(&path).unwrap();
            let hard: HardState = serde_json::from_slice(&bytes).unwrap();
            hard.validate().unwrap();
            let tail = hard
                .entries
                .last()
                .map_or(hard.snapshot_index, |entry| entry.index);
            assert_eq!(
                tail, hard.commit_index,
                "The idle baseline has an uncommitted tail"
            );
            assert!(
                hard.configuration.joint.is_none(),
                "Membership is still changing"
            );
            let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
            (path, bytes, modified)
        })
        .collect::<Vec<_>>();
    let wakes = counters
        .iter()
        .map(|counter| counter.load(Ordering::SeqCst))
        .collect::<Vec<_>>();
    let started = Instant::now();
    loop {
        for (index, computer) in group.computers.iter().enumerate() {
            let events = computer.service.poll();
            assert!(
                events.is_empty(),
                "Steady peer {index} emitted a state, coordination, error or reconnect notification: {events:?}"
            );
            assert_eq!(
                counters[index].load(Ordering::SeqCst),
                wakes[index],
                "A steady control heartbeat woke peer {index}'s GUI"
            );
            assert!(!group.contexts[index].has_requested_repaint());
        }
        if started.elapsed() >= Duration::from_millis(2_400) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // Six consensus heartbeat intervals and multiple peer pings passed without
    // a single application event, repaint callback or durable vote/log write.
    for (path, bytes, modified) in baselines {
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "Idle voting data changed"
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            modified,
            "Idle control traffic rewrote the voting file"
        );
    }
    group.finish();
}

#[test]
fn large_bootstrap_append_and_snapshot_preserve_authenticated_stream_continuity() {
    use crate::group::consensus::{Engine, HardState, Message, Rpc};
    let mut group = FailoverWorkspace::start_large(3);
    let leader = group
        .keys
        .iter()
        .position(|key| key == &group.state(0).hub)
        .unwrap();
    let follower = (0..3).find(|index| *index != leader).unwrap();
    // Reaching readiness already transported the large migration bootstrap.
    // This first change also replicates a full application payload in Append.
    group.clipboard(leader, true, &[0, 1, 2]);
    group.wait(|computers, _| {
        let authority = computers[leader].state.as_ref().unwrap().authority.as_ref();
        computers
            .iter()
            .all(|computer| computer.state.as_ref().unwrap().authority.as_ref() == authority)
    });
    let durable: HardState = serde_json::from_slice(
        &std::fs::read(group.computers[leader].dir.join("workspace-consensus.json")).unwrap(),
    )
    .unwrap();
    let image = Engine::new(group.keys[leader].clone(), durable, 0)
        .unwrap()
        .export();
    let committed: Workspace = serde_json::from_slice(&image.snapshot).unwrap();
    assert!(committed.settings.clipboard_enabled);
    assert!(image.snapshot.len() > usize::from(u16::MAX));
    let snapshot = Packet::Consensus(Rpc {
        workspace_id: image.workspace_id,
        term: image.term,
        message: Message::Snapshot {
            index: image.snapshot_index,
            last_term: image.snapshot_term,
            configuration: image.configuration,
            data: image.snapshot,
        },
    });
    let length = serde_json::to_vec(&snapshot).unwrap().len();
    assert!(length > usize::from(u16::MAX));
    assert!(length < MAX_PACKET);
    group.computers[leader]
        .service
        .commands
        .try_send(Command::Raw(
            group.keys[follower].clone(),
            Box::new(snapshot),
        ))
        .unwrap();
    // A following ordinary packet is a transport barrier: the receiver cannot
    // deliver it until every encrypted record of Snapshot has been consumed.
    // Service only emits this management event; no application updater runs.
    const BARRIER: u64 = 65_070;
    let started = group.events.len();
    group.computers[leader]
        .service
        .action(Action::Manage {
            target: group.keys[follower].clone(),
            message: workspace::management::Message::CheckUpdates { id: BARRIER },
        })
        .unwrap();
    let source = group.keys[leader].clone();
    group.wait(|_, events| {
        events[started..].iter().any(|(index, event)| {
            *index == follower
                && matches!(event, Event::Management {
                    source: received,
                    message: workspace::management::Message::CheckUpdates { id: BARRIER },
                } if received == &source)
        })
    });
    group.clipboard(follower, false, &[0, 1, 2]);
    assert!(group.computers.iter().all(|computer| {
        let state = computer.state.as_ref().unwrap();
        !state.settings.clipboard_enabled
            && state
                .members
                .values()
                .all(|member| member.online && member.node.devices.speakers.len() == 32)
    }));
    group.finish();
}

#[test]
fn commit_storage_failure_stops_only_that_voter_without_publishing_partial_state() {
    use crate::group::consensus::{Engine, HardState};
    let mut group = FailoverWorkspace::start(3);
    group.wait(|computers, _| {
        let authority = computers[0].state.as_ref().unwrap().authority.as_ref();
        authority.is_some()
            && computers
                .iter()
                .all(|computer| computer.state.as_ref().unwrap().authority.as_ref() == authority)
    });
    let leader = group
        .keys
        .iter()
        .position(|key| key == &group.state(0).hub)
        .unwrap();
    let follower = (0..3).find(|index| *index != leader).unwrap();
    let healthy = (0..3)
        .filter(|index| *index != follower)
        .collect::<Vec<_>>();
    let intent = group.state(follower).input_intent.clone();
    assert_eq!(intent, Some(group.keys[2].clone()));
    for index in 0..3 {
        assert_eq!(group.state(index).active_input, intent);
    }
    let (arm, armed) = std::sync::mpsc::channel();
    group.computers[follower]
        .service
        .commands
        .try_send(Command::FailCommitPersistence(arm))
        .unwrap();
    armed
        .recv_timeout(Duration::from_secs(2))
        .expect("The follower did not arm its one-shot commit persistence failure");
    let started = group.events.len();
    let mut settings = group.state(leader).settings.clone();
    assert!(!settings.clipboard_enabled);
    settings.clipboard_enabled = true;
    group.computers[leader]
        .service
        .action(Action::Settings {
            expected: group.state(leader).revision,
            value: settings,
        })
        .unwrap();
    until(
        &mut group.computers,
        &mut group.events,
        |computers, events| {
            assert!(
                !computers[follower]
                    .state
                    .as_ref()
                    .unwrap()
                    .settings
                    .clipboard_enabled,
                "The failing voter published a commit before its storage succeeded"
            );
            let errors = events[started..]
                .iter()
                .filter_map(|(index, event)| match event {
                    Event::Error(error) => Some((*index, error)),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert!(
                errors.iter().all(|(index, error)| {
                    *index == follower && error.to_ascii_lowercase().contains("storage")
                }),
                "A local storage failure was misreported to another peer: {errors:?}"
            );
            assert!(
                errors.len() <= 1,
                "The stopped voter reported its failure repeatedly"
            );
            computers[follower].service.commands.is_closed()
                && errors.len() == 1
                && healthy.iter().all(|index| {
                    computers[*index]
                        .state
                        .as_ref()
                        .unwrap()
                        .settings
                        .clipboard_enabled
                })
        },
    );
    group.checked_from = group.events.len();
    group.wait_ready(&healthy);
    group.observe(Duration::from_millis(250), |group| {
        group.computers[follower].service.commands.is_closed()
            && !group.state(follower).settings.clipboard_enabled
            && healthy
                .iter()
                .all(|index| group.state(*index).settings.clipboard_enabled)
    });

    // The failpoint is transient and private to the stopped process. A fresh
    // instance must load the valid pre-failure image and catch up normally.
    group.restart(follower);
    group.wait_ready(&[0, 1, 2]);
    assert!(group.state(follower).settings.clipboard_enabled);
    assert_eq!(group.state(follower).input_intent, intent);
    let durable: HardState = serde_json::from_slice(
        &std::fs::read(
            group.computers[follower]
                .dir
                .join("workspace-consensus.json"),
        )
        .unwrap(),
    )
    .unwrap();
    durable.validate().unwrap();
    let image = Engine::new(group.keys[follower].clone(), durable, 0)
        .unwrap()
        .export();
    let committed: Workspace = serde_json::from_slice(&image.snapshot).unwrap();
    committed.validate().unwrap();
    assert!(committed.settings.clipboard_enabled);
    assert_eq!(committed.input_intent, intent);
    group.finish();
}

#[test]
fn malformed_authenticated_consensus_payload_is_rejected_before_durable_storage() {
    use crate::group::consensus::{Entry, HardState, Message, Rpc};
    let mut group = FailoverWorkspace::start(3);
    group.wait(|computers, _| {
        let authority = computers[0].state.as_ref().unwrap().authority.as_ref();
        authority.is_some()
            && computers
                .iter()
                .all(|computer| computer.state.as_ref().unwrap().authority.as_ref() == authority)
    });
    let leader = group
        .keys
        .iter()
        .position(|key| key == &group.state(0).hub)
        .unwrap();
    let follower = (0..3).find(|index| *index != leader).unwrap();
    let authority = group.state(leader).authority.clone().unwrap();
    let next_index = authority.index.checked_add(1).unwrap();
    let started = group.events.len();
    group.computers[leader]
        .service
        .commands
        .try_send(Command::Raw(
            group.keys[follower].clone(),
            Box::new(Packet::Consensus(Rpc {
                workspace_id: authority.workspace_id,
                term: authority.term,
                message: Message::Append {
                    prev_index: authority.index,
                    prev_term: authority.term,
                    entries: vec![Entry {
                        index: next_index,
                        term: authority.term,
                        data: b"{".to_vec(),
                        configuration: None,
                        barrier: false,
                    }],
                    leader_commit: next_index,
                },
            })),
        ))
        .unwrap();
    until(&mut group.computers, &mut group.events, |_, events| {
        events[started..].iter().any(|(index, event)| {
            *index == leader && matches!(event, Event::Error(error) if !error.is_empty())
        })
    });
    let errors = group.events[started..]
        .iter()
        .filter(|(_, event)| matches!(event, Event::Error(_)))
        .collect::<Vec<_>>();
    assert_eq!(
        errors.len(),
        1,
        "Malformed input must report one useful error: {errors:?}"
    );
    assert_eq!(
        errors[0].0, leader,
        "The receiver must reject the sender's packet"
    );

    // Inspect the durable image immediately, before a later valid append could
    // overwrite the bad entry and hide an acknowledge-before-validation bug.
    let durable: HardState = serde_json::from_slice(
        &std::fs::read(
            group.computers[follower]
                .dir
                .join("workspace-consensus.json"),
        )
        .unwrap(),
    )
    .unwrap();
    durable.validate().unwrap();
    serde_json::from_slice::<Workspace>(&durable.snapshot)
        .unwrap()
        .validate()
        .unwrap();
    for entry in durable.entries.iter().filter(|entry| !entry.barrier) {
        serde_json::from_slice::<Workspace>(&entry.data)
            .expect("An invalid application payload reached the durable log")
            .validate()
            .unwrap();
    }
    group.checked_from = group.events.len();
    group.observe(Duration::from_millis(250), |group| {
        group.computers.iter().all(|computer| {
            let state = computer.state.as_ref().unwrap();
            !state.settings.clipboard_enabled && state.members.values().all(|member| member.online)
        })
    });
    group.clipboard(follower, true, &[0, 1, 2]);
    group.stop(follower);
    group.restart(follower);
    group.wait_ready(&[0, 1, 2]);
    for index in 0..3 {
        assert!(group.state(index).settings.clipboard_enabled);
        group.state(index).validate().unwrap();
    }
    group.finish();
}

#[test]
fn pending_monitor_command_survives_election_without_reissue_or_early_input_readiness() {
    let mut group = FailoverWorkspace::start_with_pending_monitor(3);
    let command = group.state(1).monitor_command.clone().unwrap();
    let activation = group.state(1).activation;
    let input_request = group.state(1).input_request;
    let active = group.state(1).active_input.clone();
    let intent = group.state(1).input_intent.clone();
    let monitor = group.state(1).active_monitor.clone();
    let confirmed = group.state(1).confirmed_monitor.clone();
    assert_eq!(command.id, 9);
    assert_eq!(activation, 9);
    assert_eq!(group.state(1).members[&command.destination].input_ready, 0);
    group.stop(0);
    group.wait_ready(&[1, 2]);
    for index in [1, 2] {
        let state = group.state(index);
        assert!(state.switch_pending());
        assert_eq!(state.monitor_command.as_ref(), Some(&command));
        assert_eq!(state.activation, activation);
        assert_eq!(state.input_request, input_request);
        assert_eq!(state.active_input, active);
        assert_eq!(state.input_intent, intent);
        assert_eq!(state.active_monitor, monitor);
        assert_eq!(state.confirmed_monitor, confirmed);
        assert_eq!(state.members[&command.destination].input_ready, 0);
        state.validate().unwrap();
    }
    let started = group.events.len();
    group.computers[1]
        .service
        .action(Action::Activate {
            key: group.keys[1].clone(),
        })
        .unwrap();
    until(&mut group.computers, &mut group.events, |_, events| {
        events[started..].iter().any(|(index, event)| {
            *index == 1
                && matches!(event, Event::Error(error) if error.to_ascii_lowercase().contains("switch"))
        })
    });
    let errors = group.events[started..]
        .iter()
        .filter(|(_, event)| matches!(event, Event::Error(_)))
        .collect::<Vec<_>>();
    assert_eq!(
        errors.len(),
        1,
        "A pending command should reject one explicit switch: {errors:?}"
    );
    assert_eq!(errors[0].0, 1);
    group.checked_from = group.events.len();
    group.observe(Duration::from_millis(500), |group| {
        [1, 2].iter().all(|index| {
            let state = group.state(*index);
            state.monitor_command.as_ref() == Some(&command)
                && state.activation == activation
                && state.input_request == input_request
                && state.active_input == active
                && state.input_intent == intent
                && state.members[&command.destination].input_ready == 0
        })
    });
    group.finish();
}

#[test]
fn five_member_majority_serializes_competing_edits_and_preserves_only_committed_winners() {
    let mut group = FailoverWorkspace::start(5);
    let alive = [1, 2, 4];
    let original = group.keys[0].clone();
    let intent = group.state(1).input_intent.clone();
    let monitor = group.state(1).active_monitor.clone();
    let confirmed = group.state(1).confirmed_monitor.clone();
    group.stop(0);
    group.stop(3);
    group.wait_ready(&alive);
    for index in alive {
        assert_eq!(group.state(index).active_input, intent);
    }
    let keys = group.keys.clone();
    group.wait(|computers, _| {
        let revision = computers[alive[0]].state.as_ref().unwrap().revision;
        alive
            .iter()
            .all(|index| computers[*index].state.as_ref().unwrap().revision == revision)
    });
    let leader = group.state(1).hub.clone();
    let writers = alive
        .into_iter()
        .filter(|index| keys[*index] != leader)
        .collect::<Vec<_>>();
    assert_eq!(writers.len(), 2);
    let before = group.state(writers[0]).settings.clone();
    let revision = group.state(writers[0]).revision;
    let mut first = before.clone();
    first.clipboard_enabled = !first.clipboard_enabled;
    let mut second = before.clone();
    second.edge_delay_ms = if second.edge_delay_ms == 225 {
        275
    } else {
        225
    };
    let started = group.events.len();
    for (sender, value) in [(writers[0], first.clone()), (writers[1], second.clone())] {
        group.computers[sender]
            .service
            .action(Action::Settings {
                expected: revision,
                value,
            })
            .unwrap();
    }
    until(
        &mut group.computers,
        &mut group.events,
        |computers, events| {
            let conflicts = failover_cas_conflicts(&events[started..], &writers);
            assert!(conflicts <= 1, "One stale edit was reported more than once");
            let candidate = &computers[alive[0]].state.as_ref().unwrap().settings;
            conflicts == 1
                && (candidate == &first || candidate == &second)
                && alive
                    .iter()
                    .all(|index| &computers[*index].state.as_ref().unwrap().settings == candidate)
        },
    );
    group.checked_from = group.events.len();
    let winning_settings = group.state(alive[0]).settings.clone();
    assert_ne!(winning_settings, before);

    // A member edit has its own compare-and-swap guard. Competing changes from
    // two followers must choose one complete preference value, never merge or
    // replay the losing update after a formerly unavailable voter comes back.
    let edited = group.keys[4].clone();
    let before = group.state(writers[0]).members[&edited].preferences.clone();
    let mut first = before.clone();
    first.audio.volume = 17;
    let mut second = before.clone();
    second.audio.volume = 23;
    let started = group.events.len();
    for (sender, value) in [(writers[0], first.clone()), (writers[1], second.clone())] {
        group.computers[sender]
            .service
            .action(Action::Member {
                key: edited.clone(),
                expected: before.clone(),
                value,
            })
            .unwrap();
    }
    until(
        &mut group.computers,
        &mut group.events,
        |computers, events| {
            let conflicts = failover_cas_conflicts(&events[started..], &writers);
            assert!(conflicts <= 1, "One stale edit was reported more than once");
            let candidate =
                &computers[alive[0]].state.as_ref().unwrap().members[&edited].preferences;
            conflicts == 1
                && (candidate == &first || candidate == &second)
                && alive.iter().all(|index| {
                    &computers[*index].state.as_ref().unwrap().members[&edited].preferences
                        == candidate
                })
        },
    );
    group.checked_from = group.events.len();
    let winning_preferences = group.state(alive[0]).members[&edited].preferences.clone();
    for index in alive {
        let state = group.state(index);
        assert_eq!(state.workspace_id(), original);
        assert_eq!(state.input_intent, intent);
        assert_eq!(state.active_monitor, monitor);
        assert_eq!(state.confirmed_monitor, confirmed);
        assert!(state.monitor_command.is_none());
    }
    group.restart(0);
    group.restart(3);
    group.wait_ready(&[0, 1, 2, 3, 4]);
    group.observe(Duration::from_millis(500), |group| {
        group.computers.iter().all(|computer| {
            let state = computer.state.as_ref().unwrap();
            state.workspace_id() == original
                && state.settings == winning_settings
                && state.members[&edited].preferences == winning_preferences
                && state.input_intent == intent
                && state.active_monitor == monitor
                && state.confirmed_monitor == confirmed
                && state.monitor_command.is_none()
        })
    });
    group.finish();
}

#[test]
fn majority_keeps_working_without_original_coordinator_and_rejoins_without_data_loss() {
    let mut group = FailoverWorkspace::start(3);
    let original = group.keys[0].clone();
    let stale = group.state(0).clone();
    let remembered = group.state(1).input_intent.clone();
    let monitor = group.state(1).active_monitor.clone();
    let confirmed = group.state(1).confirmed_monitor.clone();
    group.stop(0);
    group.wait_ready(&[1, 2]);
    assert_ne!(group.state(1).hub, original);
    group.clipboard(1, true, &[1, 2]);

    let before = group.state(2).members[&group.keys[1]].preferences.clone();
    let mut after = before.clone();
    after.audio.volume = 17;
    group.computers[2]
        .service
        .action(Action::Member {
            key: group.keys[1].clone(),
            expected: before,
            value: after,
        })
        .unwrap();
    let edited = group.keys[1].clone();
    group.wait(|computers, _| {
        [1, 2].iter().all(|index| {
            computers[*index].state.as_ref().unwrap().members[&edited]
                .preferences
                .audio
                .volume
                == 17
        })
    });
    let leader = group.state(1).hub.clone();
    group.restart(0);
    group.wait_ready(&[0, 1, 2]);
    group.observe(Duration::from_secs(3), |group| {
        group.computers.iter().all(|computer| {
            let state = computer.state.as_ref().unwrap();
            state.hub == leader
                && state.settings.clipboard_enabled
                && state.members[&edited].preferences.audio.volume == 17
                && state.workspace_id() == original
                && state.input_intent == remembered
                && state.active_monitor == monitor
                && state.confirmed_monitor == confirmed
                && state.monitor_command.is_none()
        })
    });

    // Even a previously authoritative member cannot publish an old full State.
    // Only a consensus commit can replace the workspace in protocol 2.
    let leader_index = group.keys.iter().position(|key| key == &leader).unwrap();
    let injected = group.events.len();
    group.computers[0]
        .service
        .commands
        .try_send(Command::Raw(
            leader,
            Box::new(Packet::State(Box::new(stale))),
        ))
        .unwrap();
    until(&mut group.computers, &mut group.events, |_, events| {
        events[injected..].iter().any(|(index, event)| {
            *index == 0 && matches!(event, Event::Error(error) if !error.is_empty())
        })
    });
    assert!(
        group.events[injected..]
            .iter()
            .all(|(index, event)| { !matches!(event, Event::Error(_)) || *index == 0 })
    );
    group.checked_from = group.events.len();
    group.observe(Duration::from_millis(250), |group| {
        group.computers.iter().all(|computer| {
            let state = computer.state.as_ref().unwrap();
            state.settings.clipboard_enabled
                && state.members[&edited].preferences.audio.volume == 17
                && state.members.values().all(|member| member.online)
        })
    });

    // Restore all voters before removing the current leader, so this phase
    // exercises a genuine majority election even if initial hub0 never led.
    group.stop(leader_index);
    let survivors = (0..3)
        .filter(|index| *index != leader_index)
        .collect::<Vec<_>>();
    group.wait_ready(&survivors);
    assert_ne!(group.state(survivors[0]).hub, group.keys[leader_index]);
    group.clipboard(survivors[0], false, &survivors);
    group.restart(leader_index);
    group.wait_ready(&[0, 1, 2]);
    for index in 0..3 {
        let state = group.state(index);
        assert!(!state.settings.clipboard_enabled);
        assert_eq!(state.members[&edited].preferences.audio.volume, 17);
        assert_eq!(state.workspace_id(), original);
        assert_eq!(state.input_intent, remembered);
        assert_eq!(state.active_monitor, monitor);
        assert_eq!(state.confirmed_monitor, confirmed);
        assert!(state.monitor_command.is_none());
        state.validate().unwrap();
        assert_eq!(
            std::fs::read(group.computers[index].dir.join("identity")).unwrap(),
            group.identities[index]
        );
    }
    group.finish();
}

#[test]
fn minority_cannot_commit_settings_or_replay_rejected_edits_after_quorum_returns() {
    let mut group = FailoverWorkspace::start(3);
    let survivor = group
        .keys
        .iter()
        .position(|key| key == &group.state(0).hub)
        .unwrap();
    let others = (0..3)
        .filter(|index| *index != survivor)
        .collect::<Vec<_>>();
    let settings = group.state(survivor).settings.clone();
    let intent = group.state(survivor).input_intent.clone();
    let monitor = group.state(survivor).active_monitor.clone();
    for index in &others {
        group.stop(*index);
    }
    group.wait(|_, events| {
        failover_coordination(events, survivor)
            .is_some_and(|(_, writable, online, quorum)| !writable && online == 1 && quorum == 2)
    });
    let mut rejected = settings.clone();
    rejected.clipboard_enabled = !rejected.clipboard_enabled;
    let attempted = group.events.len();
    group.computers[survivor]
        .service
        .action(Action::Settings {
            expected: group.state(survivor).revision,
            value: rejected,
        })
        .unwrap();
    until(&mut group.computers, &mut group.events, |_, events| {
        events[attempted..].iter().any(|(index, event)| {
            *index == survivor && matches!(event, Event::Error(error) if !error.is_empty())
        })
    });
    group.checked_from = group.events.len();
    group.observe(Duration::from_millis(500), |group| {
        let state = group.state(survivor);
        state.settings == settings
            && state.input_intent == intent
            && state.active_monitor == monitor
            && state.monitor_command.is_none()
    });
    group.restart(others[0]);
    group.wait_ready(&[survivor, others[0]]);
    assert_eq!(group.state(survivor).settings, settings);
    assert_eq!(group.state(others[0]).settings, settings);
    assert_eq!(group.state(survivor).input_intent, intent);
    assert!(group.state(survivor).monitor_command.is_none());
    group.finish();
}

#[test]
fn two_member_workspace_keeps_cached_preferences_but_requires_both_votes_for_changes() {
    let mut group = FailoverWorkspace::start(2);
    let settings = group.state(1).settings.clone();
    let intent = group.state(1).input_intent.clone();
    let monitor = group.state(1).active_monitor.clone();
    let own_preferences = group.state(1).members[&group.keys[1]].preferences.clone();
    group.stop(0);
    group.wait(|_, events| {
        failover_coordination(events, 1)
            .is_some_and(|(_, writable, online, quorum)| !writable && online == 1 && quorum == 2)
    });
    let mut rejected = settings.clone();
    rejected.clipboard_enabled = !rejected.clipboard_enabled;
    let attempted = group.events.len();
    group.computers[1]
        .service
        .action(Action::Settings {
            expected: group.state(1).revision,
            value: rejected,
        })
        .unwrap();
    until(&mut group.computers, &mut group.events, |_, events| {
        events[attempted..].iter().any(|(index, event)| {
            *index == 1 && matches!(event, Event::Error(error) if !error.is_empty())
        })
    });
    group.checked_from = group.events.len();
    group.observe(Duration::from_millis(500), |group| {
        let state = group.state(1);
        state.settings == settings
            && state.members[&group.keys[1]].preferences == own_preferences
            && state.input_intent == intent
            && state.active_monitor == monitor
            && state.monitor_command.is_none()
    });
    group.restart(0);
    group.wait_ready(&[0, 1]);
    assert_eq!(group.state(0).settings, settings);
    assert_eq!(group.state(1).settings, settings);
    assert_eq!(group.state(1).input_intent, intent);
    assert!(group.state(1).monitor_command.is_none());
    group.finish();
}
