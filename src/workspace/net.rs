use super::{Node, pairing_code};
use crate::{
    compatibility::Version,
    config::{self, Identity, Key},
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use snow::{Builder, TransportState};
use std::{net::SocketAddr, path::PathBuf, sync::mpsc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc as queue, watch},
    time::{Instant, MissedTickBehavior, interval, timeout},
};

const LIMIT: usize = 16 * 1024;
const AUTH_TIMEOUT: Duration = Duration::from_secs(4);
const CONSENT_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Clone, Debug)]
pub struct Request {
    pub key: String,
    pub name: String,
    pub code: String,
}

#[derive(Debug)]
pub enum Event {
    Request(Request),
    Pairing(Request),
    Connected {
        key: String,
        name: String,
        address: SocketAddr,
    },
    State(Box<Node>),
    Closed(String),
    Forgotten,
}
enum Command {
    Dial(Vec<SocketAddr>, String),
    Approve(String, bool),
    Disconnect,
    Forget,
}

pub struct Service {
    commands: queue::Sender<Command>,
    state: watch::Sender<Node>,
    stop: watch::Sender<bool>,
    events: mpsc::Receiver<Event>,
    pub port: u16,
}
impl Service {
    pub fn start(dir: PathBuf, node: Node, port: u16, ctx: eframe::egui::Context) -> Result<Self> {
        let identity = config::load_identity(&dir)?;
        let socket = socket2::Socket::new(
            socket2::Domain::IPV6,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )
        .and_then(|socket| {
            socket.set_only_v6(false)?;
            socket.bind(&SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], port)).into())?;
            Ok(socket)
        })
        .or_else(|_| {
            let socket = socket2::Socket::new(
                socket2::Domain::IPV4,
                socket2::Type::STREAM,
                Some(socket2::Protocol::TCP),
            )?;
            socket.bind(&SocketAddr::from(([0, 0, 0, 0], port)).into())?;
            Ok::<_, std::io::Error>(socket)
        })
        .context("Не удалось включить доступность в сети. Возможно, Desk Switch уже запущен.")?;
        socket.listen(8)?;
        socket.set_nonblocking(true)?;
        let listener: std::net::TcpListener = socket.into();
        Self::spawn(dir, node, listener, identity, ctx)
    }

    fn spawn(
        dir: PathBuf,
        node: Node,
        listener: std::net::TcpListener,
        identity: Identity,
        ctx: eframe::egui::Context,
    ) -> Result<Self> {
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let (send, commands) = queue::channel(16);
        let (state, local) = watch::channel(node);
        let (stop, stopping) = watch::channel(false);
        let (events_send, events) = mpsc::sync_channel(64);
        let notify = Notifier {
            events: events_send,
            ctx,
        };
        std::thread::Builder::new()
            .name("Desk Switch pair".into())
            .spawn(move || {
                let result = (|| {
                    crate::runtime()?.block_on(run(
                        listener,
                        dir,
                        identity,
                        local,
                        commands,
                        stopping,
                        notify.clone(),
                    ))
                })();
                if let Err(error) = result {
                    notify.send(Event::Closed(format!("{error:#}")));
                }
            })?;
        Ok(Self {
            commands: send,
            state,
            stop,
            events,
            port,
        })
    }
    pub fn dial(&self, addresses: Vec<SocketAddr>, key: String) -> Result<()> {
        ensure!(
            !addresses.is_empty() && addresses.len() <= 8,
            "адрес компьютера недоступен"
        );
        self.commands
            .try_send(Command::Dial(addresses, key))
            .context("соединение уже устанавливается")?;
        Ok(())
    }
    pub fn approve(&self, key: &str, accept: bool) {
        let _ = self.commands.try_send(Command::Approve(key.into(), accept));
    }
    pub fn disconnect(&self) {
        let _ = self.commands.try_send(Command::Disconnect);
    }
    pub fn forget(&self) -> Result<()> {
        self.commands
            .try_send(Command::Forget)
            .context("Не удалось разорвать пару")?;
        Ok(())
    }
    pub fn publish(&self, node: Node) {
        self.state.send_replace(node);
    }
    pub fn poll(&self) -> impl Iterator<Item = Event> + '_ {
        self.events.try_iter().take(64)
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}

