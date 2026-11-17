use super::*;

#[test]
fn android_fixtures_match_the_desktop_bincode_codec_without_wire_changes() {
    use crate::wire::{Input, Message};
    use bincode::Options;
    for (hex, event) in [
        (
            "0a001e01",
            Input::Key {
                code: 30,
                down: true,
            },
        ),
        (
            "0a01fb100100",
            Input::Button {
                code: 272,
                down: false,
            },
        ),
        ("0a021d8c", Input::Move { dx: -15, dy: 70 }),
        ("0a0300ef", Input::Scroll { dx: 0, dy: -120 }),
    ] {
        assert_eq!(
            config::hex(
                &bincode::DefaultOptions::new()
                    .serialize(&Message::Event(event))
                    .unwrap()
            ),
            hex
        );
    }
}

#[test]
#[ignore = "Isolated Android/JVM interoperability fixture; no native devices"]
fn external_android_client_controls_a_fake_sink() -> Result<()> {
    let output = std::env::var_os("DESK_SWITCH_MOBILE_PROOF")
        .context("Explicit isolated proof output is required")?;
    let output = PathBuf::from(output);
    ensure!(
        output.is_absolute() && output.join(".isolated-mobile-proof").is_file(),
        "Use a new isolated marked proof directory"
    );
    let _identity = config::create_identity(&output)?;
    let proof = std::sync::Arc::new(std::sync::Mutex::new(Proof::default()));
    let (service, control, input) = Service::start_at(
        output.clone(),
        "Test TV computer".into(),
        None,
        0,
        0,
        Backend::Proof(proof.clone()),
    )?;
    service.allow_screen(true);
    config::write_atomic(
        &output.join("ready.json"),
        &json!({"host":"127.0.0.1","port":control,"inputPort":input}).to_string(),
    )?;
    let deadline = std::time::Instant::now() + Duration::from_secs(100);
    loop {
        ensure!(
            std::time::Instant::now() < deadline,
            "Android interoperability fixture timed out"
        );
        match service.events.recv_timeout(Duration::from_millis(100)) {
            Ok(Event::Pair(pending)) => {
                config::write_atomic(
                    &output.join("paired.json"),
                    &json!({"code": pending.code}).to_string(),
                )?;
                let _ = pending.reply.send(true);
            }
            Ok(Event::Status(_)) => {
                let proof = proof.lock().unwrap();
                if proof.releases > 0
                    && proof.texts == ["Привет, TV 👋"]
                    && proof.inputs.iter().any(|e| {
                        matches!(
                            e,
                            crate::wire::Input::Key {
                                code: 14,
                                down: true
                            }
                        )
                    })
                {
                    ensure!(
                        proof
                            .inputs
                            .contains(&crate::wire::Input::Move { dx: -15, dy: 70 }),
                        "Motion was not received"
                    );
                    ensure!(
                        proof
                            .inputs
                            .contains(&crate::wire::Input::Scroll { dx: 0, dy: -120 }),
                        "Scroll was not received"
                    );
                    ensure!(
                        proof.inputs.contains(&crate::wire::Input::Button {
                            code: 272,
                            down: true
                        }),
                        "Click was not received"
                    );
                    config::write_atomic(&output.join("complete.json"), &json!({"input_events":proof.inputs.len(),"text_commits":proof.texts.len(),"released":proof.releases,"native_devices":false}).to_string())?;
                    break;
                }
            }
            _ => {}
        }
    }
    drop(service);
    Ok(())
}

#[test]
fn remote_text_uses_the_existing_workspace_packet_and_rejects_other_actions() {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let text = "Привет, TV 👋";
    let key = "01".repeat(32);
    let value = json!({"Clipboard": {"source":key, "mime":"text/plain;charset=utf-8", "bytes":base64::engine::general_purpose::STANDARD.encode(text), "digest":config::hex(&Sha256::digest(text.as_bytes()))}});
    assert_eq!(
        crate::group::net::remote_text(value.clone(), &key).unwrap(),
        text
    );
    assert!(crate::group::net::remote_text(value, &"02".repeat(32)).is_err());
    assert!(crate::group::net::remote_text(json!({"Action":"ReturnInput"}), &key).is_err());
}

