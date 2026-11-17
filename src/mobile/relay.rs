//! A workspace PC relays phone input with the existing XX/KK and text records.
//! There is one ordered client per target; ambiguous operations are never retried.
use super::{Destination, control, screen};
use crate::{
    config::{self, Identity},
    transport::{LEASE, PING_PERIOD, Pipe},
    wire::{Input, Message},
};
use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::{
    net::TcpStream,
    time::{Instant, MissedTickBehavior, interval, timeout},
};

pub(super) struct Client {
    reader: control::Reader,
    writer: control::Writer,
    input: Pipe,
    source: String,
    target: String,
    preview_supported: bool,
    epoch: u64,
    preview: screen::Options,
    frames: tokio::sync::watch::Sender<Option<(u64, screen::Frame)>>,
    next_ping: u64,
    last_reply: Instant,
}

impl Client {
    #[cfg(test)]
    pub(super) async fn connect(
        identity: &Identity,
        name: &str,
        destination: &Destination,
    ) -> Result<Self> {
        Self::connect_with_preview(
            identity,
            name,
            destination,
            0,
            tokio::sync::watch::channel(None).0,
        )
        .await
    }

    pub(super) async fn connect_with_preview(
        identity: &Identity,
        name: &str,
        destination: &Destination,
        epoch: u64,
        frames: tokio::sync::watch::Sender<Option<(u64, screen::Frame)>>,
    ) -> Result<Self> {
        let Destination::Remote {
            key,
            address,
            control_port,
            input_port,
            ..
        } = destination
        else {
            bail!("A relay requires an available remote computer");
        };
        ensure!(
            *control_port != 0 && *input_port != 0,
            "Invalid phone relay endpoint"
        );
        let peer_key = config::key(key)?;
        let mut control_address = *address;
        control_address.set_port(*control_port);
        let stream = timeout(Duration::from_secs(2), TcpStream::connect(control_address))
            .await
            .context("Phone relay connection timed out")??;
        let mut control = timeout(
            Duration::from_secs(5),
            crate::workspace::net::authenticate(
                stream,
                identity,
                name,
                true,
                Some(key),
                super::CONTROL_PORT,
            ),
        )
        .await
        .context("Phone relay authentication timed out")??;
        crate::compatibility::Version::current().require(Some(&control.version))?;
        ensure!(
            control.workspace_protocol == Some(crate::group::model::PROTOCOL),
            "The target uses a different workspace protocol"
        );
        timeout(Duration::from_secs(5), async {
            control.send_json(&json!({"Decision":true})).await?;
            ensure!(
                control.receive_json().await? == json!({"Decision":true}),
                "The target did not authorize this workspace PC"
            );
            control.send_json(&json!("Pinned")).await?;
            ensure!(
                control.receive_json().await? == json!("Pinned"),
                "The target did not finish authorizing phone input"
            );
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("The target did not authorize phone input")??;

        let mut input_address = *address;
        input_address.set_port(*input_port);
        let stream = timeout(Duration::from_secs(2), TcpStream::connect(input_address))
            .await
            .context("Phone input connection timed out")??;
        let mut input = Pipe::connect(stream, identity, &peer_key, true).await?;
        input
            .send(Message::Prepare {
                id: 1,
                inputs: None,
                edge: None,
            })
            .await?;
        let prepared = timeout(Duration::from_secs(30), input.messages.recv())
            .await
            .context("The target did not prepare phone input")?
            .context("The phone input connection ended")??;
        ensure!(
            prepared == Message::Ready(1),
            "The target could not prepare phone input"
        );
        input.send(Message::Activate(1)).await?;
        let activated = timeout(Duration::from_secs(2), input.messages.recv())
            .await
            .context("The target did not activate phone input")?
            .context("The phone input connection ended")??;
        ensure!(
            activated == Message::Active(1),
            "The target could not activate phone input"
        );
        let preview_supported = control.phone_preview;
        let (reader, writer) = control::Control::from_candidate(control);
        Ok(Self {
            reader,
            writer,
            input,
            source: config::hex(&identity.public),
            target: key.clone(),
            preview_supported,
            epoch,
            preview: screen::Options::default(),
            frames,
            next_ping: 0,
            last_reply: Instant::now(),
        })
    }

    pub(super) async fn input(&mut self, event: Input) -> Result<()> {
        ensure!(event.valid(), "Invalid relayed phone input");
        ensure!(
            self.last_reply.elapsed() < LEASE,
            "The phone relay heartbeat expired"
        );
        self.input.send(Message::Event(event)).await
    }

    /// Consumes one native acknowledgement. Safe to cancel when another command
    /// arrives: Pipe's bounded receiver retains each complete authenticated frame.
    pub(super) async fn poll(&mut self) -> Result<()> {
        tokio::select! {
            message = self.input.messages.recv() => {
                self.accept_reply(message.context("Phone input connection ended")??)?;
            }
            value = self.reader.recv() => {
                let value = value?;
                if let Some(frame) = screen::parse_frame(&value, &self.target)? {
                    if self.preview.enabled { self.frames.send_replace(Some((self.epoch, frame))); }
                } else {
                    bail!("The target sent an unexpected phone control response");
                }
            }
        }
        Ok(())
    }

    fn accept_reply(&mut self, message: Message) -> Result<u64> {
        let Message::Pong(id) = message else {
            bail!("The target sent an unexpected phone input response");
        };
        ensure!(
            id != 0 && id <= self.next_ping,
            "Invalid phone input acknowledgement"
        );
        self.last_reply = Instant::now();
        Ok(id)
    }

    pub(super) async fn heartbeat(&mut self) -> Result<()> {
        ensure!(
            self.last_reply.elapsed() < LEASE,
            "The phone relay heartbeat expired"
        );
        self.next_ping = self
            .next_ping
            .checked_add(1)
            .context("Phone relay heartbeat exhausted")?;
        self.input.send(Message::Ping(self.next_ping)).await
    }

    /// A barrier drains earlier native input before text; the existing text ack
    /// orders every later input. Cancelling this future requires dropping Client.
    pub(super) async fn text(&mut self, value: String) -> Result<()> {
        let bytes = value.as_bytes();
        ensure!(
            !bytes.is_empty() && bytes.len() <= 8192,
            "Remote text must be at most 8 KiB"
        );
        self.heartbeat().await?;
        let barrier = self.next_ping;
        let mut tick = interval(PING_PERIOD);
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        tick.tick().await;
        timeout(Duration::from_secs(2), async {
            loop {
                tokio::select! {
                    message = self.input.messages.recv() => {
                        let id = self.accept_reply(message.context("Phone input connection ended")??)?;
                        if id == barrier { return Ok::<_, anyhow::Error>(()); }
                    },
                    _ = tick.tick() => self.heartbeat().await?,
                }
            }
        }).await.context("The target did not acknowledge earlier input; text was not sent")??;
        let packet = json!({"Clipboard": {
            "source": self.source,
            "mime": "text/plain;charset=utf-8",
            "bytes": base64::engine::general_purpose::STANDARD.encode(bytes),
            "digest": config::hex(&Sha256::digest(bytes)),
        }});
        // A blocked control write must not retain input past its native lease.
        // A partial or ambiguous write fails this session instead of replaying it.
        timeout(Duration::from_secs(1), self.writer.send(&packet))
            .await
            .context("Text could not be sent; it was not retried")??;

        self.wait_for_ack().await
    }

    pub(super) async fn set_preview(&mut self, options: screen::Options) -> Result<()> {
        if options == self.preview {
            return Ok(());
        }
        self.preview = options;
        self.frames.send_replace(None);
        if !self.preview_supported {
            if options.enabled {
                self.frames.send_replace(Some((
                    self.epoch,
                    screen::Frame::Unavailable(
                        "The selected computer does not support cursor preview".into(),
                    ),
                )));
            }
            return Ok(());
        }
        let packet = options.packet(&self.source)?;
        timeout(Duration::from_secs(1), self.writer.send(&packet)).await??;
        self.wait_for_ack().await
    }

    async fn wait_for_ack(&mut self) -> Result<()> {
        let mut tick = interval(PING_PERIOD);
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        tick.tick().await;
        let reader = &mut self.reader;
        let frames = &self.frames;
        let epoch = self.epoch;
        let target = &self.target;
        let preview_enabled = self.preview.enabled;
        let control = &mut self.input;
        let next_ping = &mut self.next_ping;
        let last_reply = &mut self.last_reply;
        timeout(Duration::from_millis(4500), async {
            loop {
                tokio::select! {
                    result = reader.recv() => {
                        let result = result?;
                        if result == crate::group::net::remote_text_ack() {
                            return Ok::<_, anyhow::Error>(());
                        }
                        let frame = screen::parse_frame(&result, target)?.context("The target did not accept the control request; it was not retried")?;
                        if preview_enabled { frames.send_replace(Some((epoch, frame))); }
                    },
                    message = control.messages.recv() => {
                        let Message::Pong(id) = message.context("Phone input connection ended")?? else {
                            bail!("The target sent an unexpected phone input response");
                        };
                        ensure!(id != 0 && id <= *next_ping, "Invalid phone input acknowledgement");
                        *last_reply = Instant::now();
                    },
                    _ = tick.tick() => {
                        ensure!(last_reply.elapsed() < LEASE, "The phone relay heartbeat expired");
                        *next_ping = next_ping.checked_add(1).context("Phone relay heartbeat exhausted")?;
                        control.send(Message::Ping(*next_ping)).await?;
                    },
                }
            }
        }).await.context("The target did not acknowledge the control request; it was not retried")?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::net::Candidate;
    use tokio::net::TcpListener;

    fn identity() -> Identity {
        let pair = snow::Builder::new(config::NOISE.parse().unwrap())
            .generate_keypair()
            .unwrap();
        Identity {
            private: pair.private.try_into().unwrap(),
            public: pair.public.try_into().unwrap(),
        }
    }

    async fn authorize(listener: &TcpListener, identity: &Identity) -> Result<Candidate> {
        let (stream, _) = listener.accept().await?;
        let mut control = crate::workspace::net::authenticate(
            stream,
            identity,
            "TV",
            false,
            None,
            listener.local_addr()?.port(),
        )
        .await?;
        ensure!(
            control.receive_json().await? == json!({"Decision":true}),
            "Missing relay decision"
        );
        control.send_json(&json!({"Decision":true})).await?;
        ensure!(
            control.receive_json().await? == json!("Pinned"),
            "Missing relay pin"
        );
        control.send_json(&json!("Pinned")).await?;
        Ok(control)
    }

    async fn activate(
        listener: &TcpListener,
        identity: &Identity,
        peer: &config::Key,
    ) -> Result<Pipe> {
        let (stream, _) = listener.accept().await?;
        let mut pipe = Pipe::connect(stream, identity, peer, false).await?;
        ensure!(
            pipe.messages.recv().await.context("Input ended")??
                == Message::Prepare {
                    id: 1,
                    inputs: None,
                    edge: None
                },
            "Missing preparation"
        );
        pipe.send(Message::Ready(1)).await?;
        ensure!(
            pipe.messages.recv().await.context("Input ended")?? == Message::Activate(1),
            "Missing activation"
        );
        pipe.send(Message::Active(1)).await?;
        Ok(pipe)
    }

    #[tokio::test]
    async fn existing_native_and_text_frames_keep_order_and_renew_the_lease_during_slow_text()
    -> Result<()> {
        let source = identity();
        let destination_identity = identity();
        let control = TcpListener::bind("127.0.0.1:0").await?;
        let input = TcpListener::bind("127.0.0.1:0").await?;
        let destination = Destination::Remote {
            key: config::hex(&destination_identity.public),
            name: "TV".into(),
            address: control.local_addr()?,
            control_port: control.local_addr()?.port(),
            input_port: input.local_addr()?.port(),
        };
        let client = async {
            let mut client = Client::connect(&source, "Source PC", &destination).await?;
            client
                .input(Input::Key {
                    code: 30,
                    down: true,
                })
                .await?;
            client
                .input(Input::Key {
                    code: 30,
                    down: false,
                })
                .await?;
            client.text("Привет, TV 👋".into()).await?;
            client
                .input(Input::Key {
                    code: 14,
                    down: true,
                })
                .await?;
            Ok::<_, anyhow::Error>(())
        };
        let server = async {
            let mut control = authorize(&control, &destination_identity).await?;
            let mut input = activate(&input, &destination_identity, &source.public).await?;
            let mut events = Vec::new();
            loop {
                match input.messages.recv().await.context("Input ended")?? {
                    Message::Event(event) => events.push(event),
                    Message::Ping(id) => {
                        ensure!(
                            events
                                == [
                                    Input::Key {
                                        code: 30,
                                        down: true
                                    },
                                    Input::Key {
                                        code: 30,
                                        down: false
                                    }
                                ],
                            "Text barrier preceded input"
                        );
                        input.send(Message::Pong(id)).await?;
                        break;
                    }
                    _ => bail!("Unexpected native message"),
                }
            }
            let value = control.receive_json().await?;
            assert_eq!(
                crate::group::net::remote_text(value, &config::hex(&source.public))?,
                "Привет, TV 👋"
            );
            let delay = tokio::time::sleep(Duration::from_millis(1850));
            tokio::pin!(delay);
            let mut pings = 0;
            let mut last_ping = Instant::now();
            loop {
                tokio::select! {
                    _ = &mut delay => break,
                    message = input.messages.recv() => {
                        ensure!(last_ping.elapsed() < LEASE, "Native lease expired while text was pending");
                        let Message::Ping(id) = message.context("Input ended")?? else { bail!("Input overtook text"); };
                        input.send(Message::Pong(id)).await?;
                        last_ping = Instant::now();
                        pings += 1;
                    },
                }
            }
            ensure!(pings >= 5, "Text wait did not send periodic heartbeats");
            control
                .send_json(&crate::group::net::remote_text_ack())
                .await?;
            loop {
                match input.messages.recv().await.context("Input ended")?? {
                    Message::Ping(id) => input.send(Message::Pong(id)).await?,
                    Message::Event(Input::Key {
                        code: 14,
                        down: true,
                    }) => return Ok::<_, anyhow::Error>(()),
                    _ => bail!("Unexpected input after text"),
                }
            }
        };
        let (client, server) = timeout(Duration::from_secs(8), async {
            tokio::join!(client, server)
        })
        .await?;
        client?;
        server?;
        Ok(())
    }

    #[tokio::test]
    async fn relay_rejects_a_different_static_identity_before_sending_input() -> Result<()> {
        let source = identity();
        let destination_identity = identity();
        let control = TcpListener::bind("127.0.0.1:0").await?;
        let destination = Destination::Remote {
            key: config::hex(&identity().public),
            name: "TV".into(),
            address: control.local_addr()?,
            control_port: control.local_addr()?.port(),
            input_port: 45883,
        };
        let client = Client::connect(&source, "Source PC", &destination);
        let server = async {
            let (stream, _) = control.accept().await?;
            crate::workspace::net::authenticate(
                stream,
                &destination_identity,
                "TV",
                false,
                None,
                control.local_addr()?.port(),
            )
            .await
        };
        let (client, server) = tokio::join!(client, server);
        assert!(client.is_err());
        let mut server = server?;
        assert!(server.receive_json().await.is_err());
        Ok(())
    }
}
