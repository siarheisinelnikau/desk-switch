//! Bounded workers publish changes, never drive a UI polling/render loop.
use super::{
    protocol::{Catalog, FolderInfo, Recovery, Request, Response},
    settings::{self, Folder, Pair, Settings, UsbGrant},
    storage::Root,
    sync::{self, Change, Resolution, Side},
    transport::{self, Connection},
    usb,
};
use crate::{config, group::file_transfer::TrustSnapshot};
use anyhow::{Context, Result, bail, ensure};
use eframe::egui;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    net::{Shutdown, TcpStream},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tokio::sync::{Semaphore, mpsc, watch};

const PORT: u16 = 45882;
#[cfg(test)]
mod tests;
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub folders: Vec<Folder>,
    pub pairs: Vec<Arc<Pair>>,
    pub catalogs: BTreeMap<String, Catalog>,
    pub devices: Vec<usb::Device>,
    pub usb_grants: Vec<UsbGrant>,
    pub owners: BTreeMap<String, String>,
    pub imports: Vec<String>,
    pub usb_errors: BTreeMap<String, String>,
    pub recovery: Vec<Recovery>,
    pub busy: bool,
    pub status: String,
    pub error: Option<String>,
    pub bytes: u64,
    pub total: u64,
    pub current: String,
}
#[cfg_attr(test, derive(serde::Deserialize, serde::Serialize))]
pub enum Command {
    AddFolder {
        name: String,
        peer: String,
        root: PathBuf,
    },
    RemoveFolder(String),
    AddPair {
        name: String,
        local: String,
        remote: String,
    },
    RemovePair(String),
    RefreshPeer(String),
    Compare(String),
    Resolve {
        pair: String,
        choices: BTreeMap<String, Resolution>,
    },
    Run(String),
    RefreshUsb,
    ShareUsb {
        device: usb::Device,
        peer: String,
    },
    UnshareUsb(String),
    AttachUsb {
        device: usb::Device,
        peer: String,
    },
    Versions(String),
    Restore {
        folder: String,
        version: String,
    },
}
impl Command {
    fn usb_key(&self) -> Option<String> {
        match self {
            Self::ShareUsb { device, .. } => Some(device.bus_id.clone()),
            Self::UnshareUsb(id) => Some(id.clone()),
            Self::AttachUsb { device, peer } => Some(format!("{peer}/{}", device.bus_id)),
            _ => None,
        }
    }
}
struct Network {
    peer: String,
    workspace: String,
    socket: TcpStream,
}
struct Core {
    dir: PathBuf,
    port: u16,
    identity: config::Identity,
    ctx: egui::Context,
    trust: Mutex<TrustSnapshot>,
    settings: Mutex<Settings>,
    view: Mutex<Snapshot>,
    locks: Mutex<BTreeMap<String, Arc<Mutex<()>>>>,
    owners: Arc<usb::Owners>,
    imports: Mutex<BTreeMap<String, TcpStream>>,
    detaching: Mutex<BTreeSet<String>>,
    sockets: Mutex<BTreeMap<u64, Network>>,
    sequence: AtomicU64,
    cancel: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    active_socket: Mutex<Option<TcpStream>>,
    progress_at: Mutex<std::time::Instant>,
}
pub struct Service {
    core: Arc<Core>,
    commands: mpsc::Sender<Command>,
    stop: watch::Sender<bool>,
}
struct Tracked {
    core: Arc<Core>,
    id: u64,
}
impl Drop for Tracked {
    fn drop(&mut self) {
        self.core.sockets.lock().unwrap().remove(&self.id);
    }
}
impl Service {
    pub fn start(dir: PathBuf, ctx: egui::Context) -> Result<Self> {
        Self::start_on(dir, ctx, crate::listen::wildcard_tcp(PORT, 16)?)
    }
    fn start_on(dir: PathBuf, ctx: egui::Context, listener: std::net::TcpListener) -> Result<Self> {
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let settings = Settings::load(&dir)?;
        let core = Arc::new(Core {
            identity: config::load_identity(&dir)?,
            dir,
            port,
            ctx,
            trust: Mutex::new(TrustSnapshot::default()),
            settings: Mutex::new(settings),
            view: Mutex::new(Snapshot::default()),
            locks: Mutex::new(BTreeMap::new()),
            owners: Arc::new(usb::Owners::default()),
            imports: Mutex::new(BTreeMap::new()),
            detaching: Mutex::new(BTreeSet::new()),
            sockets: Mutex::new(BTreeMap::new()),
            sequence: AtomicU64::new(1),
            cancel: Arc::new(AtomicBool::new(false)),
            stopped: Arc::new(AtomicBool::new(false)),
            active_socket: Mutex::new(None),
            progress_at: Mutex::new(std::time::Instant::now()),
        });
        core.publish_settings();
        let (commands, mut receiver) = mpsc::channel::<Command>(16);
        let (stop, mut stopping) = watch::channel(false);
        let worker = core.clone();
        std::thread::Builder::new().name("desk-resources".into()).spawn(move || {
            let result = (|| -> Result<()> {
                let runtime = tokio::runtime::Builder::new_current_thread().enable_io().enable_time().build()?;
                runtime.block_on(async {
                    let listener = tokio::net::TcpListener::from_std(listener)?;
                    let slots = Arc::new(Semaphore::new(8));
                    loop {
                        tokio::select! {
                            _ = stopping.changed() => break,
                            Some(command) = receiver.recv() => {
                                let core = worker.clone();
                                if core.view.lock().unwrap().busy { core.error("Another folder/USB operation is running".into()); continue; }
                                core.cancel.store(false, Ordering::Release);
                                core.change(|view| { view.busy = true; view.error = None; view.bytes = 0; view.total = 0; view.current.clear(); });
                                std::thread::spawn(move || {
                                    let usb_key = command.usb_key();
                                    if let Some(key) = &usb_key { core.change(|view| { view.usb_errors.remove(key); }); }
                                    let result = core.execute(command);
                                    core.active_socket.lock().unwrap().take();
                                    core.change(|view| { view.busy = false; if let Err(error) = result { let error = format!("{error:#}"); if let Some(key) = usb_key { view.usb_errors.insert(key, error.clone()); } view.error = Some(error); view.status = if core.cancel.load(Ordering::Acquire) { "Paused" } else { "Action failed" }.into(); } });
                                });
                            }
                            result = listener.accept() => {
                                let (stream, _) = result?;
                                let Ok(permit) = slots.clone().try_acquire_owned() else { continue; };
                                let stream = stream.into_std()?; stream.set_nonblocking(false)?;
                                let core = worker.clone();
                                std::thread::spawn(move || { let _permit = permit; let _ = core.incoming(stream); });
                            }
                        }
                    }
                    Ok::<_, anyhow::Error>(())
                })
            })();
            if let Err(error) = result { worker.error(format!("Folder/USB sharing is unavailable: {error:#}")); }
        })?;
        Ok(Self {
            core,
            commands,
            stop,
        })
    }
    pub fn update(&self, trust: TrustSnapshot) {
        let mut current = self.core.trust.lock().unwrap();
        let changed = current.workspace_id != trust.workspace_id
            || current.peers.keys().ne(trust.peers.keys());
        *current = trust.clone();
        drop(current);
        if changed {
            for network in self.core.sockets.lock().unwrap().values() {
                if network.workspace != trust.workspace_id
                    || !trust.peers.contains_key(&network.peer)
                {
                    self.core.cancel.store(true, Ordering::Release);
                    let _ = network.socket.shutdown(Shutdown::Both);
                }
            }
            self.core
                .owners
                .close_untrusted(|peer| trust.peers.contains_key(peer));
        }
    }
    pub fn submit(&self, command: Command) -> Result<()> {
        self.commands
            .try_send(command)
            .map_err(|_| anyhow::anyhow!("Folder/USB command queue is full"))
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut view = self.core.view.lock().unwrap().clone();
        view.owners = self.core.owners.snapshot();
        view.imports = self.core.imports.lock().unwrap().keys().cloned().collect();
        view
    }
    pub fn pause(&self) {
        self.core.cancel.store(true, Ordering::Release);
        if let Some(socket) = self.core.active_socket.lock().unwrap().as_ref() {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
    pub fn detach(&self, key: &str) {
        if let Some(socket) = self.core.imports.lock().unwrap().get(key) {
            self.core.detaching.lock().unwrap().insert(key.into());
            let _ = socket.shutdown(Shutdown::Both);
        }
        if self.core.owners.snapshot().contains_key(key) {
            self.core.detaching.lock().unwrap().insert(key.into());
        }
        self.core.owners.detach(key);
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.core.stopped.store(true, Ordering::Release);
        self.pause();
        let _ = self.stop.send(true);
        self.core.close_all();
    }
}
impl Core {
    fn change(&self, update: impl FnOnce(&mut Snapshot)) {
        update(&mut self.view.lock().unwrap());
        self.ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
    fn error(&self, error: String) {
        self.change(|view| view.error = Some(error));
    }
    fn status(&self, status: &str) {
        self.change(|view| view.status = status.into());
    }
    fn progress(&self, path: &str, bytes: u64) {
        let now = std::time::Instant::now();
        let mut last = self.progress_at.lock().unwrap();
        if now.duration_since(*last) < std::time::Duration::from_millis(100) && !path.is_empty() {
            return;
        }
        *last = now;
        drop(last);
        self.change(|view| {
            view.current = path.into();
            view.bytes = bytes;
        });
    }
    fn publish_settings(&self) {
        let settings = self.settings.lock().unwrap().clone();
        self.change(|view| {
            view.folders = settings.folders;
            view.pairs = settings.pairs.into_iter().map(Arc::new).collect();
            view.usb_grants = settings.usb;
        });
    }
    fn save(&self, settings: Settings) -> Result<()> {
        settings.save(&self.dir)?;
        *self.settings.lock().unwrap() = settings;
        self.publish_settings();
        Ok(())
    }
    fn authorize(&self, peer: &str, workspace: &str) -> Result<()> {
        let trust = self.trust.lock().unwrap();
        ensure!(
            !self.stopped.load(Ordering::Acquire)
                && !workspace.is_empty()
                && trust.workspace_id == workspace
                && trust.peers.contains_key(peer),
            "Computer is no longer paired in this workspace"
        );
        Ok(())
    }
    fn track(self: &Arc<Self>, peer: &str, workspace: &str, socket: TcpStream) -> Tracked {
        let id = self.sequence.fetch_add(1, Ordering::Relaxed);
        self.sockets.lock().unwrap().insert(
            id,
            Network {
                peer: peer.into(),
                workspace: workspace.into(),
                socket,
            },
        );
        Tracked {
            core: self.clone(),
            id,
        }
    }
    fn close_all(&self) {
        for network in self.sockets.lock().unwrap().values() {
            let _ = network.socket.shutdown(Shutdown::Both);
        }
    }
    fn folder_lock(&self, id: &str) -> Arc<Mutex<()>> {
        self.locks
            .lock()
            .unwrap()
            .entry(id.into())
            .or_default()
            .clone()
    }
    fn folder(&self, id: &str, peer: &str, workspace: &str) -> Result<Folder> {
        self.authorize(peer, workspace)?;
        self.settings
            .lock()
            .unwrap()
            .folders
            .iter()
            .find(|f| f.id == id && f.peer == peer && f.workspace == workspace)
            .cloned()
            .context("Folder is not shared with this computer")
    }
    fn pair(&self, id: &str) -> Result<(Pair, Folder)> {
        let settings = self.settings.lock().unwrap();
        let pair = settings
            .pairs
            .iter()
            .find(|v| v.id == id)
            .cloned()
            .context("Unknown folder pair")?;
        let folder = settings
            .folders
            .iter()
            .find(|v| v.id == pair.local)
            .cloned()
            .context("Local folder is unavailable")?;
        drop(settings);
        self.authorize(&folder.peer, &folder.workspace)?;
        Ok((pair, folder))
    }
    fn connect(self: &Arc<Self>, peer: &str) -> Result<(Connection, Tracked)> {
        let trust = self.trust.lock().unwrap().clone();
        self.authorize(peer, &trust.workspace_id)?;
        let mut address = trust
            .peers
            .get(peer)
            .context("Choose a paired computer")?
            .address;
        address.set_port(self.port);
        let connection = transport::connect(address, &self.identity, peer, &trust.workspace_id)?;
        *self.active_socket.lock().unwrap() = Some(connection.socket()?);
        let tracked = self.track(peer, &trust.workspace_id, connection.socket()?);
        self.authorize(peer, &trust.workspace_id)?;
        Ok((connection, tracked))
    }
    fn catalog(&self, peer: &str, workspace: &str) -> Catalog {
        let settings = self.settings.lock().unwrap();
        let usb: Vec<_> = settings
            .usb
            .iter()
            .filter(|g| g.workspace == workspace && g.grant.peer == peer)
            .map(|g| g.grant.device.clone())
            .collect();
        let owners = self
            .owners
            .snapshot()
            .into_iter()
            .filter(|(id, _)| usb.iter().any(|device| device.bus_id == *id))
            .collect();
        Catalog {
            folders: settings
                .folders
                .iter()
                .filter(|f| f.peer == peer && f.workspace == workspace)
                .map(|f| FolderInfo {
                    id: f.id.clone(),
                    name: f.name.clone(),
                })
                .collect(),
            usb,
            owners,
        }
    }
    fn incoming(self: &Arc<Self>, stream: TcpStream) -> Result<()> {
        let trust = self.trust.lock().unwrap().clone();
        let (peer, mut connection) = transport::accept(
            stream,
            &self.identity,
            &trust.workspace_id,
            &trust.peers.keys().cloned().collect(),
        )?;
        let _tracked = self.track(&peer, &trust.workspace_id, connection.socket()?);
        self.authorize(&peer, &trust.workspace_id)?;
        match connection.receive()? {
            Request::Catalog => {
                connection.send(&Response::Catalog(self.catalog(&peer, &trust.workspace_id)))
            }
            Request::Folder { id, job } => {
                let result =
                    self.serve_folder(&mut connection, &peer, &trust.workspace_id, &id, &job);
                if let Err(error) = &result {
                    let _ = connection.send(&Response::Error(
                        format!("{error:#}").chars().take(2048).collect(),
                    ));
                }
                result
            }
            Request::UsbAttach { device } => {
                self.serve_usb(connection, &peer, &trust.workspace_id, device)
            }
            _ => bail!("Expected folder/USB capability request"),
        }
    }
    fn serve_folder(
        &self,
        connection: &mut Connection,
        peer: &str,
        workspace: &str,
        id: &str,
        job: &str,
    ) -> Result<()> {
        ensure!(super::safe::valid_hash(job), "Invalid sync job identity");
        let folder = self.folder(id, peer, workspace)?;
        let lock = self.folder_lock(id);
        let _lease = lock.try_lock().map_err(|_| {
            anyhow::anyhow!("Folder is already synchronizing; try again when it finishes")
        })?;
        let root = Root::open(&folder.root)?;
        let cancel = &self.stopped;
        connection.send(&Response::Manifest(root.manifest(cancel)?))?;
        let mut current = None;
        loop {
            self.folder(id, peer, workspace)?;
            match connection.receive()? {
                Request::Manifest => {
                    connection.send(&Response::Manifest(root.manifest(cancel)?))?
                }
                Request::Begin { index, operation } => {
                    ensure!(
                        index < (super::safe::MAX_ENTRIES * 2) as u32,
                        "Invalid sync operation index"
                    );
                    let operation_id = operation_id(workspace, peer, id, job, index);
                    let (done, offset, prefix) = root.begin(&operation_id, &operation, cancel)?;
                    current = Some((operation_id, operation));
                    connection.send(&Response::Ready {
                        done,
                        offset,
                        prefix,
                    })?;
                }
                Request::Chunk { offset, bytes } => {
                    let (id, op) = current.as_ref().context("No sync upload is open")?;
                    let next = root.chunk(id, op, offset, &bytes, cancel)?;
                    connection.send(&Response::Ack(next))?;
                }
                Request::Finish => {
                    let (id, op) = current.as_ref().context("No sync operation is open")?;
                    root.finish(id, op, cancel)?;
                    connection.send(&Response::Ok)?;
                }
                Request::Verify { path, value } => {
                    root.verify_source(&path, &value, cancel)?;
                    connection.send(&Response::Ok)?;
                }
                Request::Prefix { path, offset } => {
                    connection.send(&Response::Prefix(
                        root.source_prefix(&path, offset, cancel)?,
                    ))?;
                }
                Request::Read {
                    path,
                    value,
                    offset,
                } => {
                    connection.send(&Response::Bytes(root.read(&path, &value, offset, cancel)?))?;
                }
                Request::Close => return Ok(()),
                _ => bail!("Unexpected folder operation"),
            }
        }
    }
    fn serve_usb(
        self: &Arc<Self>,
        mut connection: Connection,
        peer: &str,
        workspace: &str,
        device: usb::Device,
    ) -> Result<()> {
        let prepare = (|| -> Result<_> {
            usb::bus_id(&device.bus_id)?;
            ensure!(
                device.supported && self.catalog(peer, workspace).usb.contains(&device),
                "USB device is not shared with this computer"
            );
            let lease = self
                .owners
                .reserve(&device.bus_id, peer, connection.socket()?)?;
            let actual = usb::list(usb::backend_address())?;
            ensure!(
                actual.contains(&device),
                "USB device changed or was unplugged; share it again"
            );
            Ok((lease, usb::export_socket(&device.bus_id)?))
        })();
        let (lease, mut backend) = match prepare {
            Ok(value) => value,
            Err(error) => {
                self.change(|view| {
                    view.usb_errors
                        .insert(device.bus_id.clone(), format!("{error:#}"));
                });
                connection.send(&Response::Error(format!("{error:#}")))?;
                return Err(error);
            }
        };
        self.change(|view| {
            view.usb_errors.remove(&device.bus_id);
        });
        let setup = (|| -> Result<()> {
            connection.send(&Response::Ok)?;
            self.status("USB connecting");
            // Import headers are authenticated; the backend's returned device
            // identity is verified before either endpoint upgrades to streaming.
            let Response::Bytes(request) = connection.receive()? else {
                bail!("Expected USB import header");
            };
            let request: [u8; 40] = request
                .try_into()
                .map_err(|_| anyhow::anyhow!("Invalid USB import header size"))?;
            self.authorize(peer, workspace)?;
            let reply = usb::start_export(&mut backend, &device, &request)?;
            connection.send(&Response::Bytes(reply))?;
            Ok(())
        })();
        let result = match setup {
            Ok(()) => {
                self.status("USB attached");
                usb::relay(connection, backend, self.stopped.clone())
            }
            Err(error) => {
                let _ = connection.send(&Response::Error(format!("{error:#}")));
                Err(error)
            }
        };
        drop(lease);
        let detached = self.detaching.lock().unwrap().remove(&device.bus_id);
        self.change(|view| {
            view.status = "USB disconnected".into();
            if !detached && let Err(error) = &result {
                let error = format!("USB tunnel ended: {error:#}");
                view.usb_errors.insert(device.bus_id, error.clone());
                view.error = Some(error);
            }
        });
        result
    }
}
fn response(connection: &mut Connection) -> Result<Response> {
    match connection.receive()? {
        Response::Error(error) => bail!(error),
        value => Ok(value),
    }
}
fn ok(connection: &mut Connection, request: &Request) -> Result<()> {
    connection.send(request)?;
    ensure!(
        matches!(response(connection)?, Response::Ok),
        "Unexpected folder/USB response"
    );
    Ok(())
}
fn operation_id(workspace: &str, peer: &str, folder: &str, job: &str, index: u32) -> String {
    super::safe::hash(
        format!("Desk Switch sync operation\0{workspace}\0{peer}\0{folder}\0{job}\0{index}")
            .as_bytes(),
    )
}

impl Core {
    fn execute(self: &Arc<Self>, command: Command) -> Result<()> {
        let mut settings = self.settings.lock().unwrap().clone();
        let trust = self.trust.lock().unwrap().clone();
        match command {
            Command::AddFolder { name, peer, root } => {
                settings::name(&name)?;
                self.authorize(&peer, &trust.workspace_id)?;
                Root::open(&root)?;
                let normalized = |path: &std::path::Path| {
                    format!(
                        "{}/",
                        path.to_string_lossy()
                            .replace('\\', "/")
                            .trim_end_matches('/')
                            .to_lowercase()
                    )
                };
                let candidate = normalized(&root);
                ensure!(
                    !settings.folders.iter().any(|f| {
                        let old = normalized(&f.root);
                        candidate.starts_with(&old) || old.starts_with(&candidate)
                    }),
                    "Shared folders must not overlap"
                );
                settings.folders.push(Folder {
                    id: settings::new_id()?,
                    name,
                    workspace: trust.workspace_id,
                    peer,
                    root,
                });
                self.save(settings)?;
                self.status("Folder shared");
            }
            Command::RemoveFolder(id) => {
                self.close_all();
                settings.pairs.retain(|p| p.local != id);
                settings.folders.retain(|f| f.id != id);
                self.save(settings)?;
                self.status("Folder sharing stopped");
            }
            Command::AddPair {
                name,
                local,
                remote,
            } => {
                settings::name(&name)?;
                super::safe::valid_id(&remote)?;
                let folder = settings
                    .folders
                    .iter()
                    .find(|f| f.id == local)
                    .context("Choose a local shared folder")?;
                self.authorize(&folder.peer, &folder.workspace)?;
                ensure!(
                    self.view
                        .lock()
                        .unwrap()
                        .catalogs
                        .get(&folder.peer)
                        .is_some_and(|c| c.folders.iter().any(|f| f.id == remote)),
                    "Refresh and choose a folder shared by the other computer"
                );
                ensure!(
                    !settings
                        .pairs
                        .iter()
                        .any(|p| p.local == local && p.remote == remote),
                    "This folder pair already exists"
                );
                settings.pairs.push(Pair {
                    id: settings::new_id()?,
                    name,
                    local,
                    remote,
                    baseline: None,
                    pending: None,
                    run_id: settings::new_id()?,
                    choices: BTreeMap::new(),
                    attempted: false,
                });
                self.save(settings)?;
                self.status("Folder pair saved");
            }
            Command::RemovePair(id) => {
                settings.pairs.retain(|p| p.id != id);
                self.save(settings)?;
                self.status("Folder pair removed");
            }
            Command::RefreshPeer(peer) => {
                self.status("Connecting");
                let (mut connection, _tracked) = self.connect(&peer)?;
                connection.send(&Request::Catalog)?;
                let Response::Catalog(catalog) = response(&mut connection)? else {
                    bail!("Unexpected capability response");
                };
                ensure!(
                    catalog.folders.len() <= 64 && catalog.usb.len() <= 64,
                    "Invalid remote resource list"
                );
                for folder in &catalog.folders {
                    super::safe::valid_id(&folder.id)?;
                    settings::name(&folder.name)?;
                }
                self.change(|view| {
                    view.catalogs.insert(peer, catalog);
                    view.status = "Ready".into();
                });
            }
            Command::Compare(id) => {
                self.status("Comparing folders");
                let (pair, folder) = self.pair(&id)?;
                let lock = self.folder_lock(&folder.id);
                let _lease = lock
                    .try_lock()
                    .map_err(|_| anyhow::anyhow!("Folder is already synchronizing"))?;
                let local = Root::open(&folder.root)?.manifest(&self.cancel)?;
                let (mut connection, _tracked) = self.connect(&folder.peer)?;
                connection.send(&Request::Folder {
                    id: pair.remote,
                    job: super::safe::hash(settings::new_id()?.as_bytes()),
                })?;
                let Response::Manifest(remote) = response(&mut connection)? else {
                    bail!("Unexpected sync manifest response");
                };
                connection.send(&Request::Close)?;
                let plan = sync::compare(local, remote, pair.baseline.as_ref(), &BTreeMap::new())?;
                let saved = settings
                    .pairs
                    .iter_mut()
                    .find(|p| p.id == id)
                    .context("Folder pair was removed")?;
                saved.pending = Some(plan);
                saved.run_id = settings::new_id()?;
                saved.choices.clear();
                saved.attempted = false;
                self.save(settings)?;
                self.status("Review the plan");
            }
            Command::Resolve { pair, choices } => {
                let saved = settings
                    .pairs
                    .iter_mut()
                    .find(|p| p.id == pair)
                    .context("Unknown folder pair")?;
                ensure!(
                    !saved.attempted,
                    "Compare again before changing a partially executed plan"
                );
                let pending = saved
                    .pending
                    .as_ref()
                    .context("Compare the folders first")?;
                saved.choices.extend(choices);
                saved.pending = Some(sync::compare(
                    pending.local.clone(),
                    pending.remote.clone(),
                    saved.baseline.as_ref(),
                    &saved.choices,
                )?);
                self.save(settings)?;
                self.status("Review the plan");
            }
            Command::Run(id) => {
                let (pair, folder) = self.pair(&id)?;
                let plan = pair.pending.as_ref().context("Compare the folders first")?;
                plan.validate()?;
                settings
                    .pairs
                    .iter_mut()
                    .find(|p| p.id == id)
                    .context("Unknown folder pair")?
                    .attempted = true;
                self.save(settings.clone())?;
                let job = super::safe::hash(format!("{}{}", pair.run_id, plan.id()?).as_bytes());
                let lock = self.folder_lock(&folder.id);
                let _lease = lock
                    .try_lock()
                    .map_err(|_| anyhow::anyhow!("Folder is already synchronizing"))?;
                let root = Root::open(&folder.root)?;
                let (mut connection, _tracked) = self.connect(&folder.peer)?;
                connection.send(&Request::Folder {
                    id: pair.remote.clone(),
                    job: job.clone(),
                })?;
                ensure!(
                    matches!(response(&mut connection)?, Response::Manifest(_)),
                    "Unexpected sync session response"
                );
                self.change(|view| {
                    view.status = "Synchronizing".into();
                    view.total = plan.bytes();
                });
                apply_plan(
                    &root,
                    &mut connection,
                    plan,
                    &job,
                    &folder.workspace,
                    &config::hex(&self.identity.public),
                    &folder.id,
                    &self.cancel,
                    |path, bytes| self.progress(path, bytes),
                )?;
                let actual_local = root.manifest(&self.cancel)?;
                connection.send(&Request::Manifest)?;
                let Response::Manifest(actual_remote) = response(&mut connection)? else {
                    bail!("Unexpected final sync manifest");
                };
                let expected = plan.expected();
                ensure!(
                    actual_local == expected.local && actual_remote == expected.remote,
                    "A folder changed during synchronization; compare again before continuing"
                );
                connection.send(&Request::Close)?;
                let saved = settings
                    .pairs
                    .iter_mut()
                    .find(|p| p.id == id)
                    .context("Folder pair was removed")?;
                saved.baseline = Some(expected);
                saved.pending = None;
                saved.choices.clear();
                saved.attempted = false;
                self.save(settings)?;
                self.status("Synchronization complete");
            }
            Command::RefreshUsb => {
                self.status("Reading USB devices");
                let devices = usb::list(usb::backend_address())?;
                self.change(|view| {
                    view.devices = devices;
                    view.status = "Ready".into();
                });
            }
            Command::ShareUsb { device, peer } => {
                self.authorize(&peer, &trust.workspace_id)?;
                ensure!(
                    device.supported && usb::list(usb::backend_address())?.contains(&device),
                    "USB device changed or is unsupported; refresh the device list"
                );
                ensure!(
                    !self.owners.snapshot().contains_key(&device.bus_id),
                    "Detach the USB device before changing its sharing permission"
                );
                settings
                    .usb
                    .retain(|g| g.grant.device.bus_id != device.bus_id);
                settings.usb.push(UsbGrant {
                    workspace: trust.workspace_id,
                    grant: usb::Grant { device, peer },
                });
                self.save(settings)?;
                self.status("USB device shared");
            }
            Command::UnshareUsb(id) => {
                if self.owners.snapshot().contains_key(&id) {
                    self.detaching.lock().unwrap().insert(id.clone());
                }
                self.owners.detach(&id);
                settings.usb.retain(|g| g.grant.device.bus_id != id);
                self.save(settings)?;
                self.status("USB sharing stopped");
            }
            Command::AttachUsb { device, peer } => self.attach_usb(device, peer)?,
            Command::Versions(id) => {
                let folder = settings
                    .folders
                    .iter()
                    .find(|f| f.id == id)
                    .context("Unknown local folder")?;
                let lock = self.folder_lock(&id);
                let _lease = lock
                    .try_lock()
                    .map_err(|_| anyhow::anyhow!("Folder is already synchronizing"))?;
                let versions = Root::open(&folder.root)?.versions(&self.cancel)?;
                self.change(|view| {
                    view.recovery = versions
                        .into_iter()
                        .map(|version| Recovery {
                            folder: id.clone(),
                            version,
                        })
                        .collect();
                    view.status = "Previous versions".into();
                });
            }
            Command::Restore { folder, version } => {
                let saved = settings
                    .folders
                    .iter()
                    .find(|f| f.id == folder)
                    .context("Unknown local folder")?;
                let lock = self.folder_lock(&folder);
                let _lease = lock
                    .try_lock()
                    .map_err(|_| anyhow::anyhow!("Folder is already synchronizing"))?;
                let root = Root::open(&saved.root)?;
                let version = root
                    .versions(&self.cancel)?
                    .into_iter()
                    .find(|v| v.id == version)
                    .context("Unknown previous version")?;
                root.restore(
                    &version,
                    &super::safe::hash(settings::new_id()?.as_bytes()),
                    &self.cancel,
                )?;
                // Existing previews are stale after a user-initiated restore.
                for pair in &mut settings.pairs {
                    if pair.local == folder {
                        pair.pending = None;
                    }
                }
                self.save(settings)?;
                self.status("Version restored");
            }
        }
        Ok(())
    }
    fn attach_usb(self: &Arc<Self>, device: usb::Device, peer: String) -> Result<()> {
        ensure!(
            cfg!(target_os = "linux"),
            "USB import currently requires Ubuntu and the Linux usbip client; Windows can export through usbipd-win"
        );
        usb::bus_id(&device.bus_id)?;
        let key = format!("{peer}/{}", device.bus_id);
        ensure!(
            !self.imports.lock().unwrap().contains_key(&key),
            "USB device is already attached"
        );
        self.status("USB connecting");
        let (mut connection, tracked) = self.connect(&peer)?;
        ok(
            &mut connection,
            &Request::UsbAttach {
                device: device.clone(),
            },
        )?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let bus_id = device.bus_id.clone();
        let (result_send, result_receive) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let _ = result_send.send(usb::attach(port, &bus_id));
        });
        let started = std::time::Instant::now();
        let mut local = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if let Ok(result) = result_receive.try_recv() {
                        result?;
                        bail!("USB/IP client did not open the tunnel");
                    }
                    ensure!(
                        started.elapsed() < std::time::Duration::from_secs(16)
                            && !self.cancel.load(Ordering::Acquire),
                        "USB attach was interrupted or timed out"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(error) => return Err(error.into()),
            }
        };
        local.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
        let mut request = [0; 40];
        local.read_exact(&mut request)?;
        usb::validate_import(&request, &device.bus_id)?;
        connection.send(&Response::Bytes(request.to_vec()))?;
        let Response::Bytes(reply) = response(&mut connection)? else {
            bail!("Expected verified USB import response");
        };
        ensure!(reply.len() == 320, "Invalid USB import response size");
        local.write_all(&reply)?;
        let socket = connection.socket()?;
        self.imports
            .lock()
            .unwrap()
            .insert(key.clone(), socket.try_clone()?);
        let core = self.clone();
        let imported = key.clone();
        let bus_id = device.bus_id.clone();
        std::thread::spawn(move || {
            let _tracked = tracked;
            let result = usb::relay(connection, local, core.stopped.clone());
            let detached = usb::detach_import(port, &bus_id);
            core.imports.lock().unwrap().remove(&imported);
            let requested = core.detaching.lock().unwrap().remove(&imported);
            core.change(|view| {
                view.status = "USB disconnected".into();
                let error = detached
                    .err()
                    .map(|error| format!("{error:#}"))
                    .or_else(|| {
                        if requested {
                            None
                        } else {
                            result
                                .err()
                                .map(|error| format!("USB tunnel ended: {error:#}"))
                        }
                    });
                if let Some(error) = error {
                    view.usb_errors.insert(imported, error.clone());
                    view.error = Some(error);
                }
            });
        });
        match result_receive.recv_timeout(std::time::Duration::from_secs(17)) {
            Ok(Ok(())) => {
                ensure!(
                    self.imports.lock().unwrap().contains_key(&key),
                    "USB device disconnected during attachment"
                );
                self.status("USB attached");
                Ok(())
            }
            result => {
                let _ = socket.shutdown(Shutdown::Both);
                match result {
                    Ok(Err(error)) => Err(error),
                    Err(error) => Err(error.into()),
                    _ => unreachable!(),
                }
            }
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "All authority and cancellation bindings remain explicit at the executor boundary"
)]
fn apply_plan(
    root: &Root,
    connection: &mut Connection,
    plan: &sync::Plan,
    job: &str,
    workspace: &str,
    own: &str,
    folder: &str,
    cancel: &AtomicBool,
    progress: impl Fn(&str, u64),
) -> Result<()> {
    let mut completed = 0;
    for (index, op) in plan.operations.iter().enumerate() {
        ensure!(
            !cancel.load(Ordering::Acquire),
            "Sync paused; resume to continue"
        );
        progress(&op.path, completed);
        let id = operation_id(workspace, own, folder, job, index as u32);
        let (done, mut offset, prefix) = if op.side == Side::Remote {
            connection.send(&Request::Begin {
                index: index as u32,
                operation: op.clone(),
            })?;
            let Response::Ready {
                done,
                offset,
                prefix,
            } = response(connection)?
            else {
                bail!("Unexpected sync checkpoint");
            };
            (done, offset, prefix)
        } else {
            root.begin(&id, op, cancel)?
        };
        if !done {
            if let Change::Set {
                value: value @ sync::Node::File { size, .. },
            } = &op.change
            {
                if op.side == Side::Remote {
                    root.verify_source(&op.path, value, cancel)?;
                    ensure!(
                        offset <= *size && root.source_prefix(&op.path, offset, cancel)? == prefix,
                        "Sync partial failed verification; compare again"
                    );
                    while offset < *size {
                        let bytes = root.read(&op.path, value, offset, cancel)?;
                        let next = offset + bytes.len() as u64;
                        connection.send(&Request::Chunk { offset, bytes })?;
                        ensure!(
                            matches!(response(connection)?, Response::Ack(ack) if ack == next),
                            "Invalid sync chunk acknowledgement"
                        );
                        offset = next;
                        progress(&op.path, completed + offset);
                    }
                } else {
                    ok(
                        connection,
                        &Request::Verify {
                            path: op.path.clone(),
                            value: value.clone(),
                        },
                    )?;
                    connection.send(&Request::Prefix {
                        path: op.path.clone(),
                        offset,
                    })?;
                    ensure!(
                        matches!(response(connection)?, Response::Prefix(hash) if hash == prefix),
                        "Sync partial failed verification; compare again"
                    );
                    while offset < *size {
                        ensure!(
                            !cancel.load(Ordering::Acquire),
                            "Sync paused; resume to continue"
                        );
                        connection.send(&Request::Read {
                            path: op.path.clone(),
                            value: value.clone(),
                            offset,
                        })?;
                        let Response::Bytes(bytes) = response(connection)? else {
                            bail!("Unexpected sync file data");
                        };
                        offset = root.chunk(&id, op, offset, &bytes, cancel)?;
                        progress(&op.path, completed + offset);
                    }
                }
            }
            if op.side == Side::Remote {
                ok(connection, &Request::Finish)?;
            } else {
                root.finish(&id, op, cancel)?;
            }
        }
        if let Change::Set { value } = &op.change {
            completed += value.size();
        }
    }
    progress("", completed);
    Ok(())
}
