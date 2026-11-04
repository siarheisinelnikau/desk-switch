use super::{
    link::{FRAME_BYTES, Link, Packet, Pcm},
    playout::{Mix, Playout},
    stream::{self, SILENCE},
};
use crate::{
    config::{Identity, NOISE},
    controller::{Controller, Effect, Transfer},
    transport::Pipe,
    video::{Inputs, Job},
    wire::{Message, Mode},
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncWriteExt, duplex},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    task::JoinHandle,
    time::{MissedTickBehavior, interval, sleep, timeout},
};

fn identity() -> Identity {
    let pair = snow::Builder::new(NOISE.parse().unwrap())
        .generate_keypair()
        .unwrap();
    Identity {
        private: pair.private.try_into().unwrap(),
        public: pair.public.try_into().unwrap(),
    }
}

async fn sockets() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (client, server) = tokio::join!(
        TcpStream::connect(listener.local_addr().unwrap()),
        listener.accept()
    );
    (client.unwrap(), server.unwrap().0)
}

async fn links(windows: &Identity, ubuntu: &Identity) -> (Link, Link) {
    let (client, server) = sockets().await;
    let (client, server) = tokio::join!(
        Link::open(client, windows, &ubuntu.public, true),
        Link::open(server, ubuntu, &windows.public, false)
    );
    (client.unwrap(), server.unwrap())
}

fn tone(amplitude: f32) -> Pcm {
    let mut pcm = SILENCE;
    for (index, stereo) in pcm.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let phase = std::f32::consts::TAU * index as f32 / 48.0;
        let sample = (amplitude * phase.sin()) as i16;
        stereo[..2].copy_from_slice(&sample.to_le_bytes());
        stereo[2..].copy_from_slice(&sample.to_le_bytes());
    }
    pcm
}

struct DropNotice(Arc<AtomicBool>);

impl Drop for DropNotice {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

struct Fixture {
    capture: JoinHandle<anyhow::Result<()>>,
    player: JoinHandle<anyhow::Result<()>>,
    frames: mpsc::Receiver<u64>,
    capture_closed: Arc<AtomicBool>,
    queue: Arc<Mutex<Playout>>,
}

impl Fixture {
    async fn start(
        windows: &Identity,
        ubuntu: &Identity,
        queue: Arc<Mutex<Playout>>,
        pcm: Pcm,
    ) -> Self {
        let (mut client, mut server) = links(windows, ubuntu).await;
        let capture_closed = Arc::new(AtomicBool::new(false));
        let closed = Arc::clone(&capture_closed);
        let capture = tokio::spawn(async move {
            stream::request_start(&mut server).await?;
            let _capture = DropNotice(closed);
            let (mut samples, mut source) = duplex(2 * FRAME_BYTES);
            let synthetic = async {
                let mut tick = interval(Duration::from_millis(10));
                tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
                loop {
                    tick.tick().await;
                    samples.write_all(&pcm).await?;
                }
            };
            tokio::select! {
                result = stream::transmit(&mut server, &mut source) => result,
                result = synthetic => result,
            }
        });
        let (send, frames) = mpsc::channel(16);
        let playout = Arc::clone(&queue);
        let player = tokio::spawn(async move {
            stream::receive(&mut client, &playout, |sequence| {
                let _ = send.try_send(sequence);
            })
            .await
        });
        Self {
            capture,
            player,
            frames,
            capture_closed,
            queue,
        }
    }

    async fn frame(&mut self) -> u64 {
        timeout(Duration::from_secs(2), self.frames.recv())
            .await
            .unwrap()
            .unwrap()
    }

    fn render(&self) -> Pcm {
        let mut pcm = SILENCE;
        self.queue.lock().unwrap().render(
            &mut pcm,
            Mix {
                volume: 100,
                muted: false,
            },
            Instant::now(),
        );
        pcm
    }