#[derive(Clone)]
struct Notifier {
    events: mpsc::SyncSender<Event>,
    ctx: eframe::egui::Context,
}
impl Notifier {
    fn send(&self, event: Event) -> bool {
        let snapshot = matches!(event, Event::State(_));
        // Bounded: the UI receives at most four snapshots per second.
        match self.events.try_send(event) {
            Ok(()) => {
                self.ctx.request_repaint();
                true
            }
            Err(mpsc::TrySendError::Full(_)) => snapshot,
            Err(mpsc::TrySendError::Disconnected(_)) => false,
        }
    }
}

#[derive(Deserialize, Serialize)]
struct Hello {
    name: String,
    app: String,
    protocol: u16,
    port: u16,
}
impl Hello {
    fn validate(&self) -> Result<()> {
        ensure!(self.port != 0, "invalid control port");
        ensure!(
            !self.app.is_empty()
                && self.app.len() <= 48
                && self.app.bytes().all(|b| b.is_ascii_graphic()),
            "invalid application version"
        );
        ensure!(
            !self.name.is_empty()
                && self.name.len() <= 256
                && self.name.chars().all(|c| !c.is_control()),
            "invalid computer name"
        );
        Version::current().require(Some(&Version {
            app: self.app.clone(),
            protocol: self.protocol,
        }))
    }
}

async fn read_record(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let length = usize::from(stream.read_u16().await?);
    ensure!((1..=LIMIT).contains(&length), "invalid pair frame length");
    let mut data = vec![0; length];
    stream.read_exact(&mut data).await?;
    Ok(data)
}
async fn write_record(stream: &mut TcpStream, data: &[u8]) -> Result<()> {
    ensure!((1..=LIMIT).contains(&data.len()), "pair frame is too large");
    stream.write_u16(data.len() as u16).await?;
    stream.write_all(data).await?;
    Ok(())
}

struct Candidate {
    stream: TcpStream,
    noise: TransportState,
    key: Key,
    name: String,
    code: String,
    outgoing: bool,
    requested: bool,
    address: SocketAddr,
}
async fn authenticate(
    mut stream: TcpStream,
    identity: &Identity,
    name: &str,
    outgoing: bool,
    expected: Option<&str>,
    port: u16,
) -> Result<Candidate> {
    stream.set_nodelay(true)?;
    let mut address = stream.peer_addr()?;
    if let SocketAddr::V6(v6) = address
        && let Some(ip) = v6.ip().to_ipv4_mapped()
    {
        address = SocketAddr::new(ip.into(), v6.port());
    }
    let builder = Builder::new("Noise_XX_25519_ChaChaPoly_BLAKE2s".parse()?)
        .prologue(b"Desk Switch workspace 1")?
        .local_private_key(&identity.private)?;
    let mut noise = if outgoing {
        builder.build_initiator()?
    } else {
        builder.build_responder()?
    };
    let hello = serde_json::to_vec(&Hello {
        name: name.into(),
        app: crate::compatibility::VERSION.into(),
        protocol: crate::compatibility::PROTOCOL,
        port,
    })?;
    let mut remote = None;
    let mut buffer = vec![0; LIMIT];
    for send in [outgoing, !outgoing, outgoing] {
        if send {
            let len = noise.write_message(&hello, &mut buffer)?;
            write_record(&mut stream, &buffer[..len]).await?;
        } else {
            let bytes = read_record(&mut stream).await?;
            let len = noise.read_message(&bytes, &mut buffer)?;
            remote = Some(serde_json::from_slice::<Hello>(&buffer[..len])?);
        }
    }
    let remote = remote.context("missing computer information")?;
    remote.validate()?;
    address.set_port(remote.port);
    let key: Key = noise
        .get_remote_static()
        .context("missing peer identity")?
        .try_into()?;
    ensure!(
        key != identity.public && key != [0; 32],
        "invalid remote identity"
    );
    if let Some(expected) = expected {
        ensure!(
            config::hex(&key) == expected,
            "Идентификатор найденного компьютера изменился. Повторите поиск."
        );
    }
    let code = pairing_code(noise.get_handshake_hash());
    Ok(Candidate {
        stream,
        noise: noise.into_transport_mode()?,
        key,
        name: remote.name,
        code,
        outgoing,
        requested: outgoing,
        address,
    })
}

