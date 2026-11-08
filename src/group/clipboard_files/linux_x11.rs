//! Event-driven native X11 file selection owner. Conditional publication uses
//! one connection for the server-serialized revision check and ownership change.
use super::{GNOME_MIME, LIMIT, Selection, URI_MIME, encode_paths, parse_selection};
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
    path::PathBuf,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        xfixes::{ConnectionExt as _, SelectionEventMask},
        xproto::{
            Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux,
            EventMask, PropMode, Property, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent,
            SelectionRequestEvent, Window, WindowClass,
        },
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

const CALL_TIMEOUT: Duration = Duration::from_secs(5);
const OUTGOING_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_OUTGOING: usize = 16;
const MAX_EVENTS: usize = 1024;

type Reply<T> = mpsc::SyncSender<(u64, Result<T>)>;
enum Command {
    Read(Reply<Option<Selection>>),
    #[cfg(test)]
    RecopySameTimestamp(Reply<()>),
    #[cfg(test)]
    DelayedRead(Duration, Reply<Option<Selection>>),
    #[cfg(test)]
    OwnedTimestamp(Reply<u32>),
    Publish {
        value: Prepared,
        expected: Option<u64>,
        deadline: Instant,
        reply: Reply<bool>,
    },
}

pub(super) struct X11 {
    commands: mpsc::SyncSender<Command>,
    wake: UnixStream,
    pub(super) sequence: u64,
}
impl X11 {
    pub(super) fn new() -> Result<Self> {
        let (wake, waiter) = UnixStream::pair()?;
        wake.set_nonblocking(true)?;
        waiter.set_nonblocking(true)?;
        let (commands, receiver) = mpsc::sync_channel(8);
        let (started, starting) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("Desk Switch X11 file clipboard".into())
            .spawn(move || match Owner::new() {
                Ok(owner) => {
                    if started.send(Ok(())).is_ok() {
                        let _ = owner.run(waiter, receiver);
                    }
                }
                Err(error) => {
                    let _ = started.send(Err(error));
                }
            })?;
        starting
            .recv_timeout(CALL_TIMEOUT)
            .context("Starting the native file clipboard timed out")??;
        Ok(Self {
            commands,
            wake,
            sequence: 0,
        })
    }
    fn queue<T>(&mut self, command: Command, reply: mpsc::Receiver<(u64, Result<T>)>) -> Result<T> {
        self.commands
            .try_send(command)
            .context("The native file clipboard is busy")?;
        match self.wake.write(&[1]) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.into()),
        }
        let (sequence, result) = reply
            .recv_timeout(CALL_TIMEOUT)
            .context("The native file clipboard did not respond")?;
        self.sequence = sequence;
        result
    }
    pub(super) fn read(&mut self) -> Result<Option<Selection>> {
        let (send, reply) = mpsc::sync_channel(1);
        self.queue(Command::Read(send), reply)
    }
    #[cfg(test)]
    pub(super) fn write(&mut self, paths: &[PathBuf]) -> Result<()> {
        self.publish(paths, None).map(|_| ())
    }
    pub(super) fn write_if_current(&mut self, paths: &[PathBuf], expected: u64) -> Result<bool> {
        self.publish(paths, Some(expected))
    }
    fn publish(&mut self, paths: &[PathBuf], expected: Option<u64>) -> Result<bool> {
        let value = Prepared::new(paths)?;
        let (send, reply) = mpsc::sync_channel(1);
        self.queue(
            Command::Publish {
                value,
                expected,
                deadline: Instant::now() + CALL_TIMEOUT,
                reply: send,
            },
            reply,
        )
    }
    #[cfg(test)]
    pub(super) fn recopy_same_timestamp(&mut self) -> Result<()> {
        let (send, reply) = mpsc::sync_channel(1);
        self.queue(Command::RecopySameTimestamp(send), reply)
    }
    #[cfg(test)]
    pub(super) fn timed_out_publication_keeps_selection(
        &mut self,
        paths: &[PathBuf],
    ) -> Result<()> {
        let (send, _reply) = mpsc::sync_channel(1);
        self.commands.try_send(Command::DelayedRead(
            CALL_TIMEOUT + Duration::from_millis(500),
            send,
        ))?;
        // The normal public call supplies its real five-second deadline. Its
        // request must remain rejected when the synthetic delayed read ends.
        ensure!(
            self.publish(paths, None).is_err(),
            "The queued publication did not time out"
        );
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn rejects_old_selection_request(&mut self) -> Result<()> {
        let (send, reply) = mpsc::sync_channel(1);
        let acquired = self.queue(Command::OwnedTimestamp(send), reply)?;
        ensure!(acquired > 1, "The private server timestamp is unavailable");
        let (connection, screen) = x11rb::connect(None)?;
        let window = connection.generate_id()?;
        connection
            .create_window(
                0,
                window,
                connection.setup().roots[screen].root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                0,
                &CreateWindowAux::new(),
            )?
            .check()?;
        let atoms = Atoms::new(&connection)?.reply()?;
        connection.convert_selection(
            window,
            atoms.CLIPBOARD,
            atoms.uri_list,
            atoms.DESK_SWITCH_FILE_COPY,
            acquired.wrapping_sub(1),
        )?;
        connection.flush()?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            ensure!(
                Instant::now() < deadline,
                "The old native paste request did not finish"
            );
            if let Some(event) = connection.poll_for_event()? {
                if let Event::SelectionNotify(event) = event
                    && event.requestor == window
                    && event.target == atoms.uri_list
                {
                    ensure!(
                        event.property == x11rb::NONE,
                        "An old queued paste request received the new ownership's paths"
                    );
                    return Ok(());
                }
                continue;
            }
            let mut fd = libc::pollfd {
                fd: connection.stream().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let millis = deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
                .clamp(1, i32::MAX as u128) as i32;
            let result = unsafe { libc::poll(&mut fd, 1, millis) };
            if result < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error.into());
            }
            ensure!(result > 0, "The old native paste request timed out");
        }
    }
}
impl Drop for X11 {
    fn drop(&mut self) {
        // HUP wakes the otherwise indefinitely sleeping native owner. Closing
        // its X connection releases its selection and all outstanding sessions.
        let _ = self.wake.shutdown(std::net::Shutdown::Both);
    }
}