struct Fixture {
    service: Option<Service>,
    root: PathBuf,
    dir: PathBuf,
    phone: config::Identity,
    control: u16,
    input: u16,
    proof: std::sync::Arc<std::sync::Mutex<Proof>>,
}
impl Fixture {
    fn new() -> Result<Self> {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().canonicalize()?;
        let dir = root.join(format!(
            "desk-switch-mobile-test-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir)?;
        config::create_identity(&dir)?;
        let phone_dir = dir.join("phone");
        std::fs::create_dir(&phone_dir)?;
        let phone = config::create_identity(&phone_dir)?;
        let proof = std::sync::Arc::new(std::sync::Mutex::new(Proof::default()));
        let (service, control, input) = Service::start_at(
            dir.clone(),
            "TV".into(),
            None,
            0,
            0,
            Backend::Proof(proof.clone()),
        )?;
        Ok(Self {
            service: Some(service),
            root,
            dir,
            phone,
            control,
            input,
            proof,
        })
    }
    async fn connect(&self) -> Result<Candidate> {
        let stream =
            tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, self.control)).await?;
        crate::workspace::net::authenticate(
            stream,
            &self.phone,
            "Test phone",
            true,
            None,
            self.control,
        )
        .await
    }
    fn request(&self) -> Result<Pending> {
        let service = self.service.as_ref().unwrap();
        loop {
            if let Event::Pair(request) = service.events.recv_timeout(Duration::from_secs(2))? {
                return Ok(request);
            }
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.service.take();
        if let Ok(path) = self.dir.canonicalize()
            && path.parent() == Some(self.root.as_path())
            && path.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .starts_with("desk-switch-mobile-test-")
            })
        {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

#[tokio::test]
async fn pairing_code_is_visible_before_phone_confirmation_and_decline_never_grants_input()
-> Result<()> {
    let fixture = Fixture::new()?;
    // An unrelated broken handshake must not stop the desktop service.
    drop(tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, fixture.control)).await?);
    let mut peer = fixture.connect().await?;
    let pending = fixture.request()?;
    assert_eq!(pending.code, peer.code);
    assert!(!fixture.dir.join("mobile-peers.json").exists());
    let _ = pending.reply.send(false);
    assert_eq!(
        timeout(Duration::from_secs(2), peer.receive_json()).await??,
        json!({"Decision":false})
    );
    assert!(!fixture.dir.join("mobile-peers.json").exists());
    assert!(fixture.proof.lock().unwrap().inputs.is_empty());
    Ok(())
}