#[derive(Deserialize, Serialize)]
enum Frame {
    Decision(bool),
    Pinned,
    State(Box<Node>),
    Forget,
}
impl Candidate {
    async fn send(&mut self, frame: Frame) -> Result<()> {
        let data = serde_json::to_vec(&frame)?;
        let mut encrypted = vec![0; LIMIT];
        let len = self.noise.write_message(&data, &mut encrypted)?;
        timeout(
            AUTH_TIMEOUT,
            write_record(&mut self.stream, &encrypted[..len]),
        )
        .await??;
        Ok(())
    }
    async fn receive(&mut self) -> Result<Frame> {
        let data = read_record(&mut self.stream).await?;
        let mut plain = vec![0; LIMIT];
        let len = self.noise.read_message(&data, &mut plain)?;
        Ok(serde_json::from_slice(&plain[..len])?)
    }
}

struct Dial {
    key: String,
    intended: bool,
    task: tokio::task::JoinHandle<()>,
}

fn start_dial(
    identity: &Identity,
    name: String,
    port: u16,
    sender: queue::Sender<Result<Candidate>>,
    addresses: Vec<SocketAddr>,
    expected: String,
    intended: bool,
) -> tokio::task::JoinHandle<()> {
    let identity = Identity {
        private: identity.private,
        public: identity.public,
    };
    tokio::spawn(async move {
        let result = async {
            let mut last = None;
            for address in addresses {
                let result = timeout(AUTH_TIMEOUT, async {
                    let stream = TcpStream::connect(address).await?;
                    authenticate(stream, &identity, &name, true, Some(&expected), port).await
                })
                .await;
                match result {
                    Ok(Ok(mut candidate)) => {
                        candidate.requested = intended;
                        return Ok(candidate);
                    }
                    Ok(Err(e)) => last = Some(e),
                    Err(e) => last = Some(e.into()),
                }
            }
            Err(last.context("Не удалось найти второй ПК")?)
        }
        .await;
        let _ = sender.send(result).await;
    })
}