struct Prepared {
    paths: Vec<PathBuf>,
    uri: Arc<Vec<u8>>,
    gnome: Arc<Vec<u8>>,
    timestamp: u32,
}
impl Prepared {
    fn new(paths: &[PathBuf]) -> Result<Self> {
        // All filesystem work is complete before acquiring a server grab.
        for path in paths {
            let info = std::fs::symlink_metadata(path)?;
            ensure!(
                !info.file_type().is_symlink(),
                "Staged clipboard files must not be symbolic links"
            );
            ensure!(
                info.is_file() || info.is_dir(),
                "Staged clipboard item is not a file or folder"
            );
        }
        let uri = encode_paths(paths)?;
        let text = std::str::from_utf8(&uri)?;
        let gnome = format!("copy\n{}", text.replace("\r\n", "\n")).into_bytes();
        ensure!(
            gnome.len() <= LIMIT,
            "File clipboard exceeds the 1 MiB metadata limit"
        );
        Ok(Self {
            paths: paths.to_vec(),
            uri: Arc::new(uri),
            gnome: Arc::new(gnome),
            timestamp: 0,
        })
    }
}

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        CLIPBOARD,
        TARGETS,
        TIMESTAMP,
        INCR,
        DESK_SWITCH_FILE_COPY,
        uri_list: b"text/uri-list",
        gnome_files: b"x-special/gnome-copied-files",
    }
}

struct Outgoing {
    target: Atom,
    bytes: Arc<Vec<u8>>,
    offset: usize,
    deadline: Instant,
}

