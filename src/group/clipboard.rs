//! Opt-in clipboard sharing. The worker owns the OS clipboard and never blocks
//! the GUI. Wire hashes verify bytes; decoded fingerprints suppress
//! echoes even when the OS re-encodes an image.
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

pub type Content = (String, Vec<u8>, String);
const LIMIT: usize = 8 * 1024 * 1024;
const SIDE: usize = 4096;
const INTERVAL: Duration = Duration::from_millis(400);
const PUBLISH_RETRY: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, PartialEq)]
enum Value {
    Text(String),
    Image {
        width: usize,
        height: usize,
        rgba: Vec<u8>,
    },
}
impl Value {
    fn fingerprint(&self) -> String {
        let mut digest = Sha256::new();
        match self {
            Self::Text(text) => {
                digest.update(b"text/plain\0");
                digest.update(text.as_bytes());
            }
            Self::Image {
                width,
                height,
                rgba,
            } => {
                digest.update(b"image/rgba\0");
                digest.update((*width as u64).to_le_bytes());
                digest.update((*height as u64).to_le_bytes());
                digest.update(rgba);
            }
        }
        format!("{digest:x}", digest = digest.finalize())
    }
    fn encode(&self) -> Result<Content> {
        let (mime, bytes) = match self {
            Self::Text(text) => ("text/plain", text.as_bytes().to_vec()),
            Self::Image {
                width,
                height,
                rgba,
            } => {
                validate_image(*width, *height, rgba.len())?;
                let mut bytes = Vec::new();
                use image::ImageEncoder;
                image::codecs::png::PngEncoder::new_with_quality(
                    &mut bytes,
                    image::codecs::png::CompressionType::Fast,
                    image::codecs::png::FilterType::Adaptive,
                )
                .write_image(
                    rgba,
                    *width as u32,
                    *height as u32,
                    image::ExtendedColorType::Rgba8,
                )?;
                ("image/png", bytes)
            }
        };
        ensure!(
            bytes.len() <= LIMIT,
            "Clipboard content exceeds the 8 MiB sharing limit"
        );
        let digest = hash(&bytes);
        Ok((mime.into(), bytes, digest))
    }
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn validate_image(width: usize, height: usize, len: usize) -> Result<()> {
    ensure!(
        width > 0
            && height > 0
            && width <= SIDE
            && height <= SIDE
            && width
                .checked_mul(height)
                .and_then(|pixels| pixels.checked_mul(4))
                == Some(len),
        "Clipboard image exceeds 4096 × 4096 pixels or is damaged"
    );
    Ok(())
}
fn decode((mime, bytes, digest): Content) -> Result<Value> {
    ensure!(
        bytes.len() <= LIMIT && hash(&bytes) == digest,
        "Clipboard checksum or size is invalid"
    );
    match mime.as_str() {
        "text/plain" => Ok(Value::Text(
            String::from_utf8(bytes).context("Clipboard text is not UTF-8")?,
        )),
        "image/png" => {
            let mut reader = image::ImageReader::with_format(
                std::io::Cursor::new(bytes),
                image::ImageFormat::Png,
            );
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(SIDE as u32);
            limits.max_image_height = Some(SIDE as u32);
            limits.max_alloc = Some(64 * 1024 * 1024);
            reader.limits(limits);
            let image = reader
                .decode()
                .context("Clipboard PNG is damaged or too large")?
                .to_rgba8();
            Ok(Value::Image {
                width: image.width() as usize,
                height: image.height() as usize,
                rgba: image.into_raw(),
            })
        }
        _ => anyhow::bail!("Shared clipboard supports text and PNG images only"),
    }
}

trait Backend {
    fn read(&mut self) -> Result<Option<Value>>;
    fn write(&mut self, value: &Value) -> Result<()>;
}
struct Native {
    clipboard: arboard::Clipboard,
    sequence: Option<u64>,
    error: Option<String>,
}
#[cfg(windows)]
fn native_sequence() -> Option<u64> {
    // SAFETY: the function takes no arguments, returns a plain counter and
    // transfers no ownership.
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetClipboardSequenceNumber() -> u32;
    }
    // SAFETY: the call has no pointer arguments and only reads the clipboard
    // sequence counter.
    let sequence = unsafe { GetClipboardSequenceNumber() };
    (sequence != 0).then_some(u64::from(sequence))
}
#[cfg(target_os = "macos")]
fn native_sequence() -> Option<u64> {
    use objc2::{
        ClassType, msg_send,
        rc::{Retained, autoreleasepool},
    };
    use objc2_app_kit::NSPasteboard;
    autoreleasepool(|_| {
        // NSPasteboard can be used on the clipboard worker, just as arboard
        // does. Match its nullable lookup for non-GUI/launchd environments.
        let pasteboard: Option<Retained<NSPasteboard>> =
            // SAFETY: the message takes no arguments and returns a retained
            // pasteboard, which the optional keeps owned.
            unsafe { msg_send![NSPasteboard::class(), generalPasteboard] };
        pasteboard.and_then(|pasteboard| u64::try_from(pasteboard.changeCount()).ok())
    })
}
#[cfg(not(any(windows, target_os = "macos")))]
fn native_sequence() -> Option<u64> {
    None
}
impl Native {
    fn read_value(&mut self) -> Result<Option<Value>> {
        // A copied image may also advertise a URL/text description. Prefer its
        // actual image rather than sending that ancillary text.
        match self.clipboard.get_image() {
            Ok(image) => {
                validate_image(image.width, image.height, image.bytes.len())?;
                return Ok(Some(Value::Image {
                    width: image.width,
                    height: image.height,
                    rgba: image.bytes.into_owned(),
                }));
            }
            Err(arboard::Error::ContentNotAvailable | arboard::Error::ConversionFailure) => {}
            Err(error) => return Err(error.into()),
        }
        match self.clipboard.get_text() {
            Ok(text) => {
                ensure!(
                    text.len() <= LIMIT,
                    "Clipboard text exceeds the 8 MiB sharing limit"
                );
                Ok(Some(Value::Text(text)))
            }
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}
impl Backend for Native {
    fn read(&mut self) -> Result<Option<Value>> {
        let sequence = native_sequence();
        if sequence.is_some() && sequence == self.sequence {
            if let Some(error) = &self.error {
                anyhow::bail!("{error}");
            }
            return Ok(None);
        }
        let result = self.read_value();
        let occupied = result
            .as_ref()
            .err()
            .and_then(|error| error.downcast_ref::<arboard::Error>())
            .is_some_and(|error| matches!(error, arboard::Error::ClipboardOccupied));
        if !occupied {
            self.sequence = sequence;
            self.error = result.as_ref().err().map(ToString::to_string);
        }
        result
    }
    fn write(&mut self, value: &Value) -> Result<()> {
        match value {
            Value::Text(text) => self.clipboard.set_text(text.clone())?,
            Value::Image {
                width,
                height,
                rgba,
            } => self.clipboard.set_image(arboard::ImageData {
                width: *width,
                height: *height,
                bytes: Cow::Borrowed(rgba),
            })?,
        }
        self.sequence = None;
        self.error = None;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod gnome {
    use super::*;
    const NAME: &str = "io.github.siarheisinelnikau.DeskSwitch.Clipboard";
    const PATH: &str = "/io/github/siarheisinelnikau/DeskSwitch/Clipboard";
    pub(super) struct Gnome {
        connection: zbus::blocking::Connection,
        serial: u32,
        error: Option<String>,
    }
    impl Gnome {
        pub fn new() -> Result<Self> {
            let connection = zbus::blocking::connection::Builder::session()?
                .method_timeout(Duration::from_secs(2))
                .build()?;
            let proxy = zbus::blocking::Proxy::new(&connection, NAME, PATH, NAME)?;
            let version: u32 = proxy.call("Version", &())?;
            ensure!(
                matches!(version, 1 | 2),
                "Unsupported GNOME clipboard companion"
            );
            Ok(Self {
                connection,
                serial: 0,
                error: None,
            })
        }
    }
    impl Backend for Gnome {
        fn read(&mut self) -> Result<Option<Value>> {
            let proxy = zbus::blocking::Proxy::new(&self.connection, NAME, PATH, NAME)?;
            let (serial, mime, bytes): (u32, String, Vec<u8>) =
                match proxy.call("Read", &(self.serial,)) {
                    Ok(content) => content,
                    Err(error) => {
                        // Enabling/restarting the extension restarts its serial.
                        // A failed call must make the next successful one fetch
                        // content even if that new serial happens to match ours.
                        self.serial = 0;
                        self.error = None;
                        return Err(error.into());
                    }
                };
            if mime.is_empty() {
                if self.serial == serial
                    && let Some(error) = &self.error
                {
                    anyhow::bail!("{error}");
                }
                self.serial = serial;
                self.error = None;
                return Ok(None);
            }
            let digest = hash(&bytes);
            self.serial = serial;
            let value = decode((mime, bytes, digest));
            self.error = value.as_ref().err().map(ToString::to_string);
            value.map(Some)
        }
        fn write(&mut self, value: &Value) -> Result<()> {
            let (mime, bytes, _) = value.encode()?;
            let proxy = zbus::blocking::Proxy::new(&self.connection, NAME, PATH, NAME)?;
            self.serial = proxy.call("Write", &(mime, bytes))?;
            self.error = None;
            Ok(())
        }
    }
    pub fn required() -> bool {
        let wayland = std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland")
            || std::env::var_os("WAYLAND_DISPLAY").is_some();
        let desktop = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_ascii_lowercase();
        wayland && (desktop.contains("gnome") || desktop.contains("ubuntu"))
    }
}
fn backend() -> Result<Box<dyn Backend>> {
    #[cfg(target_os = "linux")]
    if gnome::required() {
        return gnome::Gnome::new()
            .map(|backend| Box::new(backend) as Box<dyn Backend>)
            .context(
                "Enable Desk Switch Tray in GNOME Extensions to share text and images on Wayland",
            );
    }
    Ok(Box::new(Native {
        clipboard: arboard::Clipboard::new()?,
        sequence: None,
        error: None,
    }))
}

#[derive(Default)]
struct Engine {
    fingerprint: Option<String>,
    pending: Option<Value>,
    rejected: Option<String>,
}
impl Engine {
    fn receive(&mut self, content: Content) -> Result<()> {
        self.pending = Some(decode(content)?);
        Ok(())
    }
    fn discard_file_ack(&mut self, file_present: bool) -> bool {
        if file_present
            && self
                .pending
                .as_ref()
                .is_some_and(|value| self.fingerprint.as_ref() == Some(&value.fingerprint()))
        {
            self.pending = None;
            true
        } else {
            false
        }
    }
    fn guarded_step(
        &mut self,
        backend: &mut dyn Backend,
        file_present: bool,
    ) -> Result<Option<Content>> {
        self.discard_file_ack(file_present);
        if file_present && self.pending.is_none() {
            return Ok(None);
        }
        self.step(backend)
    }
    fn step(&mut self, backend: &mut dyn Backend) -> Result<Option<Content>> {
        if let Some(value) = self.pending.as_ref() {
            self.rejected = None;
            let fingerprint = value.fingerprint();
            if self.fingerprint.as_ref() != Some(&fingerprint) {
                backend.write(value)?;
                self.fingerprint = Some(fingerprint);
                // Some native formats normalize pixels/text on insertion.
                // Read back once rather than echoing the new encoding.
                if let Ok(Some(observed)) = backend.read() {
                    self.fingerprint = Some(observed.fingerprint());
                }
            }
            // The controller also acknowledges our own copies. Keep the
            // originator's rich/native formats when its plain content matches.
            self.pending = None;
        }
        let Some(value) = backend.read()? else {
            if let Some(error) = &self.rejected {
                anyhow::bail!("{error}");
            }
            return Ok(None);
        };
        let fingerprint = value.fingerprint();
        if self.fingerprint.as_ref() == Some(&fingerprint) {
            if let Some(error) = &self.rejected {
                anyhow::bail!("{error}");
            }
            return Ok(None);
        }
        // Do not repeatedly re-encode a too-large unchanged image.
        self.fingerprint = Some(fingerprint);
        let result = value.encode();
        self.rejected = result.as_ref().err().map(ToString::to_string);
        result.map(Some)
    }
}

#[derive(Default)]
struct State {
    enabled: bool,
    generation: u64,
    stop: bool,
    incoming: Option<(u64, Content)>,
    outgoing: Option<(u64, Content)>,
    status: Option<String>,
    files_begin: Option<(u64, String, Option<u64>)>,
    files_incoming: Option<(u64, String, Vec<PathBuf>)>,
    files_outgoing: Option<(u64, Option<super::clipboard_files::Selection>)>,
    files_offer: Option<(
        u64,
        super::clipboard_files::provider::RemoteOffer,
        Option<u64>,
    )>,
    files_ready: VecDeque<(String, bool)>,
    files_offer_retry_at: Option<Instant>,
    native_revision: Option<u64>,
    files_cancel: Option<(u64, String)>,
}

/// Native file-drop metadata never enters the text/image wire protocol. A
/// reservation protects a newer local copy while the remote files download.
#[derive(Default)]
struct FileEngine {
    observed: Option<super::clipboard_files::Selection>,
    reservation: Option<String>,
    pending: Option<(String, Vec<PathBuf>)>,
    revision: Option<u64>,
    written: Option<Vec<PathBuf>>,
}
impl FileEngine {
    fn observe(
        &mut self,
        selection: Option<super::clipboard_files::Selection>,
        revision: Option<u64>,
    ) -> Option<Vec<PathBuf>> {
        if self.observed == selection && self.revision == revision {
            return None;
        }
        self.revision = revision;
        let own_write = self
            .written
            .take()
            .is_some_and(|paths| selection.as_ref().is_some_and(|value| value.paths == paths));
        self.observed = selection.clone();
        self.reservation = None;
        self.pending = None;
        if own_write {
            return None;
        }
        selection
            .filter(|value| !value.paths.is_empty())
            .map(|value| value.paths)
    }
    fn invalidate(&mut self) {
        self.reservation = None;
        self.pending = None;
    }
    fn reserve(&mut self, id: String, expected: Option<u64>, local_change: bool) -> bool {
        if expected.is_some() && expected == self.revision && !local_change {
            self.reservation = Some(id);
            true
        } else {
            false
        }
    }
}
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}
pub struct Clipboard {
    shared: Arc<Shared>,
}
impl Clipboard {
    pub fn start(ctx: eframe::egui::Context) -> Result<Self> {
        Self::spawn_with_files(ctx, backend, true)
    }
    #[cfg(test)]
    fn spawn(
        ctx: eframe::egui::Context,
        factory: impl FnMut() -> Result<Box<dyn Backend>> + Send + 'static,
    ) -> Result<Self> {
        Self::spawn_with_files(ctx, factory, false)
    }
    fn spawn_with_files(
        ctx: eframe::egui::Context,
        mut factory: impl FnMut() -> Result<Box<dyn Backend>> + Send + 'static,
        native_files: bool,
    ) -> Result<Self> {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("Desk Switch shared clipboard".into())
            .spawn(move || {
                let mut native: Option<Box<dyn Backend>> = None;
                let mut engine = Engine::default();
                let mut generation = 0;
                let mut files = FileEngine::default();
                let mut published: Option<(
                    String,
                    Box<dyn super::clipboard_files::provider::PublishedOffer>,
                )> = None;
                loop {
                    let mut state = worker.state.lock().unwrap();
                    while !state.enabled && !state.stop {
                        state = worker.wake.wait(state).unwrap();
                    }
                    if state.stop {
                        break;
                    }
                    let current = state.generation;
                    if current != generation {
                        engine = Engine::default();
                        files = FileEngine::default();
                        published = None;
                        generation = current;
                    }
                    let content = state
                        .incoming
                        .take()
                        .filter(|(epoch, _)| *epoch == current)
                        .map(|(_, content)| content);
                    let begin = state
                        .files_begin
                        .take()
                        .filter(|(epoch, _, _)| *epoch == current)
                        .map(|(_, id, expected)| (id, expected));
                    let incoming_files = state
                        .files_incoming
                        .take()
                        .filter(|(epoch, _, _)| *epoch == current)
                        .map(|(_, id, paths)| (id, paths));
                    let cancelled_files = state
                        .files_cancel
                        .take()
                        .filter(|(epoch, _)| *epoch == current)
                        .map(|(_, id)| id);
                    let retry_ready = state
                        .files_offer_retry_at
                        .is_none_or(|at| Instant::now() >= at);
                    let offered = if retry_ready {
                        state.files_offer_retry_at = None;
                        state
                            .files_offer
                            .take()
                            .filter(|(epoch, _, _)| *epoch == current)
                            .map(|(_, offer, expected)| (offer, expected))
                    } else {
                        None
                    };
                    let retry_offer = offered.clone();
                    let mut offer_attempted = false;
                    let mut publication_failed = false;
                    drop(state);
                    let mut outgoing_files: Option<Option<super::clipboard_files::Selection>> =
                        None;
                    let mut ready = Vec::new();
                    let result = (|| {
                        // Disabling cancels queued operations. An OS call already
                        // in flight finishes on this worker and never holds the UI.
                        {
                            let state = worker.state.lock().unwrap();
                            if !state.enabled || state.generation != current {
                                return Ok(None);
                            }
                        }
                        let receiving_text = content.is_some();
                        if let Some(id) = cancelled_files {
                            if published
                                .as_ref()
                                .is_some_and(|(current, _)| current == &id)
                            {
                                published = None;
                            }
                            if files.reservation.as_ref() == Some(&id)
                                || files
                                    .pending
                                    .as_ref()
                                    .is_some_and(|(pending, _)| pending == &id)
                            {
                                files.invalidate();
                            }
                            ready.push((id, false));
                        }
                        if let Some(content) = content {
                            engine.receive(content)?;
                        }
                        let file_present = if native_files {
                            let own_offer = if let Some((_, binding)) = &published {
                                binding.is_current()?
                            } else {
                                false
                            };
                            if own_offer {
                                files.revision = published
                                    .as_ref()
                                    .and_then(|(_, binding)| binding.sequence());
                                true
                            } else {
                                published = None;
                                let selection = super::clipboard_files::read()?;
                                let present = selection.is_some();
                                let revision = super::clipboard_files::sequence();
                                let changed =
                                    files.observed != selection || files.revision != revision;
                                let provider_path = selection.as_ref().is_some_and(|selection| {
                                    selection
                                        .paths
                                        .iter()
                                        .any(|path| super::clipboard_files::is_provider_path(path))
                                });
                                let operation = selection
                                    .as_ref()
                                    .map_or(super::file_transfer::Operation::Copy, |selection| {
                                        selection.operation
                                    });
                                let pending = files.pending.as_ref().map(|(id, _)| id.clone());
                                let outgoing = files.observe(selection, revision);
                                if changed && !provider_path {
                                    outgoing_files = Some(outgoing.map(|paths| {
                                        super::clipboard_files::Selection {
                                            sequence: revision.unwrap_or_default(),
                                            paths,
                                            operation,
                                        }
                                    }));
                                }
                                if files.pending.is_none()
                                    && let Some(id) = pending
                                {
                                    ready.push((id, false));
                                }
                                present
                            }
                        } else {
                            false
                        };
                        let receiving_text =
                            receiving_text && !engine.discard_file_ack(file_present);
                        let applying_text = engine.pending.is_some();
                        if receiving_text || applying_text {
                            if let Some((id, _)) = files.pending.take() {
                                ready.push((id, false));
                            }
                            files.invalidate();
                        }
                        let value = if !file_present || applying_text {
                            if native.is_none() {
                                native = Some(factory()?);
                            }
                            let value = engine
                                .guarded_step(native.as_mut().unwrap().as_mut(), file_present)?;
                            if value.is_some() {
                                files.invalidate();
                            }
                            value
                        } else {
                            None
                        };
                        if let Some((id, expected)) = begin {
                            if files.reserve(
                                id.clone(),
                                expected,
                                value.is_some()
                                    || outgoing_files.is_some()
                                    || receiving_text
                                    || applying_text,
                            ) {
                                if let Some((old, _)) = files.pending.take()
                                    && old != id
                                {
                                    ready.push((old, false));
                                }
                            } else {
                                ready.push((id, false));
                            }
                        }
                        if let Some((id, paths)) = incoming_files {
                            if files.reservation.as_ref() == Some(&id) {
                                ensure!(
                                    !paths.is_empty()
                                        && paths.len() <= 10_000
                                        && paths.iter().all(|p| p.is_absolute()),
                                    "The received file clipboard is invalid"
                                );
                                files.pending = Some((id, paths));
                            } else {
                                ready.push((id, false));
                            }
                        }
                        if let Some((id, paths)) = files.pending.as_ref()
                            && native_files
                        {
                            let accepted = files.revision.is_some_and(|expected| {
                                super::clipboard_files::sequence() == Some(expected)
                            });
                            let accepted = if accepted {
                                super::clipboard_files::write_if_current(
                                    paths,
                                    files.revision.unwrap(),
                                )?
                            } else {
                                false
                            };
                            // Suppress our next observation even if the OS
                            // is briefly busy immediately after publication.
                            if accepted {
                                files.written = Some(paths.clone());
                                files.observed = None;
                                // Capture our new revision now so a rapid
                                // following announcement reserves it. A
                                // busy clipboard is observed on the next
                                // tick without echoing these owned paths.
                                if let Ok(Some(observed)) = super::clipboard_files::read()
                                    && observed.paths == *paths
                                {
                                    files.revision = super::clipboard_files::sequence();
                                    files.observed = Some(observed);
                                    files.written = None;
                                }
                            }
                            ready.push((id.clone(), accepted));
                            files.pending = None;
                            files.reservation = None;
                            engine = Engine::default();
                        }
                        if let Some((offer, expected)) = offered {
                            offer_attempted = true;
                            let id = offer.id.clone();
                            let allowed = native_files
                                && outgoing_files.is_none()
                                && value.is_none()
                                && !receiving_text
                                && !applying_text
                                && expected.is_some()
                                && files.revision == expected;
                            let binding = if allowed {
                                match super::clipboard_files::publish_offer_if_current(
                                    offer,
                                    expected.unwrap(),
                                ) {
                                    Ok(binding) => binding,
                                    Err(error) => {
                                        // Keep the remote offer alive and retry
                                        // after a short backoff. Retiring it
                                        // here loses a transient D-Bus/Explorer
                                        // busy result permanently.
                                        publication_failed = true;
                                        return Err(error);
                                    }
                                }
                            } else {
                                None
                            };
                            let accepted = binding.is_some();
                            if let Some(binding) = binding {
                                files.revision = binding.sequence();
                                files.written = Some(binding.local_paths());
                                published = Some((id.clone(), binding));
                                engine = Engine::default();
                            }
                            ready.push((id, accepted));
                        }
                        Ok(value)
                    })();
                    let mut state = worker.state.lock().unwrap();
                    if state.enabled && state.generation == current {
                        if result.is_err()
                            && publication_failed
                            && state.files_offer.is_none()
                            && let Some((offer, expected)) = retry_offer
                        {
                            state.files_offer = Some((current, offer, expected));
                            state.files_offer_retry_at = Some(Instant::now() + PUBLISH_RETRY);
                        } else if result.is_err()
                            && !offer_attempted
                            && state.files_offer.is_none()
                            && let Some((offer, expected)) = retry_offer
                        {
                            state.files_offer = Some((current, offer, expected));
                        }
                        state.native_revision = files.revision;
                        if let Some(selection) = outgoing_files {
                            state.files_outgoing = Some((current, selection));
                            ctx.request_repaint_of(eframe::egui::ViewportId::ROOT);
                        }
                        for ready in ready {
                            if state.files_ready.len() >= 64 {
                                state.files_ready.pop_front();
                            }
                            state.files_ready.push_back(ready);
                            ctx.request_repaint_of(eframe::egui::ViewportId::ROOT);
                        }
                        let status = match result {
                            Ok(content) => {
                                if let Some(content) = content {
                                    state.outgoing = Some((current, content));
                                    ctx.request_repaint_of(eframe::egui::ViewportId::ROOT);
                                }
                                None
                            }
                            Err(error) => Some(format!("Shared clipboard: {error:#}")),
                        };
                        if state.status != status {
                            state.status = status;
                            ctx.request_repaint_of(eframe::egui::ViewportId::ROOT);
                        }
                    }
                    if state.stop {
                        break;
                    }
                    let offer_retry_pending = state.files_offer.is_some()
                        && state
                            .files_offer_retry_at
                            .is_some_and(|at| Instant::now() < at);
                    if state.incoming.is_none()
                        && state.files_begin.is_none()
                        && state.files_incoming.is_none()
                        && state.files_cancel.is_none()
                        && state.files_offer.is_none()
                        && state.enabled
                        && state.generation == current
                        && !offer_retry_pending
                    {
                        let _ = worker.wake.wait_timeout(state, INTERVAL).unwrap();
                    } else if offer_retry_pending {
                        let wait = state
                            .files_offer_retry_at
                            .map(|at| at.saturating_duration_since(Instant::now()))
                            .unwrap_or(INTERVAL)
                            .min(INTERVAL);
                        let _ = worker.wake.wait_timeout(state, wait).unwrap();
                    }
                }
            })?;
        Ok(Self { shared })
    }
    pub fn enabled(&self, value: bool) {
        let mut state = self.shared.state.lock().unwrap();
        if state.enabled != value {
            state.enabled = value;
            state.generation = state.generation.wrapping_add(1);
            state.incoming = None;
            state.outgoing = None;
            state.status = None;
            state.files_begin = None;
            state.files_incoming = None;
            state.files_outgoing = None;
            state.files_offer = None;
            state.files_offer_retry_at = None;
            state.files_ready.clear();
            state.native_revision = None;
            state.files_cancel = None;
            self.shared.wake.notify_one();
        }
    }
    pub fn receive(&self, mime: String, bytes: Vec<u8>, digest: String) {
        if bytes.len() > LIMIT || !["text/plain", "image/png"].contains(&mime.as_str()) {
            return;
        }
        let mut state = self.shared.state.lock().unwrap();
        if state.enabled {
            state.incoming = Some((state.generation, (mime, bytes, digest)));
            self.shared.wake.notify_one();
        }
    }
    pub fn poll(&self) -> Vec<Content> {
        let mut state = self.shared.state.lock().unwrap();
        let generation = state.generation;
        state
            .outgoing
            .take()
            .filter(|(epoch, _)| state.enabled && *epoch == generation)
            .map(|(_, content)| vec![content])
            .unwrap_or_default()
    }
    pub(crate) fn poll_files(&self) -> Vec<Option<super::clipboard_files::Selection>> {
        let mut state = self.shared.state.lock().unwrap();
        let generation = state.generation;
        state
            .files_outgoing
            .take()
            .filter(|(epoch, _)| state.enabled && *epoch == generation)
            .map(|(_, paths)| vec![paths])
            .unwrap_or_default()
    }
    pub(crate) fn receive_offer(&self, offer: super::clipboard_files::provider::RemoteOffer) {
        let mut state = self.shared.state.lock().unwrap();
        if state.enabled {
            state.files_offer = Some((state.generation, offer, state.native_revision));
            state.files_offer_retry_at = None;
            self.shared.wake.notify_one();
        }
    }
    pub(crate) fn begin_files(&self, id: String) {
        let mut state = self.shared.state.lock().unwrap();
        if state.enabled {
            state.files_begin = Some((state.generation, id, state.native_revision));
            self.shared.wake.notify_one();
        }
    }
    pub(crate) fn receive_files(&self, id: String, paths: Vec<PathBuf>) {
        let mut state = self.shared.state.lock().unwrap();
        if state.enabled {
            state.files_incoming = Some((state.generation, id, paths));
            self.shared.wake.notify_one();
        }
    }
    pub(crate) fn files_ready(&self) -> Vec<(String, bool)> {
        self.shared
            .state
            .lock()
            .unwrap()
            .files_ready
            .drain(..)
            .collect()
    }
    pub(crate) fn cancel_files(&self, id: String) {
        let mut state = self.shared.state.lock().unwrap();
        if state
            .files_begin
            .as_ref()
            .is_some_and(|(_, pending, _)| pending == &id)
        {
            state.files_begin = None;
        }
        if state
            .files_incoming
            .as_ref()
            .is_some_and(|(_, pending, _)| pending == &id)
        {
            state.files_incoming = None;
        }
        state.files_cancel = Some((state.generation, id));
        self.shared.wake.notify_one();
    }
    pub fn status(&self) -> Option<String> {
        self.shared
            .state
            .lock()
            .unwrap()
            .status
            .as_ref()
            .map(|message| {
                if let Some(detail) = message.strip_prefix("Shared clipboard: ") {
                    format!(
                        "{}: {}",
                        crate::i18n::tr("Shared clipboard"),
                        detail
                            .split(": ")
                            .map(crate::i18n::tr)
                            .collect::<Vec<_>>()
                            .join(": ")
                    )
                } else {
                    crate::i18n::tr(message)
                }
            })
    }
}
impl Drop for Clipboard {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock().unwrap();
        state.stop = true;
        state.incoming = None;
        state.outgoing = None;
        self.shared.wake.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[test]
    fn native_file_formats_block_ancillary_text_ack_reads_and_busy_text_writes_retry() {
        struct Observed {
            inner: Fake,
            reads: usize,
        }
        impl Backend for Observed {
            fn read(&mut self) -> Result<Option<Value>> {
                self.reads += 1;
                self.inner.read()
            }
            fn write(&mut self, value: &Value) -> Result<()> {
                self.inner.write(value)
            }
        }
        let mut backend = Observed {
            inner: Fake {
                value: Some(Value::Text("Earlier text".into())),
                ..Default::default()
            },
            reads: 0,
        };
        let mut engine = Engine::default();
        let old = engine.step(&mut backend).unwrap().unwrap();
        backend.reads = 0;
        backend.inner.value = Some(Value::Text("file:///private/ancillary-file-path".into()));
        engine.receive(old).unwrap();
        assert!(engine.guarded_step(&mut backend, true).unwrap().is_none());
        assert_eq!(
            backend.reads, 0,
            "File metadata must never fall through to the plain-text reader"
        );
        assert_eq!(backend.inner.writes, 0);
        engine
            .receive(
                Value::Text("A new remote text copy".into())
                    .encode()
                    .unwrap(),
            )
            .unwrap();
        backend.inner.fail = true;
        assert!(engine.guarded_step(&mut backend, true).is_err());
        assert!(engine.pending.is_some());
        assert!(engine.guarded_step(&mut backend, true).unwrap().is_none());
        assert_eq!(backend.inner.writes, 1);
        assert_eq!(
            backend.inner.value,
            Some(Value::Text("A new remote text copy".into()))
        );
    }
    #[test]
    fn newer_native_revision_cancels_a_file_reservation_even_for_identical_text() {
        let mut engine = FileEngine::default();
        assert!(engine.observe(None, Some(7)).is_none());
        assert!(engine.reserve("download".into(), Some(7), false));
        // No file MIME and no changed text fingerprint are needed: a native
        // clipboard ownership change alone makes the old reservation stale.
        assert!(engine.observe(None, Some(8)).is_none());
        assert!(engine.reservation.is_none());
        assert!(!engine.reserve("late".into(), Some(7), false));
        assert!(!engine.reserve("same-tick-local".into(), Some(8), true));
        assert!(engine.reserve("next".into(), Some(8), false));
    }
    #[test]
    fn owned_file_publication_does_not_echo_but_a_later_real_copy_does() {
        let paths = vec![std::env::temp_dir().join("synthetic-file-drop")];
        let mut engine = FileEngine {
            written: Some(paths.clone()),
            ..Default::default()
        };
        assert!(
            engine
                .observe(
                    Some(super::super::clipboard_files::Selection {
                        sequence: 10,
                        operation: super::super::file_transfer::Operation::Copy,
                        paths: paths.clone()
                    }),
                    Some(10)
                )
                .is_none()
        );
        assert!(engine.reserve("next-download".into(), Some(10), false));
        assert_eq!(
            engine.observe(
                Some(super::super::clipboard_files::Selection {
                    sequence: 11,
                    operation: super::super::file_transfer::Operation::Copy,
                    paths: paths.clone()
                }),
                Some(11)
            ),
            Some(paths)
        );
        assert!(engine.reservation.is_none());
        assert!(
            engine
                .observe(
                    Some(super::super::clipboard_files::Selection {
                        sequence: 12,
                        operation: super::super::file_transfer::Operation::Copy,
                        paths: vec![]
                    }),
                    Some(12)
                )
                .is_none()
        );
    }
    #[test]
    fn cancelling_a_queued_publication_discards_its_begin_and_ready_together() {
        let clipboard = Clipboard::spawn(eframe::egui::Context::default(), || {
            Ok(Box::new(Fake::default()))
        })
        .unwrap();
        // Keep the worker disabled so this is solely a queue-ordering fixture.
        {
            let mut state = clipboard.shared.state.lock().unwrap();
            state.files_begin = Some((0, "pending".into(), Some(2)));
            state.files_incoming = Some((
                0,
                "pending".into(),
                vec![std::env::temp_dir().join("synthetic")],
            ));
        }
        clipboard.cancel_files("pending".into());
        let state = clipboard.shared.state.lock().unwrap();
        assert!(state.files_begin.is_none() && state.files_incoming.is_none());
    }
    #[derive(Default)]
    struct Fake {
        value: Option<Value>,
        writes: usize,
        fail: bool,
    }
    impl Backend for Fake {
        fn read(&mut self) -> Result<Option<Value>> {
            Ok(self.value.clone())
        }
        fn write(&mut self, value: &Value) -> Result<()> {
            if std::mem::take(&mut self.fail) {
                anyhow::bail!("clipboard is temporarily occupied");
            }
            self.writes += 1;
            self.value = Some(value.clone());
            Ok(())
        }
    }
    #[test]
    fn remote_image_reencoding_does_not_echo_and_a_later_local_copy_is_shared() {
        let image = Value::Image {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 128],
        };
        let mut bytes = Vec::new();
        use image::ImageEncoder;
        if let Value::Image {
            width,
            height,
            rgba,
        } = &image
        {
            image::codecs::png::PngEncoder::new_with_quality(
                &mut bytes,
                image::codecs::png::CompressionType::Best,
                image::codecs::png::FilterType::NoFilter,
            )
            .write_image(
                rgba,
                *width as u32,
                *height as u32,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        }
        assert_ne!(bytes, image.encode().unwrap().1);
        let mut engine = Engine::default();
        let mut backend = Fake::default();
        let digest = hash(&bytes);
        engine.receive(("image/png".into(), bytes, digest)).unwrap();
        assert!(engine.step(&mut backend).unwrap().is_none());
        assert!(engine.step(&mut backend).unwrap().is_none());
        assert_eq!(backend.writes, 1);
        backend.value = Some(Value::Text("New copy — привет\n".into()));
        let content = engine.step(&mut backend).unwrap().unwrap();
        assert_eq!(decode(content).unwrap(), backend.value.unwrap());
    }
    #[test]
    fn a_busy_clipboard_retries_the_latest_received_value() {
        let mut engine = Engine::default();
        let mut backend = Fake {
            fail: true,
            ..Default::default()
        };
        engine
            .receive(Value::Text("first".into()).encode().unwrap())
            .unwrap();
        assert!(engine.step(&mut backend).is_err());
        engine
            .receive(Value::Text("latest".into()).encode().unwrap())
            .unwrap();
        assert!(engine.step(&mut backend).unwrap().is_none());
        assert_eq!(backend.value, Some(Value::Text("latest".into())));
        assert_eq!(backend.writes, 1);
    }
    #[test]
    fn the_controllers_own_copy_acknowledgement_preserves_native_clipboard_formats() {
        let mut engine = Engine::default();
        let mut backend = Fake {
            value: Some(Value::Text("a local rich-text copy".into())),
            ..Default::default()
        };
        let content = engine.step(&mut backend).unwrap().unwrap();
        engine.receive(content).unwrap();
        assert!(engine.step(&mut backend).unwrap().is_none());
        assert_eq!(backend.writes, 0);
    }
    #[test]
    fn malformed_payloads_are_rejected_without_any_native_write() {
        assert!(decode(("text/plain".into(), vec![0xff], hash(&[0xff]))).is_err());
        assert!(decode(("text/plain".into(), b"secret".to_vec(), "wrong".into())).is_err());
        assert!(
            decode((
                "text/uri-list".into(),
                b"file:///private".to_vec(),
                hash(b"file:///private")
            ))
            .is_err()
        );
        assert!(decode(("image/png".into(), b"bad".to_vec(), hash(b"bad"))).is_err());
        assert!(validate_image(4097, 1, 4097 * 4).is_err());
        assert!(validate_image(1, 1, 3).is_err());
    }
    #[test]
    fn disabled_worker_never_opens_native_clipboard_and_discards_queued_content() {
        let opens = Arc::new(AtomicUsize::new(0));
        let counter = opens.clone();
        let clipboard = Clipboard::spawn(eframe::egui::Context::default(), move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(Fake::default()))
        })
        .unwrap();
        let content = Value::Text("must stay local".into()).encode().unwrap();
        clipboard.receive(content.0, content.1, content.2);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(opens.load(Ordering::SeqCst), 0);
        assert!(clipboard.poll().is_empty());
        clipboard.enabled(true);
        clipboard.enabled(false);
        assert!(clipboard.poll().is_empty());
        assert!(clipboard.status().is_none());
    }

    #[test]
    fn worker_wakes_hidden_root_and_discards_copies_when_disabled() {
        struct SharedFake(Arc<Mutex<Fake>>);
        impl Backend for SharedFake {
            fn read(&mut self) -> Result<Option<Value>> {
                self.0.lock().unwrap().read()
            }
            fn write(&mut self, value: &Value) -> Result<()> {
                self.0.lock().unwrap().write(value)
            }
        }
        fn wait(mut condition: impl FnMut() -> bool) {
            let since = std::time::Instant::now();
            while !condition() {
                assert!(
                    since.elapsed() < Duration::from_secs(2),
                    "clipboard worker did not respond"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let ctx = eframe::egui::Context::default();
        for _ in 0..8 {
            let _ = ctx.run_ui(Default::default(), |_| {});
        }
        let wakes = Arc::new(AtomicUsize::new(0));
        let reported = wakes.clone();
        ctx.set_request_repaint_callback(move |request| {
            assert_eq!(request.viewport_id, eframe::egui::ViewportId::ROOT);
            reported.fetch_add(1, Ordering::SeqCst);
        });
        let fake = Arc::new(Mutex::new(Fake {
            value: Some(Value::Text("local".into())),
            ..Default::default()
        }));
        let backend = fake.clone();
        let clipboard =
            Clipboard::spawn(ctx, move || Ok(Box::new(SharedFake(backend.clone())))).unwrap();
        clipboard.enabled(true);
        wait(|| clipboard.shared.state.lock().unwrap().outgoing.is_some());
        assert_eq!(
            decode(clipboard.poll().pop().unwrap()).unwrap(),
            Value::Text("local".into())
        );
        assert!(wakes.load(Ordering::SeqCst) > 0);
        let (mime, bytes, digest) = Value::Text("remote".into()).encode().unwrap();
        clipboard.receive(mime, bytes, digest);
        wait(|| fake.lock().unwrap().writes == 1);
        assert!(
            clipboard.poll().is_empty(),
            "remote writes must not be echoed"
        );
        fake.lock().unwrap().value = Some(Value::Text("queued before disable".into()));
        wait(|| clipboard.shared.state.lock().unwrap().outgoing.is_some());
        clipboard.enabled(false);
        assert!(
            clipboard.poll().is_empty(),
            "disabling must discard queued outgoing data"
        );
        let (mime, bytes, digest) = Value::Text("must be ignored".into()).encode().unwrap();
        clipboard.receive(mime, bytes, digest);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(fake.lock().unwrap().writes, 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn gnome_bridge_uses_a_private_bus_and_real_binary_payloads() {
        const MARKER: &str = "DESK_SWITCH_CLIPBOARD_PRIVATE_BUS";
        if std::env::var(MARKER).as_deref() != Ok("1") {
            let output = std::process::Command::new("dbus-run-session")
                .arg("--")
                .arg(std::env::current_exe().unwrap())
                .args(["--exact", "group::clipboard::tests::gnome_bridge_uses_a_private_bus_and_real_binary_payloads", "--nocapture"])
                .env(MARKER, "1")
                .env("DESK_SWITCH_CLIPBOARD_PARENT_BUS", std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default())
                .output()
                .expect("dbus-run-session is required for the private clipboard protocol test");
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap();
        assert!(!address.is_empty());
        assert_ne!(
            Some(address),
            std::env::var("DESK_SWITCH_CLIPBOARD_PARENT_BUS").ok()
        );
        #[derive(Clone)]
        struct Mock {
            state: Arc<Mutex<(u32, String, Vec<u8>)>>,
        }
        #[zbus::interface(name = "io.github.siarheisinelnikau.DeskSwitch.Clipboard")]
        impl Mock {
            fn version(&self) -> u32 {
                1
            }
            fn read(&self, known: u32) -> (u32, String, Vec<u8>) {
                let state = self.state.lock().unwrap();
                if state.0 == known {
                    (state.0, String::new(), Vec::new())
                } else {
                    state.clone()
                }
            }
            fn write(&self, mime: String, bytes: Vec<u8>) -> u32 {
                let mut state = self.state.lock().unwrap();
                state.0 += 1;
                state.1 = mime;
                state.2 = bytes;
                state.0
            }
        }
        let state = Arc::new(Mutex::new((
            1,
            "text/plain".to_owned(),
            "Fixture — привет\n".as_bytes().to_vec(),
        )));
        let _server = zbus::blocking::connection::Builder::session()
            .unwrap()
            .name("io.github.siarheisinelnikau.DeskSwitch.Clipboard")
            .unwrap()
            .serve_at(
                "/io/github/siarheisinelnikau/DeskSwitch/Clipboard",
                Mock {
                    state: state.clone(),
                },
            )
            .unwrap()
            .build()
            .unwrap();
        let mut bridge = gnome::Gnome::new().unwrap();
        assert_eq!(
            bridge.read().unwrap(),
            Some(Value::Text("Fixture — привет\n".into()))
        );
        assert!(bridge.read().unwrap().is_none());
        let image = Value::Image {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 128],
        };
        bridge.write(&image).unwrap();
        assert!(bridge.read().unwrap().is_none());
        let stored = state.lock().unwrap().clone();
        let digest = hash(&stored.2);
        assert_eq!(decode((stored.1, stored.2, digest)).unwrap(), image);
        let text = Value::Text("Another synthetic copy".into())
            .encode()
            .unwrap();
        *state.lock().unwrap() = (3, text.0, text.1);
        assert_eq!(
            bridge.read().unwrap(),
            Some(Value::Text("Another synthetic copy".into()))
        );
    }
}