async fn run(
    listener: std::net::TcpListener,
    dir: PathBuf,
    identity: Identity,
    local: watch::Receiver<Node>,
    mut commands: queue::Receiver<Command>,
    mut stop: watch::Receiver<bool>,
    notify: Notifier,
) -> Result<()> {
    let port = listener.local_addr()?.port();
    let listener = TcpListener::from_std(listener)?;
    let (send_candidates, mut candidates) = queue::channel::<Result<Candidate>>(4);
    let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
    let own = config::hex(&identity.public);
    let mut dial: Option<Dial> = None;
    let mut reverse_wait: Option<Instant> = None;
    loop {
        tokio::select! {
            _=stop.changed()=>break,
            _=tokio::time::sleep_until(reverse_wait.unwrap_or_else(||Instant::now()+Duration::from_secs(3600))),if reverse_wait.is_some()=> {
                reverse_wait=None;
                if let Some(dial)=dial.take(){dial.task.abort();}
                notify.send(Event::Closed("Не удалось найти второй ПК".into()));
            }
            incoming=listener.accept()=> {
                let (stream,_)=incoming?;
                let Ok(permit)=permits.clone().try_acquire_owned() else {continue};
                let sender=send_candidates.clone();let identity=Identity {private:identity.private,public:identity.public};let name=local.borrow().name.clone();
                tokio::spawn(async move {
                    let _permit=permit;
                    if let Ok(Ok(candidate))=timeout(AUTH_TIMEOUT,authenticate(stream,&identity,&name,false,None,port)).await {let _=sender.try_send(Ok(candidate));}
                });
            }
            command=commands.recv()=>match command {
                Some(Command::Dial(addresses,key)) if dial.is_none()=> {
                    let task=start_dial(&identity,local.borrow().name.clone(),port,send_candidates.clone(),addresses,key.clone(),true);
                    dial=Some(Dial {key,intended:true,task});
                }
                None=>break,_=>{},
            },
            candidate=candidates.recv()=> {
                let Some(candidate)=candidate else{break};
                let mut candidate=match candidate {Ok(c)=>c,Err(e)=>{notify.send(Event::Closed(format!("{e:#}")));dial=None;reverse_wait=None;continue}};
                let key=config::hex(&candidate.key);
                if dial.as_ref().is_some_and(|d|d.intended&&d.key!=key){continue;}
                if dir.join("peer").exists() && config::load_peer(&dir)? != candidate.key {continue;}
                // Exactly one TCP direction is used for a pair, regardless of which
                // user clicked Link. A higher-key initiator requests a callback;
                // that callback retains the requester's intent, not the recipient's.
                if candidate.outgoing && own>key {
                    reverse_wait=Some(Instant::now()+Duration::from_secs(8));
                    continue;
                }
                if !candidate.outgoing && own<key {
                    if !dial.as_ref().is_some_and(|d|d.key==key) {
                        let intended=dial.as_ref().is_some_and(|d|d.key==key&&d.intended);
                        if let Some(old)=dial.take(){old.task.abort();}
                        let task=start_dial(&identity,local.borrow().name.clone(),port,send_candidates.clone(),vec![candidate.address],key.clone(),intended);
                        dial=Some(Dial {key,intended,task});
                    }
                    continue;
                }
                reverse_wait=None;
                let intended=candidate.requested || dial.as_ref().is_some_and(|d|d.key==key&&d.intended);
                if let Some(dial)=dial.take(){dial.task.abort();}
                let operation=session(&mut candidate,&dir,&own,intended,local.clone(),&mut commands,&notify);
                let result=tokio::select! {_=stop.changed()=>break,result=operation=>result};
                notify.send(Event::Closed(match result{Ok(())=>String::new(),Err(e)=>format!("{e:#}")}));
                while candidates.try_recv().is_ok(){}
            }
        }
    }
    if let Some(dial) = dial {
        dial.task.abort();
    }
    Ok(())
}