/// This guard never invokes another X connection, filesystem work or callbacks.
/// Its Drop runs on every success/error/early-return path after a grab attempt.
struct ServerGrab(Arc<RustConnection>);
impl ServerGrab {
    fn new(connection: Arc<RustConnection>) -> Result<Self> {
        let guard = Self(connection);
        guard.0.grab_server()?.check()?;
        Ok(guard)
    }
}
impl Drop for ServerGrab {
    fn drop(&mut self) {
        let _ = self.0.ungrab_server();
        let _ = self.0.flush();
    }
}

struct Owner {
    connection: Arc<RustConnection>,
    window: Window,
    root: Window,
    atoms: Atoms,
    sequence: u64,
    dirty: bool,
    paths: Option<Vec<PathBuf>>,
    last_owner: Option<Window>,
    owned: Option<Prepared>,
    inline: usize,
    outgoing: BTreeMap<(Window, Atom), Outgoing>,
    deferred: VecDeque<Event>,
}
impl Owner {
    fn new() -> Result<Self> {
        let (connection, screen) = x11rb::connect(None)?;
        let root = connection.setup().roots[screen].root;
        let request_bytes = usize::from(connection.setup().maximum_request_length) * 4;
        ensure!(
            request_bytes > 128,
            "The X11 server request limit is too small"
        );
        let inline = (request_bytes - 64).min(48 * 1024);
        let window = connection.generate_id()?;
        connection
            .create_window(
                0,
                window,
                root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                0,
                &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )?
            .check()?;
        let atoms = Atoms::new(&connection)?.reply()?;
        connection.xfixes_query_version(5, 0)?.reply()?;
        connection
            .xfixes_select_selection_input(
                window,
                atoms.CLIPBOARD,
                SelectionEventMask::SET_SELECTION_OWNER
                    | SelectionEventMask::SELECTION_WINDOW_DESTROY
                    | SelectionEventMask::SELECTION_CLIENT_CLOSE,
            )?
            .check()?;
        connection.flush()?;
        Ok(Self {
            connection: Arc::new(connection),
            window,
            root,
            atoms,
            sequence: 1,
            dirty: true,
            paths: None,
            last_owner: None,
            owned: None,
            inline,
            outgoing: BTreeMap::new(),
            deferred: VecDeque::with_capacity(MAX_EVENTS * 2),
        })
    }
    fn run(mut self, mut wake: UnixStream, commands: mpsc::Receiver<Command>) -> Result<()> {
        loop {
            self.expire_outgoing();
            self.drain()?;
            for _ in 0..8 {
                match commands.try_recv() {
                    Ok(Command::Read(reply)) => {
                        let result = self.read();
                        let _ = reply.send((self.sequence, result));
                    }
                    #[cfg(test)]
                    Ok(Command::RecopySameTimestamp(reply)) => {
                        let result = self.recopy_same_timestamp();
                        let _ = reply.send((self.sequence, result));
                    }
                    #[cfg(test)]
                    Ok(Command::DelayedRead(duration, reply)) => {
                        std::thread::sleep(duration);
                        let result = self.read();
                        let _ = reply.send((self.sequence, result));
                    }
                    #[cfg(test)]
                    Ok(Command::OwnedTimestamp(reply)) => {
                        let result = self
                            .owned
                            .as_ref()
                            .map(|value| value.timestamp)
                            .context("The private native selection is not owned");
                        let _ = reply.send((self.sequence, result));
                    }
                    Ok(Command::Publish {
                        value,
                        expected,
                        deadline,
                        reply,
                    }) => {
                        let result = self.publish(value, expected, deadline);
                        let _ = reply.send((self.sequence, result));
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
                }
            }
            // Native requests may have been buffered while servicing a
            // synchronous command. Do not sleep with queued paste requests.
            self.drain()?;
            let timeout = self
                .outgoing
                .values()
                .map(|value| value.deadline)
                .min()
                .map_or(-1, |deadline| {
                    deadline
                        .saturating_duration_since(Instant::now())
                        .as_millis()
                        .clamp(1, i32::MAX as u128) as i32
                });
            let mut fds = [
                libc::pollfd {
                    fd: self.connection.stream().as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: wake.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout) };
            if ready < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error.into());
            }
            ensure!(
                fds[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) == 0,
                "The native clipboard connection was closed"
            );
            if fds[1].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                return Ok(());
            }
            if fds[1].revents & libc::POLLIN != 0 {
                let mut bytes = [0; 64];
                loop {
                    match wake.read(&mut bytes) {
                        Ok(0) => return Ok(()),
                        Ok(_) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(error) => return Err(error.into()),
                    }
                }
            }
        }
    }
    fn note_revision(&mut self, event: &Event) -> bool {
        if let Event::XfixesSelectionNotify(event) = event
            && event.window == self.window
            && event.selection == self.atoms.CLIPBOARD
        {
            // Count notifications, not timestamps: two SetSelectionOwner calls
            // by the same owner in one server millisecond are distinct copies.
            self.sequence = self.sequence.wrapping_add(1).max(1);
            self.dirty = true;
            if event.owner == self.window
                && let Some(owned) = &mut self.owned
            {
                owned.timestamp = event.selection_timestamp;
            } else if event.owner != self.window {
                self.owned = None;
            }
            return true;
        }
        false
    }
    fn handle(&mut self, event: &Event) -> Result<()> {
        if self.note_revision(event) {
            return Ok(());
        }
        match event {
            Event::SelectionRequest(request)
                if request.owner == self.window && request.selection == self.atoms.CLIPBOARD =>
            {
                // A requestor can disappear between the event and our reply.
                // That must not terminate the application's native clipboard.
                let _ = self.respond(request);
            }
            // XFixes above orders ownership changes accurately. A delayed
            // SelectionClear can describe a previous ownership, so it must not
            // discard files published after that change.
            Event::PropertyNotify(event) if event.state == Property::DELETE => {
                let _ = self.advance_outgoing((event.window, event.atom));
            }
            Event::DestroyNotify(event) => {
                self.outgoing
                    .retain(|(window, _), _| *window != event.window);
            }
            _ => {}
        }
        Ok(())
    }
    fn drain(&mut self) -> Result<()> {
        for _ in 0..MAX_EVENTS {
            let event = if let Some(event) = self.deferred.pop_front() {
                Some(event)
            } else {
                self.connection.poll_for_event()?
            };
            let Some(event) = event else {
                return Ok(());
            };
            self.handle(&event)?;
        }
        bail!("The native clipboard is busy; retry the file copy")
    }
    fn drain_revisions_while_grabbed(&mut self) -> Result<()> {
        for _ in 0..MAX_EVENTS {
            let Some(event) = self.connection.poll_for_event()? else {
                return Ok(());
            };
            if !self.note_revision(&event) {
                ensure!(
                    self.deferred.len() < MAX_EVENTS * 2,
                    "The native clipboard is busy"
                );
                self.deferred.push_back(event);
            }
        }
        bail!("The native clipboard is busy; retry the file copy")
    }
    fn publish(
        &mut self,
        value: Prepared,
        expected: Option<u64>,
        deadline: Instant,
    ) -> Result<bool> {
        self.drain()?;
        let paths = value.paths.clone();
        // Only our connection is used while grabbed. This short critical region
        // contains native synchronization/check/set requests and bounded event
        // bookkeeping; no file access, other client call, or await is involved.
        let grab = ServerGrab::new(self.connection.clone())?;
        let owner = self
            .connection
            .get_selection_owner(self.atoms.CLIPBOARD)?
            .reply()?
            .owner;
        self.drain_revisions_while_grabbed()?;
        if Instant::now() >= deadline
            || expected
                .is_some_and(|revision| revision != self.sequence || self.last_owner != Some(owner))
        {
            drop(grab);
            return Ok(false);
        }
        let previous = self.owned.replace(value);
        let result = (|| {
            self.connection
                .set_selection_owner(self.window, self.atoms.CLIPBOARD, x11rb::CURRENT_TIME)?
                .check()?;
            ensure!(
                self.connection
                    .get_selection_owner(self.atoms.CLIPBOARD)?
                    .reply()?
                    .owner
                    == self.window,
                "The native clipboard ownership change was rejected"
            );
            self.drain_revisions_while_grabbed()?;
            Ok::<_, anyhow::Error>(())
        })();
        if let Err(error) = result {
            self.owned = previous;
            return Err(error);
        }
        self.paths = Some(paths);
        self.last_owner = Some(self.window);
        self.dirty = false;
        drop(grab);
        Ok(true)
    }
    #[cfg(test)]
    fn recopy_same_timestamp(&mut self) -> Result<()> {
        let timestamp = self
            .owned
            .as_ref()
            .context("No private test selection is owned")?
            .timestamp;
        ensure!(
            timestamp != 0,
            "The private server did not report the ownership timestamp"
        );
        // Equal acquisition time is legal. This produces a distinct XFixes
        // notification even though owner, bytes and millisecond are identical.
        self.connection
            .set_selection_owner(self.window, self.atoms.CLIPBOARD, timestamp)?
            .check()?;
        self.drain()?;
        Ok(())
    }
    fn selection(&self) -> Option<Selection> {
        self.paths.clone().map(|paths| Selection {
            sequence: self.sequence,
            paths,
        })
    }
    fn read(&mut self) -> Result<Option<Selection>> {
        self.drain()?;
        if !self.dirty {
            return Ok(self.selection());
        }
        let owner = self
            .connection
            .get_selection_owner(self.atoms.CLIPBOARD)?
            .reply()?
            .owner;
        if owner == x11rb::NONE {
            self.paths = None;
            self.last_owner = Some(owner);
            self.dirty = false;
            return Ok(None);
        }
        if owner == self.window
            && let Some(value) = &self.owned
        {
            self.paths = Some(value.paths.clone());
            self.last_owner = Some(owner);
            self.dirty = false;
            return Ok(self.selection());
        }
        let initial = self.sequence;
        let value = if let Some(bytes) = self.convert(self.atoms.gnome_files)? {
            parse_selection(GNOME_MIME, &bytes)?
        } else if let Some(bytes) = self.convert(self.atoms.uri_list)? {
            parse_selection(URI_MIME, &bytes)?
        } else {
            None
        };
        self.drain()?;
        ensure!(
            self.sequence == initial
                && self
                    .connection
                    .get_selection_owner(self.atoms.CLIPBOARD)?
                    .reply()?
                    .owner
                    == owner,
            "The clipboard changed; retry the file copy"
        );
        self.paths = value;
        self.last_owner = Some(owner);
        self.dirty = false;
        Ok(self.selection())
    }
    fn respond(&mut self, request: &SelectionRequestEvent) -> Result<()> {
        let property = if request.property == x11rb::NONE {
            request.target
        } else {
            request.property
        };
        let mut accepted = false;
        if let Some(value) = &self.owned
            && valid_request_time(request.time, value.timestamp)
        {
            if request.target == self.atoms.TARGETS {
                self.connection
                    .change_property32(
                        PropMode::REPLACE,
                        request.requestor,
                        property,
                        AtomEnum::ATOM,
                        &[
                            self.atoms.TARGETS,
                            self.atoms.TIMESTAMP,
                            self.atoms.uri_list,
                            self.atoms.gnome_files,
                        ],
                    )?
                    .check()?;
                accepted = true;
            } else if request.target == self.atoms.TIMESTAMP {
                self.connection
                    .change_property32(
                        PropMode::REPLACE,
                        request.requestor,
                        property,
                        AtomEnum::INTEGER,
                        &[value.timestamp],
                    )?
                    .check()?;
                accepted = true;
            } else {
                let bytes = if request.target == self.atoms.uri_list {
                    Some(value.uri.clone())
                } else if request.target == self.atoms.gnome_files {
                    Some(value.gnome.clone())
                } else {
                    None
                };
                if let Some(bytes) = bytes {
                    if bytes.len() <= self.inline {
                        self.connection
                            .change_property8(
                                PropMode::REPLACE,
                                request.requestor,
                                property,
                                request.target,
                                &bytes,
                            )?
                            .check()?;
                        accepted = true;
                    } else if self.outgoing.len() < MAX_OUTGOING {
                        self.connection
                            .change_window_attributes(
                                request.requestor,
                                &ChangeWindowAttributesAux::new().event_mask(
                                    EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY,
                                ),
                            )?
                            .check()?;
                        self.connection
                            .change_property32(
                                PropMode::REPLACE,
                                request.requestor,
                                property,
                                self.atoms.INCR,
                                &[bytes.len() as u32],
                            )?
                            .check()?;
                        self.outgoing.insert(
                            (request.requestor, property),
                            Outgoing {
                                target: request.target,
                                bytes,
                                offset: 0,
                                deadline: Instant::now() + OUTGOING_TIMEOUT,
                            },
                        );
                        accepted = true;
                    }
                }
            }
        }
        let notification = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: request.time,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property: if accepted { property } else { x11rb::NONE },
        };
        self.connection
            .send_event(false, request.requestor, EventMask::NO_EVENT, notification)?
            .check()?;
        self.connection.flush()?;
        Ok(())
    }
    fn stop_watching(&self, window: Window) {
        if !self
            .outgoing
            .keys()
            .any(|(requestor, _)| *requestor == window)
        {
            let _ = self.connection.change_window_attributes(
                window,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::NO_EVENT),
            );
        }
    }
    fn advance_outgoing(&mut self, key: (Window, Atom)) -> Result<()> {
        let Some(mut value) = self.outgoing.remove(&key) else {
            return Ok(());
        };
        let end = (value.offset + self.inline).min(value.bytes.len());
        let chunk = &value.bytes[value.offset..end];
        let result = self
            .connection
            .change_property8(PropMode::REPLACE, key.0, key.1, value.target, chunk)?
            .check();
        if result.is_err() || chunk.is_empty() {
            self.stop_watching(key.0);
        } else {
            value.offset = end;
            self.outgoing.insert(key, value);
        }
        self.connection.flush()?;
        result.map_err(Into::into)
    }
    fn expire_outgoing(&mut self) {
        let now = Instant::now();
        let expired = self
            .outgoing
            .iter()
            .filter(|(_, value)| value.deadline <= now)
            .map(|(key, _)| *key)
            .collect::<Vec<_>>();
        for key in expired {
            self.outgoing.remove(&key);
            self.stop_watching(key.0);
        }
        let _ = self.connection.flush();
    }
    fn wait_event(&mut self, deadline: Instant) -> Result<Event> {
        loop {
            ensure!(
                Instant::now() < deadline,
                "Reading the native file clipboard timed out"
            );
            let event = if let Some(event) = self.deferred.pop_front() {
                Some(event)
            } else {
                self.connection.poll_for_event()?
            };
            if let Some(event) = event {
                self.handle(&event)?;
                return Ok(event);
            }
            let mut fd = libc::pollfd {
                fd: self.connection.stream().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let wait = deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
                .clamp(1, i32::MAX as u128) as i32;
            let result = unsafe { libc::poll(&mut fd, 1, wait) };
            if result < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error.into());
            }
            ensure!(result != 0, "Reading the native file clipboard timed out");
            ensure!(
                fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) == 0,
                "The native clipboard connection was closed"
            );
        }
    }
    fn property(
        &self,
        window: Window,
        remaining: usize,
    ) -> Result<x11rb::protocol::xproto::GetPropertyReply> {
        let reply = self
            .connection
            .get_property(
                true,
                window,
                self.atoms.DESK_SWITCH_FILE_COPY,
                AtomEnum::ANY,
                0,
                ((remaining + 4) / 4) as u32,
            )?
            .reply()?;
        ensure!(
            reply.bytes_after == 0 && reply.value.len() <= remaining,
            "File clipboard exceeds the 1 MiB metadata limit"
        );
        Ok(reply)
    }
    fn convert(&mut self, target: Atom) -> Result<Option<Vec<u8>>> {
        let window = self.connection.generate_id()?;
        self.connection
            .create_window(
                0,
                window,
                self.root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                0,
                &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )?
            .check()?;
        let result = self.convert_inner(window, target);
        let _ = self.connection.destroy_window(window);
        let _ = self.connection.flush();
        result
    }
    fn convert_inner(&mut self, window: Window, target: Atom) -> Result<Option<Vec<u8>>> {
        let deadline = Instant::now() + Duration::from_millis(1500);
        self.connection.convert_selection(
            window,
            self.atoms.CLIPBOARD,
            target,
            self.atoms.DESK_SWITCH_FILE_COPY,
            x11rb::CURRENT_TIME,
        )?;
        self.connection.flush()?;
        loop {
            if let Event::SelectionNotify(event) = self.wait_event(deadline)?
                && event.requestor == window
                && event.selection == self.atoms.CLIPBOARD
                && event.target == target
            {
                if event.property == x11rb::NONE {
                    return Ok(None);
                }
                ensure!(
                    event.property == self.atoms.DESK_SWITCH_FILE_COPY,
                    "The native file clipboard returned an unexpected property"
                );
                break;
            }
        }
        let reply = self.property(window, LIMIT)?;
        if reply.type_ == target {
            ensure!(
                reply.format == 8,
                "The native file clipboard format is invalid"
            );
            return Ok(Some(reply.value));
        }
        ensure!(
            reply.type_ == self.atoms.INCR && reply.format == 32,
            "The native file clipboard returned an unsupported format"
        );
        let advertised = reply
            .value32()
            .and_then(|mut values| values.next())
            .unwrap_or(0) as usize;
        ensure!(
            advertised <= LIMIT,
            "File clipboard exceeds the 1 MiB metadata limit"
        );
        self.connection.flush()?;
        let mut bytes = Vec::new();
        loop {
            if let Event::PropertyNotify(event) = self.wait_event(deadline)?
                && event.window == window
                && event.atom == self.atoms.DESK_SWITCH_FILE_COPY
                && event.state == Property::NEW_VALUE
            {
                let reply = self.property(window, LIMIT - bytes.len())?;
                ensure!(
                    reply.type_ == target && reply.format == 8,
                    "The native file clipboard changed format during transfer"
                );
                if reply.value.is_empty() {
                    return Ok(Some(bytes));
                }
                bytes.extend_from_slice(&reply.value);
                self.connection.flush()?;
            }
        }
    }
}

fn valid_request_time(request: u32, acquired: u32) -> bool {
    // ICCCM permits CurrentTime. Otherwise X timestamps use wrap-safe signed
    // differences within half their 32-bit range, not ordinary integer order.
    request == x11rb::CURRENT_TIME || (request.wrapping_sub(acquired) as i32) >= 0
}

#[cfg(test)]
mod tests {
    use super::valid_request_time;

    #[test]
    fn selection_requests_respect_acquisition_time_across_wraparound() {
        assert!(valid_request_time(0, 100));
        assert!(valid_request_time(100, 100));
        assert!(valid_request_time(101, 100));
        assert!(!valid_request_time(99, 100));
        assert!(valid_request_time(4, u32::MAX - 4));
        assert!(!valid_request_time(u32::MAX - 4, 4));
    }
}
