// These tests use only authenticated loopback control links. No native workers,
// input devices, audio devices, or monitor commands are started.

#[test]
fn reconnect_accepts_controller_epoch_16_over_cached_epoch_14() {
    monitor_epoch_connection(false);
}

#[test]
fn new_invite_does_not_import_an_unrelated_future_monitor_epoch() {
    monitor_epoch_connection(true);
}

fn monitor_epoch_connection(new_invite: bool) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!(
            "qa-monitor-epoch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
    let fixture = model::tests::fixture(2);
    let mut members = Vec::new();
    let mut directories = Vec::new();
    for (index, template) in fixture.members.values().enumerate() {
        let dir = root.join(index.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let identity = config::create_identity(&dir).unwrap();
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
        member.input_ready = if index == 0 { 16 } else { 14 };
        members.push(member);
        directories.push(dir);
    }
    let keys = members
        .iter()
        .map(|member| member.node.key.clone())
        .collect::<Vec<_>>();
    let failure = "   Try errors: DDCRC_RETRIES(EIO)";
    members[1].node.monitor_error = Some(failure.into());

    let mut controller = fixture;
    controller.hub = keys[0].clone();
    controller.revision = 24_733;
    controller.activation = 16;
    controller.active_input = Some(keys[0].clone());
    controller.active_monitor = Some(keys[usize::from(!new_invite)].clone());
    controller.members = members
        .iter()
        .take(if new_invite { 1 } else { 2 })
        .map(|member| (member.node.key.clone(), member.clone()))
        .collect();
    controller.validate().unwrap();

    let mut cached = controller.clone();
    if new_invite {
        // A previously independent workspace has its own counter namespace.
        cached.hub = keys[1].clone();
        cached.revision = 50_000;
        cached.activation = 4_096;
        cached.active_input = Some(keys[1].clone());
        cached.active_monitor = Some(keys[1].clone());
        members[1].input_ready = 4_096;
        cached.members = [(keys[1].clone(), members[1].clone())].into();
    } else {
        // The report that exposed this defect had a controller at 16 while
        // Ubuntu retained epoch 14 and its prior failed DDC result.
        cached.revision = 24_732;
        cached.activation = 14;
        cached.members.get_mut(&keys[0]).unwrap().input_ready = 14;
    }
    cached.validate().unwrap();
    save(&directories[0], &controller).unwrap();
    save(&directories[1], &cached).unwrap();
    let mut computers = members
        .into_iter()
        .zip(directories)
        .map(|(member, dir)| Computer {
            service: Service::start(dir.clone(), member.clone(), 0, egui::Context::default())
                .unwrap(),
            state: None,
            dir,
            member,
        })
        .collect::<Vec<_>>();
    let mut events = Vec::new();
    until(&mut computers, &mut events, |computers, _| {
        computers.iter().all(|computer| computer.state.is_some())
    });
    let address = computers[1].state.as_ref().unwrap().members[&keys[1]].address;
    computers[0]
        .service
        .action(Action::Invite {
            address,
            key: keys[1].clone(),
        })
        .unwrap();
    until(&mut computers, &mut events, |computers, events| {
        assert!(
            !events
                .iter()
                .any(|(_, event)| matches!(event, Event::Error(_))),
            "An epoch from Hello or initial Node broke the connection: {events:?}"
        );
        computers.iter().all(|computer| {
            computer.state.as_ref().is_some_and(|state| {
                state.workspace_id() == keys[0]
                    && state.authority.is_some()
                    && state.activation == 16
                    && state.members.len() == 2
                    && state.members.values().all(|member| member.online)
            })
        }) && (0..computers.len()).all(|index| {
            events
                .iter()
                .rev()
                .find_map(|(source, event)| match event {
                    Event::Coordination { writable, .. } if *source == index => Some(*writable),
                    _ => None,
                })
                .unwrap_or(false)
        })
    });

    // Exercise both directions after full State synchronization. This also
    // drains the initial Node and proves the connection survives its arrival.
    let mut settings = computers[0].state.as_ref().unwrap().settings.clone();
    settings.clipboard_enabled = true;
    computers[0]
        .service
        .action(Action::Settings {
            expected: computers[0].state.as_ref().unwrap().revision,
            value: settings,
        })
        .unwrap();
    until(&mut computers, &mut events, |computers, _| {
        computers
            .iter()
            .all(|computer| computer.state.as_ref().unwrap().settings.clipboard_enabled)
    });
    let before = computers[1].state.as_ref().unwrap().members[&keys[0]]
        .preferences
        .clone();
    let mut after = before.clone();
    after.audio.volume = 61;
    computers[1]
        .service
        .action(Action::Member {
            key: keys[0].clone(),
            expected: before,
            value: after,
        })
        .unwrap();
    until(&mut computers, &mut events, |computers, _| {
        computers.iter().all(|computer| {
            computer.state.as_ref().unwrap().members[&keys[0]]
                .preferences
                .audio
                .volume
                == 61
        })
    });

    // A delayed GUI publish can still carry the old singleton epoch after the
    // new authoritative State. Its hardware observation must get through while
    // its stale readiness and monitor result cannot replace controller state.
    let mut observation = computers[1].state.as_ref().unwrap().members[&keys[1]]
        .node
        .clone();
    observation.name = "Fresh hardware observation".into();
    observation.monitor_error = Some("Result from an unrelated GUI snapshot".into());
    computers[1].service.publish(observation, None, 4_096);
    until(&mut computers, &mut events, |computers, events| {
        assert!(
            !events
                .iter()
                .any(|(_, event)| matches!(event, Event::Error(_))),
            "A queued GUI observation imported a foreign epoch: {events:?}"
        );
        computers.iter().all(|computer| {
            computer.state.as_ref().unwrap().members[&keys[1]].node.name
                == "Fresh hardware observation"
        })
    });
    assert!(
        !events
            .iter()
            .any(|(_, event)| matches!(event, Event::Error(_))),
        "A synchronized connection reported a hard error: {events:?}"
    );
    for computer in &computers {
        let state = computer.state.as_ref().unwrap();
        state.validate().unwrap();
        assert_eq!(state.members[&keys[0]].input_ready, 16);
        assert_eq!(
            state.members[&keys[1]].input_ready,
            if new_invite { 0 } else { 14 }
        );
        assert_eq!(
            state.members[&keys[1]].node.monitor_error.as_deref(),
            if new_invite { None } else { Some(failure) }
        );
        assert!(state.monitor_command.is_none());
        computer.service.stop();
    }
    drop(computers);
    std::thread::sleep(Duration::from_millis(100));
    std::fs::remove_dir_all(root).unwrap();
}