async fn session(
    peer: &mut Candidate,
    dir: &std::path::Path,
    own: &str,
    intended: bool,
    local: watch::Receiver<Node>,
    commands: &mut queue::Receiver<Command>,
    notify: &Notifier,
) -> Result<()> {
    let key = config::hex(&peer.key);
    let pinned = if dir.join("peer").exists() {
        Some(config::load_peer(dir)?)
    } else {
        None
    };
    ensure!(
        pinned.is_none_or(|known| known == peer.key),
        "Этот ПК уже связан с другим компьютером. Сначала разорвите существующую пару."
    );
    let trusted = pinned == Some(peer.key);
    let request = Request {
        key: key.clone(),
        name: peer.name.clone(),
        code: peer.code.clone(),
    };
    let mut remote_approved = false;
    let approved = if trusted || intended {
        if !trusted {
            notify.send(Event::Pairing(request));
        }
        true
    } else {
        ensure!(
            notify.send(Event::Request(request)),
            "окно приложения закрыто"
        );
        ensure!(
            matches!(
                timeout(AUTH_TIMEOUT, peer.receive()).await??,
                Frame::Decision(true)
            ),
            "Другой ПК отклонил запрос"
        );
        remote_approved = true;
        timeout(CONSENT_TIMEOUT, async {
            loop {
                let command=tokio::select! {
                    command=commands.recv()=>command,
                    _=async {
                        loop {
                            peer.stream.readable().await?;
                            let mut byte=[0];
                            match peer.stream.try_read(&mut byte) {
                                Err(e) if e.kind()==std::io::ErrorKind::WouldBlock=>continue,
                                _=>return Err::<(),_>(anyhow::anyhow!("Соединение с другим ПК закрыто")),
                            }
                        }
                    }=>bail!("Связывание отменено"),
                };
                match command {
                    Some(Command::Approve(target, accept)) if target == key => {
                        return Ok::<bool, anyhow::Error>(accept);
                    }
                    Some(Command::Dial(_, target)) if target == key => return Ok(true),
                    Some(Command::Disconnect) | None => return Ok(false),
                    _ => {}
                }
            }
        })
        .await
        .context("Время подтверждения истекло")??
    };
    peer.send(Frame::Decision(approved)).await?;
    ensure!(approved, "Запрос отклонён");
    if !remote_approved {
        let decision = tokio::select! {
            result=timeout(CONSENT_TIMEOUT,peer.receive())=>result??,
            _=async {while let Some(command)=commands.recv().await {if matches!(command,Command::Disconnect){break}}}=>bail!("Связывание отменено"),
        };
        ensure!(
            matches!(decision, Frame::Decision(true)),
            "Другой ПК отклонил запрос"
        );
    }
    config::pair(dir, &key)?;
    peer.send(Frame::Pinned).await?;
    ensure!(
        matches!(timeout(AUTH_TIMEOUT, peer.receive()).await??, Frame::Pinned),
        "Связывание не завершено"
    );
    ensure!(
        notify.send(Event::Connected {
            key: key.clone(),
            name: peer.name.clone(),
            address: peer.address,
        }),
        "окно приложения закрыто"
    );
    let mut tick = interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut last_seen = Instant::now();
    // Keep a dedicated reader so a GUI publish/tick cannot cancel half a TCP frame.
    let (mut read, mut write) = peer.stream.split();
    let mut read_buffer = Vec::new();
    loop {
        tokio::select! {
            command=commands.recv()=>match command {
                Some(Command::Disconnect)|None=>return Ok(()),
                Some(Command::Forget)=> {
                    let bytes=serde_json::to_vec(&Frame::Forget)?;let mut encrypted=vec![0;LIMIT];let len=peer.noise.write_message(&bytes,&mut encrypted)?;
                    timeout(AUTH_TIMEOUT,async {write.write_u16(len as u16).await?;write.write_all(&encrypted[..len]).await}).await??;
                    std::fs::remove_file(dir.join("peer"))?;notify.send(Event::Forgotten);return Ok(());
                }
                _=>{}
            },
            _=tick.tick()=> {
                ensure!(last_seen.elapsed()<Duration::from_secs(4),"Соединение с другим ПК потеряно");
                let state=local.borrow().clone();
                let bytes=serde_json::to_vec(&Frame::State(Box::new(state)))?;
                let mut encrypted=vec![0;LIMIT]; let len=peer.noise.write_message(&bytes,&mut encrypted)?;
                timeout(AUTH_TIMEOUT,async {write.write_u16(len as u16).await?;write.write_all(&encrypted[..len]).await}).await??;
            }
            read=read.read_buf(&mut read_buffer)=> {
                ensure!(read?>0,"Соединение с другим ПК закрыто");
                ensure!(read_buffer.len()<=LIMIT*2,"pair receive buffer overflow");
                while read_buffer.len()>=2 {
                    let length=usize::from(u16::from_be_bytes([read_buffer[0],read_buffer[1]]));
                    ensure!((1..=LIMIT).contains(&length),"invalid pair frame length");
                    if read_buffer.len()<length+2 {break;}
                    let mut plain=vec![0;LIMIT]; let len=peer.noise.read_message(&read_buffer[2..length+2],&mut plain)?;
                    let frame=serde_json::from_slice::<Frame>(&plain[..len])?;
                    if matches!(frame,Frame::Forget) {std::fs::remove_file(dir.join("peer"))?;notify.send(Event::Forgotten);return Ok(());}
                    let Frame::State(state)=frame else {bail!("unexpected pair command")};
                    state.validate(own,&key)?; last_seen=Instant::now();
                    ensure!(notify.send(Event::State(state)),"окно приложения закрыто");
                    read_buffer.drain(..length+2);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity() -> Identity {
        let pair = Builder::new(config::NOISE.parse().unwrap())
            .generate_keypair()
            .unwrap();
        Identity {
            private: pair.private.try_into().unwrap(),
            public: pair.public.try_into().unwrap(),
        }
    }

    struct Fixture {
        service: Service,
        dir: PathBuf,
        key: String,
        address: SocketAddr,
    }
    impl Fixture {
        fn new(label: &str) -> Self {
            static SEQUENCE: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "desk-switch-pair-{}-{}-{label}",
                std::process::id(),
                SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            let identity = config::create_identity(&dir).unwrap();
            let key = config::hex(&identity.public);
            let mut node = crate::workspace::tests::node(&key);
            node.hardware.known = true;
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let service = Service::spawn(
                dir.clone(),
                node,
                listener,
                identity,
                eframe::egui::Context::default(),
            )
            .unwrap();
            Self {
                service,
                dir,
                key,
                address,
            }
        }
        fn wait(&self, accept: impl Fn(&Event) -> bool) -> Event {
            let deadline = std::time::Instant::now() + Duration::from_secs(6);
            let mut seen = Vec::new();
            loop {
                if let Ok(event) = self.service.events.recv_timeout(Duration::from_millis(50)) {
                    if accept(&event) {
                        return event;
                    }
                    seen.push(format!("{event:?}"));
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "event timed out; seen: {seen:?}"
                );
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.service.stop.send_replace(true);
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn a_link_click_and_remote_approval_pin_both_keys_and_reconnect_without_approval() {
        let a = Fixture::new("approve-a");
        let b = Fixture::new("approve-b");
        a.service.dial(vec![b.address], b.key.clone()).unwrap();
        let Event::Request(request) = b.wait(|e| matches!(e, Event::Request(_))) else {
            unreachable!()
        };
        assert_eq!(request.key, a.key);
        assert!(!a.dir.join("peer").exists() && !b.dir.join("peer").exists());
        b.service.approve(&a.key, true);
        a.wait(|e| matches!(e, Event::Connected { .. }));
        b.wait(|e| matches!(e, Event::Connected { .. }));
        assert_eq!(config::hex(&config::load_peer(&a.dir).unwrap()), b.key);
        assert_eq!(config::hex(&config::load_peer(&b.dir).unwrap()), a.key);
        let mut state = a.service.state.borrow().clone();
        state.shared.audio = super::super::Selection::Computer(a.key.clone());
        state.shared.changed(&a.key);
        state.shared.volume = 74;
        a.service.publish(state);
        let Event::State(remote) = b.wait(|e| matches!(e,Event::State(n) if n.shared.volume==74))
        else {
            unreachable!()
        };
        assert_eq!(
            remote.shared.audio,
            super::super::Selection::Computer(a.key.clone())
        );
        a.service.disconnect();
        a.wait(|e| matches!(e, Event::Closed(_)));
        b.wait(|e| matches!(e, Event::Closed(_)));
        a.service.dial(vec![b.address], b.key.clone()).unwrap();
        a.wait(|e| {
            assert!(!matches!(e, Event::Request(_)));
            matches!(e, Event::Connected { .. })
        });
        b.wait(|e| {
            assert!(!matches!(e, Event::Request(_)));
            matches!(e, Event::Connected { .. })
        });
        a.service.forget().unwrap();
        a.wait(|e| matches!(e, Event::Forgotten));
        b.wait(|e| matches!(e, Event::Forgotten));
        assert!(!a.dir.join("peer").exists() && !b.dir.join("peer").exists());
    }

    #[test]
    fn rejection_never_pins_a_key_and_does_not_require_manual_tokens() {
        let a = Fixture::new("reject-a");
        let b = Fixture::new("reject-b");
        a.service.dial(vec![b.address], b.key.clone()).unwrap();
        b.wait(|e| matches!(e, Event::Request(_)));
        b.service.approve(&a.key, false);
        a.wait(|e| matches!(e, Event::Closed(_)));
        b.wait(|e| matches!(e, Event::Closed(_)));
        assert!(!a.dir.join("peer").exists() && !b.dir.join("peer").exists());
    }

    #[test]
    fn closing_the_requester_cancels_the_remote_prompt_without_pinning_keys() {
        let a = Fixture::new("cancel-a");
        let b = Fixture::new("cancel-b");
        a.service.dial(vec![b.address], b.key.clone()).unwrap();
        b.wait(|e| matches!(e, Event::Request(_)));
        a.service.disconnect();
        b.wait(|e| matches!(e, Event::Closed(_)));
        assert!(!a.dir.join("peer").exists() && !b.dir.join("peer").exists());
    }

    #[test]
    fn simultaneous_link_clicks_converge_on_one_connection() {
        let a = Fixture::new("simultaneous-a");
        let b = Fixture::new("simultaneous-b");
        a.service.dial(vec![b.address], b.key.clone()).unwrap();
        b.service.dial(vec![a.address], a.key.clone()).unwrap();
        a.wait(|e| matches!(e, Event::Connected { .. }));
        b.wait(|e| matches!(e, Event::Connected { .. }));
        a.wait(|e| matches!(e, Event::State(_)));
        b.wait(|e| matches!(e, Event::State(_)));
    }
    #[tokio::test]
    async fn initial_key_exchange_authenticates_and_uses_same_confirmation_code() {
        let a = identity();
        let b = identity();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let expected = config::hex(&b.public);
        let client = async {
            authenticate(
                TcpStream::connect(addr).await.unwrap(),
                &a,
                "A",
                true,
                Some(&expected),
                super::super::PORT,
            )
            .await
            .unwrap()
        };
        let server = async {
            authenticate(
                listener.accept().await.unwrap().0,
                &b,
                "B",
                false,
                None,
                super::super::PORT,
            )
            .await
            .unwrap()
        };
        let (mut left, mut right) = tokio::join!(client, server);
        assert_eq!(left.key, b.public);
        assert_eq!(right.key, a.public);
        assert_eq!(left.code, right.code);
        left.send(Frame::Decision(false)).await.unwrap();
        assert!(matches!(
            right.receive().await.unwrap(),
            Frame::Decision(false)
        ));
    }
    #[tokio::test]
    async fn discovery_cannot_replace_the_identity_of_a_selected_peer() {
        let a = identity();
        let b = identity();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client = async {
            authenticate(
                TcpStream::connect(addr).await.unwrap(),
                &a,
                "A",
                true,
                Some(&"ff".repeat(32)),
                super::super::PORT,
            )
            .await
            .is_err()
        };
        let server = async {
            authenticate(
                listener.accept().await.unwrap().0,
                &b,
                "B",
                false,
                None,
                super::super::PORT,
            )
            .await
            .is_ok()
        };
        let (rejected, finished) = tokio::join!(client, server);
        assert!(rejected && finished);
    }
}