#[tokio::test]
async fn forgotten_phone_disconnects_releases_held_input_and_requires_new_consent() -> Result<()> {
    use crate::wire::{Input, Message};
    let fixture = Fixture::new()?;
    let mut peer = fixture.connect().await?;
    let pending = fixture.request()?;
    let _ = pending.reply.send(true);
    peer.send_json(&json!({"Decision":true})).await?;
    assert_eq!(peer.receive_json().await?, json!({"Decision":true}));
    peer.send_json(&json!("Pinned")).await?;
    assert_eq!(peer.receive_json().await?, json!("Pinned"));
    let stream =
        tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, fixture.input)).await?;
    let mut pipe = crate::transport::Pipe::connect(stream, &fixture.phone, &peer.key, true).await?;
    pipe.send(Message::Prepare {
        id: 1,
        inputs: None,
        edge: None,
    })
    .await?;
    assert_eq!(
        pipe.messages.recv().await.context("input closed")??,
        Message::Ready(1)
    );
    pipe.send(Message::Activate(1)).await?;
    assert_eq!(
        pipe.messages.recv().await.context("input closed")??,
        Message::Active(1)
    );
    let held = Input::Key {
        code: 42,
        down: true,
    };
    pipe.send(Message::Event(held)).await?;
    pipe.send(Message::Ping(2)).await?;
    assert_eq!(
        pipe.messages.recv().await.context("input closed")??,
        Message::Pong(2)
    );
    assert!(fixture.proof.lock().unwrap().inputs.contains(&held));
    fixture
        .service
        .as_ref()
        .unwrap()
        .forget(&config::hex(&fixture.phone.public))?;
    assert!(matches!(
        timeout(Duration::from_secs(2), pipe.messages.recv()).await?,
        None | Some(Err(_))
    ));
    assert!(fixture.proof.lock().unwrap().inputs.contains(&Input::Key {
        code: 42,
        down: false
    }));
    assert!(saved(&fixture.dir)?.is_empty());
    drop(peer);
    let mut again = fixture.connect().await?;
    let pending = fixture.request()?;
    let _ = pending.reply.send(false);
    assert_eq!(again.receive_json().await?, json!({"Decision":false}));
    Ok(())
}

struct Phone {
    control: Candidate,
    input: crate::transport::Pipe,
    key: String,
    ping: u64,
}
impl Phone {
    async fn control(
        fixture: &Fixture,
        identity: &config::Identity,
        name: &str,
    ) -> Result<Candidate> {
        let stream =
            tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, fixture.control))
                .await?;
        let mut control = crate::workspace::net::authenticate(
            stream,
            identity,
            name,
            true,
            None,
            fixture.control,
        )
        .await?;
        let request = fixture.request()?;
        assert_eq!(request.key, config::hex(&identity.public));
        assert_eq!(request.code, control.code);
        let _ = request.reply.send(true);
        control.send_json(&json!({"Decision":true})).await?;
        assert_eq!(control.receive_json().await?, json!({"Decision":true}));
        control.send_json(&json!("Pinned")).await?;
        assert_eq!(control.receive_json().await?, json!("Pinned"));
        Ok(control)
    }
    async fn native(
        fixture: &Fixture,
        identity: &config::Identity,
        control: Candidate,
    ) -> Result<Self> {
        use crate::wire::Message;
        let stream =
            tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, fixture.input)).await?;
        let mut input =
            crate::transport::Pipe::connect(stream, identity, &control.key, true).await?;
        input
            .send(Message::Prepare {
                id: 1,
                inputs: None,
                edge: None,
            })
            .await?;
        assert_eq!(
            timeout(Duration::from_secs(3), input.messages.recv())
                .await?
                .context("native closed")??,
            Message::Ready(1)
        );
        input.send(Message::Activate(1)).await?;
        assert_eq!(
            input.messages.recv().await.context("native closed")??,
            Message::Active(1)
        );
        Ok(Self {
            control,
            input,
            key: config::hex(&identity.public),
            ping: 0,
        })
    }
    async fn event(&mut self, event: crate::wire::Input) -> Result<()> {
        self.input.send(crate::wire::Message::Event(event)).await
    }
    async fn ping(&mut self) -> Result<()> {
        self.ping += 1;
        self.input
            .send(crate::wire::Message::Ping(self.ping))
            .await?;
        assert_eq!(
            timeout(Duration::from_secs(1), self.input.messages.recv())
                .await?
                .context("native closed")??,
            crate::wire::Message::Pong(self.ping)
        );
        Ok(())
    }
    async fn text(&mut self, value: &str) -> Result<()> {
        assert_eq!(self.request_text(value).await?, json!("Ping"));
        Ok(())
    }
    async fn request_text(&mut self, value: &str) -> Result<serde_json::Value> {
        use base64::Engine;
        use sha2::{Digest, Sha256};
        self.ping().await?;
        let packet = json!({"Clipboard":{"source":self.key,"mime":"text/plain;charset=utf-8","bytes":base64::engine::general_purpose::STANDARD.encode(value),"digest":config::hex(&Sha256::digest(value.as_bytes()))}});
        self.control.send_json(&packet).await?;
        timeout(Duration::from_secs(3), self.control.receive_json()).await?
    }
    async fn wait(&mut self, check: impl Fn() -> bool) -> Result<()> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !check() {
            ensure!(
                tokio::time::Instant::now() < deadline,
                "Phone proof timed out"
            );
            self.ping().await?;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(())
    }
}