    async fn off(self) {
        self.player.abort();
        assert!(self.player.await.unwrap_err().is_cancelled());
        assert!(
            timeout(Duration::from_secs(2), self.capture)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        assert!(self.capture_closed.load(Ordering::Acquire));
    }
}

#[tokio::test]
async fn user_off_releases_capture_and_reconnects_without_old_audio() {
    let windows = identity();
    let ubuntu = identity();
    let queue = Arc::new(Mutex::new(Playout::new()));
    let pcm = tone(2_000.0);
    let mut first = Fixture::start(&windows, &ubuntu, Arc::clone(&queue), pcm).await;
    first.frame().await;
    first.frame().await;
    assert_eq!(first.render(), pcm);
    first.off().await;

    let mut after_off = [1; FRAME_BYTES];
    queue.lock().unwrap().render(
        &mut after_off,
        Mix {
            volume: 100,
            muted: false,
        },
        Instant::now(),
    );
    assert_eq!(after_off, SILENCE);

    let fresh = tone(800.0);
    let mut second = Fixture::start(&windows, &ubuntu, queue, fresh).await;
    second.frame().await;
    second.frame().await;
    assert_eq!(second.render(), fresh);
    second.off().await;
}

#[tokio::test]
async fn broken_capture_link_clears_playback() {
    let windows = identity();
    let ubuntu = identity();
    let mut fixture = Fixture::start(
        &windows,
        &ubuntu,
        Arc::new(Mutex::new(Playout::new())),
        tone(1600.0),
    )
    .await;
    fixture.frame().await;
    fixture.frame().await;
    fixture.capture.abort();
    assert!(fixture.capture.await.unwrap_err().is_cancelled());
    assert!(
        timeout(Duration::from_secs(2), fixture.player)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(fixture.capture_closed.load(Ordering::Acquire));
    let mut after_loss = [1; FRAME_BYTES];
    fixture.queue.lock().unwrap().render(
        &mut after_loss,
        Mix {
            volume: 100,
            muted: false,
        },
        Instant::now(),
    );
    assert_eq!(after_loss, SILENCE);
}

#[tokio::test]
async fn paired_connection_still_requires_an_explicit_audio_start() {
    let (mut client, mut server) = links(&identity(), &identity()).await;
    client.send(Packet::Credit(0), &SILENCE).await.unwrap();
    let error = stream::request_start(&mut server).await.unwrap_err();
    assert!(error.to_string().contains("explicit request"));
}

#[tokio::test]
async fn late_audio_is_never_rendered() {
    let (mut client, mut server) = links(&identity(), &identity()).await;
    let queue = Mutex::new(Playout::new());
    let receive = async {
        assert!(
            stream::receive(&mut client, &queue, |_| panic!(
                "late audio escaped the deadline"
            ))
            .await
            .is_err()
        );
    };
    let delayed = async {
        stream::request_start(&mut server).await.unwrap();
        server.send(Packet::Ready, &SILENCE).await.unwrap();
        let mut pcm = SILENCE;
        assert_eq!(server.receive(&mut pcm).await.unwrap(), Packet::Credit(0));
        sleep(Duration::from_millis(250)).await;
        let _ = server
            .send(
                Packet::Audio {
                    ticket: 0,
                    sequence: 0,
                },
                &tone(1200.0),
            )
            .await;
    };
    timeout(Duration::from_secs(2), async {
        tokio::join!(receive, delayed);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn audio_and_control_cannot_authenticate_as_each_other() {
    let windows = identity();
    let ubuntu = identity();
    let (client, server) = sockets().await;
    let (control, audio) = tokio::join!(
        Pipe::connect(client, &windows, &ubuntu.public, true),
        Link::open(server, &ubuntu, &windows.public, false)
    );
    assert!(control.is_err() && audio.is_err());
}

#[tokio::test]
async fn audio_stays_continuous_through_a_video_and_input_round_trip() {
    let windows = identity();
    let ubuntu = identity();
    let pcm = tone(1_200.0);
    let mut fixture =
        Fixture::start(&windows, &ubuntu, Arc::new(Mutex::new(Playout::new())), pcm).await;
    let mut previous = fixture.frame().await;
    let (client, server) = sockets().await;
    let (client, server) = tokio::join!(
        Pipe::connect(client, &windows, &ubuntu.public, true),
        Pipe::connect(server, &ubuntu, &windows.public, false)
    );
    let (mut client, mut server) = (client.unwrap(), server.unwrap());
    let control = async {
        let inputs = Inputs {
            windows: 17,
            ubuntu: 15,
        };
        let mut controller = Controller::new(Some(inputs));
        let Effect::Send(prepare) = controller
            .start(Transfer {
                id: 1,
                mode: Mode::Video,
                edge: None,
            })
            .unwrap()
        else {
            panic!()
        };
        client.send(prepare).await.unwrap();
        assert_eq!(server.messages.recv().await.unwrap().unwrap(), prepare);
        server.send(Message::Ready(1)).await.unwrap();
        assert_eq!(
            controller
                .message(client.messages.recv().await.unwrap().unwrap())
                .unwrap(),
            Effect::Ddc(Job::Select { from: 17, to: 15 })
        );
        sleep(Duration::from_millis(25)).await;
        assert_eq!(
            controller.written().unwrap(),
            Effect::Send(Message::Verify(1))
        );
        assert_eq!(
            controller
                .message(Message::Observed { id: 1, input: 15 })
                .unwrap(),
            Effect::Send(Message::Activate(1))
        );
        assert_eq!(
            controller.message(Message::Active(1)).unwrap(),
            Effect::Forward
        );
        assert_eq!(
            controller.return_home(None, true).unwrap(),
            Effect::Send(Message::Return(1))
        );
        sleep(Duration::from_millis(25)).await;
        assert_eq!(
            controller.message(Message::Returning(1)).unwrap(),
            Effect::Ddc(Job::Read)
        );
        assert_eq!(
            controller.checked(Some(17)).unwrap(),
            Effect::Send(Message::Returned(1))
        );
        assert_eq!(
            controller.finished(),
            Effect::Local {
                position: None,
                upgrade: false
            }
        );
    };
    let audio = async {
        let mut continuous = false;
        for _ in 0..12 {
            let sequence = fixture.frame().await;
            assert_eq!(sequence, previous + 1);
            previous = sequence;
            let output = fixture.render();
            if sequence >= 2 {
                assert_eq!(output, pcm);
                continuous = true;
            }
        }
        assert!(continuous);
    };
    tokio::join!(control, audio);
    fixture.off().await;
}