#[tokio::test]
async fn two_same_ip_phones_match_their_own_native_stream_and_one_forget_keeps_the_other_held()
-> Result<()> {
    use crate::wire::Input;
    let fixture = Fixture::new()?;
    let other_dir = fixture.dir.join("second-phone");
    std::fs::create_dir(&other_dir)?;
    let other = config::create_identity(&other_dir)?;
    let first_control = Phone::control(&fixture, &fixture.phone, "First phone").await?;
    let second_control = Phone::control(&fixture, &other, "Second phone").await?;
    // Native arrival order is deliberately opposite to control arrival order.
    let mut second = Phone::native(&fixture, &other, second_control).await?;
    let mut first = Phone::native(&fixture, &fixture.phone, first_control).await?;
    let shift = Input::Key {
        code: 42,
        down: true,
    };
    first.event(shift).await?;
    second.event(shift).await?;
    first.text("first").await?;
    second.text("second").await?;
    assert_eq!(
        fixture
            .proof
            .lock()
            .unwrap()
            .inputs
            .iter()
            .filter(|event| **event == shift)
            .count(),
        1
    );
    fixture.service.as_ref().unwrap().forget(&first.key)?;
    assert!(matches!(
        timeout(Duration::from_secs(2), first.input.messages.recv()).await?,
        None | Some(Err(_))
    ));
    second.text("still connected").await?;
    let up = Input::Key {
        code: 42,
        down: false,
    };
    assert!(!fixture.proof.lock().unwrap().inputs.contains(&up));
    second.event(up).await?;
    second.text("released").await?;
    assert_eq!(
        fixture
            .proof
            .lock()
            .unwrap()
            .inputs
            .iter()
            .filter(|event| **event == up)
            .count(),
        1
    );
    assert_eq!(
        fixture.proof.lock().unwrap().texts,
        ["first", "second", "still connected", "released"]
    );
    assert_eq!(saved(&fixture.dir)?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_shared_phone_follows_the_selected_pc_after_monitor_settle_without_replaying_input()
-> Result<()> {
    use crate::wire::Input;
    let source = Fixture::new()?;
    let target = Fixture::new()?;
    let source_key = config::hex(&config::load_identity(&source.dir)?.public);
    let target_key = config::hex(&config::load_identity(&target.dir)?.public);
    source.service.as_ref().unwrap().allow_screen(true);
    target.service.as_ref().unwrap().allow_screen(true);
    target.service.as_ref().unwrap().set_routing(Routing {
        members: [source_key.clone()].into_iter().collect(),
        ..Default::default()
    });
    let control = Phone::control(&source, &source.phone, "Routed phone").await?;
    let mut phone = Phone::native(&source, &source.phone, control).await?;
    let held = Input::Button {
        code: 272,
        down: true,
    };
    phone.event(held).await?;
    phone.text("source").await?;
    let members = [target_key.clone()].into_iter().collect();
    source.service.as_ref().unwrap().set_routing(Routing {
        generation: 2,
        destination: Destination::Paused {
            reason: "Monitor switch is settling".into(),
        },
        members,
    });
    phone
        .wait(|| {
            source
                .proof
                .lock()
                .unwrap()
                .inputs
                .contains(&Input::Button {
                    code: 272,
                    down: false,
                })
        })
        .await?;
    let paused = Input::Move { dx: 999, dy: 999 };
    let paused_scroll = Input::Scroll { dx: 120, dy: -120 };
    phone.event(paused).await?;
    phone.event(paused_scroll).await?;
    let rejected = phone.request_text("typed during monitor switch").await?;
    assert!(
        rejected
            .get("Error")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| value.starts_with("Text: "))
    );
    phone.ping().await?;
    source.service.as_ref().unwrap().set_routing(Routing {
        generation: 2,
        destination: Destination::Remote {
            key: target_key.clone(),
            name: "Other PC".into(),
            address: ([127, 0, 0, 1], 1).into(),
            control_port: target.control,
            input_port: target.input,
        },
        members: [target_key].into_iter().collect(),
    });
    // Probe fresh motion while keeping the source's existing native lease alive.
    let fresh = Input::Move { dx: 7, dy: -3 };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !target.proof.lock().unwrap().inputs.contains(&fresh) {
        ensure!(
            tokio::time::Instant::now() < deadline,
            "Remote route did not become ready"
        );
        phone.event(fresh).await?;
        phone.ping().await?;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let remote_scroll = Input::Scroll { dx: -35, dy: 240 };
    phone.event(remote_scroll).await?;
    phone
        .wait(|| target.proof.lock().unwrap().inputs.contains(&remote_scroll))
        .await?;
    phone.text("Привет, второй ПК 👋").await?;
    {
        let target = target.proof.lock().unwrap();
        assert!(!target.inputs.contains(&held));
        assert!(!target.inputs.contains(&paused));
        assert!(!target.inputs.contains(&paused_scroll));
    }
    assert!(!source.proof.lock().unwrap().inputs.contains(&remote_scroll));
    assert_eq!(target.proof.lock().unwrap().texts, ["Привет, второй ПК 👋"]);
    assert_eq!(source.proof.lock().unwrap().texts, ["source"]);
    phone
        .control
        .send_json(
            &screen::Options {
                enabled: true,
                fps: 12,
                ..Default::default()
            }
            .packet(&phone.key)?,
        )
        .await?;
    assert_eq!(
        timeout(Duration::from_secs(2), phone.control.receive_json()).await??,
        json!("Ping")
    );
    // A complete synthetic crop must arrive through the authenticated relay.
    let source_frame = timeout(Duration::from_secs(2), async {
        loop {
            let value = phone.control.receive_json().await?;
            if let Some(screen::Frame::Png(bytes)) = screen::parse_frame(&value, &source_key)? {
                break Ok::<_, anyhow::Error>(bytes);
            }
        }
    })
    .await??;
    assert!(source_frame.starts_with(b"\x89PNG\r\n\x1a\n"));
    phone
        .control
        .send_json(&screen::Options::default().packet(&phone.key)?)
        .await?;
    timeout(Duration::from_secs(2), async {
        while phone.control.receive_json().await? != json!("Ping") {}
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    assert!(
        saved(&target.dir)?.is_empty(),
        "A workspace relay is not a phone or a workspace voter"
    );
    source.service.as_ref().unwrap().set_routing(Routing {
        generation: 3,
        ..Default::default()
    });
    let returned = Input::Move { dx: -7, dy: 4 };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !source.proof.lock().unwrap().inputs.contains(&returned) {
        ensure!(
            tokio::time::Instant::now() < deadline,
            "Local route did not become ready again"
        );
        phone.event(returned).await?;
        phone.ping().await?;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let local_scroll = Input::Scroll { dx: 240, dy: -35 };
    phone.event(local_scroll).await?;
    phone
        .wait(|| source.proof.lock().unwrap().inputs.contains(&local_scroll))
        .await?;
    phone.text("back on source").await?;
    {
        let source = source.proof.lock().unwrap();
        assert!(!source.inputs.contains(&paused_scroll));
        assert_eq!(source.texts, ["source", "back on source"]);
    }
    assert!(!target.proof.lock().unwrap().inputs.contains(&local_scroll));
    Ok(())
}
