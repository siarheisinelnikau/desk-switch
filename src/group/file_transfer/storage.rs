//! Confined, durable storage for explicitly exported files and folders.
//!
//! Network requests name a random export and a numeric manifest entry, never a
//! local path. Exports are immutable owned snapshots. Receiver checkpoints are
//! committed only after the corresponding bytes have been flushed, and are
//! checked against the on-disk prefix before resuming.
use super::protocol::{
    self, CHUNK, Description, Descriptor, DescriptorKind, Entry, Kind, MAX_ENTRIES, MAX_MANIFEST,
    MAX_TOTAL, Manifest,
};
use anyhow::{Context, Result, bail, ensure};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, DirBuilder, File, Metadata, OpenOptions},
    time::SystemTime,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

pub mod source_move;

const FORMAT: u16 = 1;
const BUFFER: usize = 64 * 1024;
const CHECKPOINT_LIMIT: usize = 4096;
const CANCELLATION_LIMIT: usize = 1024;
const MAX_CANCELLATIONS: usize = 65_536;
static TEMP: AtomicU64 = AtomicU64::new(1);
static NO_CANCEL: AtomicBool = AtomicBool::new(false);
static CANCELLATIONS: Mutex<()> = Mutex::new(());

// Observe source I/O boundaries on the current test thread. Capture must never
// request a content-capable handle, and cancellation must interrupt verification
// between buffers even when a cloud/FUSE open itself could hydrate file bytes.
#[cfg(test)]
thread_local! {
    static CONTENT_OPENS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static HASH_BUFFERS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static AFTER_HASH_BUFFER: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Cancellation {
    format: u16,
    workspace: String,
    peer: String,
    id: String,
}

fn cancellation(workspace: &str, peer: &str, id: &str) -> Result<(Cancellation, String)> {
    protocol::valid_id(id)?;
    crate::group::model::valid_key(peer)?;
    ensure!(
        !workspace.is_empty() && workspace.len() <= 256 && !workspace.chars().any(char::is_control),
        "Invalid file cancellation workspace"
    );
    let mut hash = Sha256::new();
    hash.update(b"Desk Switch file cancellation\0");
    for value in [workspace, peer, id] {
        hash.update((value.len() as u64).to_le_bytes());
        hash.update(value.as_bytes());
    }
    Ok((
        Cancellation {
            format: FORMAT,
            workspace: workspace.into(),
            peer: peer.into(),
            id: id.into(),
        },
        format!("{:x}.json", hash.finalize()),
    ))
}

fn cancellation_directory(state_dir: &Path, create: bool) -> Result<Option<Dir>> {
    let mut current = match absolute_directory(state_dir, create) {
        Ok(directory) => directory,
        Err(error) if !create && missing(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    for name in ["file-transfer", "cancellations"] {
        current = match directory(&current, OsStr::new(name), create) {
            Ok(directory) => directory,
            Err(error) if !create && missing(&error) => return Ok(None),
            Err(error) => return Err(error),
        };
        private_directory(&current)?;
    }
    Ok(Some(current))
}

/// Persist cancellation of an authenticated export, including one whose offer
/// has not arrived yet. This grants no file read or receiving-stage authority.
pub fn remember_cancel(state_dir: &Path, workspace: &str, peer: &str, id: &str) -> Result<()> {
    remember_cancel_with_limit(state_dir, workspace, peer, id, MAX_CANCELLATIONS)
}

fn remember_cancel_with_limit(
    state_dir: &Path,
    workspace: &str,
    peer: &str,
    id: &str,
    max: usize,
) -> Result<()> {
    ensure!(max > 0, "Invalid file cancellation history limit");
    let (binding, name) = cancellation(workspace, peer, id)?;
    let _guard = CANCELLATIONS
        .lock()
        .map_err(|_| anyhow::anyhow!("File cancellation storage is unavailable"))?;
    let directory = cancellation_directory(state_dir, true)?
        .context("File cancellation storage is unavailable")?;
    if exists(&directory, &name)? {
        ensure!(
            read_json::<Cancellation>(&directory, &name, CANCELLATION_LIMIT)? == binding,
            "The saved cancellation belongs to a different export or computer"
        );
        return Ok(());
    }
    // Never evict a tombstone: an old in-flight offer must not revive merely
    // because its visible transfer history was pruned. Capacity failure remains
    // explicit and preserves every earlier cancellation.
    let mut count = 0;
    for entry in directory.entries()? {
        let _ = entry?;
        count += 1;
        ensure!(
            count < max,
            "The file cancellation history is full; no earlier cancellations were removed"
        );
    }
    let bytes = serde_json::to_vec(&binding)?;
    ensure!(
        bytes.len() <= CANCELLATION_LIMIT,
        "File cancellation metadata is too large"
    );
    write_new(&directory, &name, &bytes)
}

pub fn was_cancelled(state_dir: &Path, workspace: &str, peer: &str, id: &str) -> Result<bool> {
    let (binding, name) = cancellation(workspace, peer, id)?;
    let _guard = CANCELLATIONS
        .lock()
        .map_err(|_| anyhow::anyhow!("File cancellation storage is unavailable"))?;
    let Some(directory) = cancellation_directory(state_dir, false)? else {
        return Ok(false);
    };
    if !exists(&directory, &name)? {
        return Ok(false);
    }
    ensure!(
        read_json::<Cancellation>(&directory, &name, CANCELLATION_LIMIT)? == binding,
        "The saved cancellation belongs to a different export or computer"
    );
    Ok(true)
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn cancelled(cancel: &AtomicBool) -> Result<()> {
    ensure!(
        !cancel.load(Ordering::Acquire),
        "The file transfer was cancelled"
    );
    Ok(())
}

fn no_links(metadata: &Metadata) -> Result<()> {
    ensure!(
        !metadata.file_type().is_symlink(),
        "Symbolic links cannot be transferred"
    );
    #[cfg(windows)]
    {
        use cap_std::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "Reparse points cannot be transferred"
        );
    }
    Ok(())
}

fn regular(file: &File, private: bool) -> Result<Metadata> {
    let metadata = file.metadata()?;
    no_links(&metadata)?;
    ensure!(
        metadata.is_file(),
        "Only regular files and folders can be transferred"
    );
    if private {
        #[cfg(unix)]
        {
            use cap_std::fs::MetadataExt;
            ensure!(
                metadata.nlink() == 1,
                "Transfer storage contains an unexpected hard link"
            );
        }
        #[cfg(windows)]
        {
            use cap_fs_ext::MetadataExt;
            ensure!(
                metadata.nlink() == 1,
                "Transfer storage contains an unexpected hard link"
            );
        }
    }
    Ok(metadata)
}

fn private_directory(dir: &Dir) -> Result<()> {
    let metadata = dir.dir_metadata()?;
    no_links(&metadata)?;
    ensure!(metadata.is_dir(), "Transfer storage is not a folder");
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt;
        ensure!(
            metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o077 == 0,
            "Transfer storage must be private to the current user"
        );
    }
    Ok(())
}

fn directory(parent: &Dir, name: &OsStr, create: bool) -> Result<Dir> {
    if create {
        match parent.create_dir_with(name, &private_builder()) {
            Ok(()) => sync_dir(parent)?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    let child = parent.open_dir_nofollow(name)?;
    no_links(&child.dir_metadata()?)?;
    Ok(child)
}

/// Open every component independently with no-follow semantics. In particular,
/// a symlink in an ancestor cannot redirect a later source read or stage write.
fn absolute_directory(path: &Path, create: bool) -> Result<Dir> {
    absolute_directory_with(path, create, false)
}

fn source_absolute_directory(path: &Path) -> Result<Dir> {
    absolute_directory_with(path, false, true)
}

/// Open the source root without blocking a later rename of any descendant.
#[cfg(windows)]
fn source_root_directory(path: &Path) -> Result<Dir> {
    use std::os::windows::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(0x0200_0000) // FILE_FLAG_BACKUP_SEMANTICS
        .share_mode(0x1 | 0x2 | 0x4) // FILE_SHARE_READ | WRITE | DELETE
        .open(path)?;
    Ok(Dir::from_std_file(file))
}

#[cfg(not(windows))]
fn source_root_directory(path: &Path) -> Result<Dir> {
    Ok(Dir::open_ambient_dir(path, ambient_authority())?)
}

/// Capture capabilities allow normal local renames/deletes while a clipboard
/// offer waits for paste. Only metadata operations use these shared handles.
fn source_directory(parent: &Dir, name: &OsStr) -> Result<Dir> {
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt;
        let mut options = OpenOptions::new();
        options
            // Do not request FILE_READ_DATA: if a racing substitution changes
            // this name from a directory into a cloud file, even the rejected
            // open must remain metadata-only and must not hydrate its bytes.
            .access_mode(0)
            .follow(FollowSymlinks::No)
            // FILE_FLAG_BACKUP_SEMANTICS permits a directory handle.
            .custom_flags(0x0200_0000)
            // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE.
            .share_mode(0x1 | 0x2 | 0x4);
        let file = parent.open_with(name, &options)?;
        let metadata = file.metadata()?;
        no_links(&metadata)?;
        ensure!(metadata.is_dir(), "The copied source is not a folder");
        Ok(Dir::from_std_file(file.into_std()))
    }
    #[cfg(not(windows))]
    {
        let directory = parent.open_dir_nofollow(name)?;
        no_links(&directory.dir_metadata()?)?;
        Ok(directory)
    }
}

fn absolute_directory_with(path: &Path, create: bool, source: bool) -> Result<Dir> {
    ensure!(
        path.is_absolute(),
        "Choose an absolute local file or folder path"
    );
    let mut root = PathBuf::new();
    let mut names = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                #[cfg(windows)]
                ensure!(
                    matches!(
                        prefix.kind(),
                        std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)
                    ),
                    "Network and device paths cannot be transferred"
                );
                root.push(prefix.as_os_str());
            }
            Component::RootDir => root.push(component.as_os_str()),
            Component::Normal(name) => names.push(name.to_os_string()),
            _ => bail!("Relative path components cannot be transferred"),
        }
    }
    ensure!(
        !root.as_os_str().is_empty(),
        "Choose an absolute local file or folder path"
    );
    let mut current = if source {
        source_root_directory(&root)?
    } else {
        Dir::open_ambient_dir(root, ambient_authority())?
    };
    for name in names {
        current = if source {
            source_directory(&current, &name)?
        } else {
            directory(&current, &name, create)?
        };
    }
    Ok(current)
}

fn open_file(parent: &Dir, name: impl AsRef<Path>, write: bool, create: bool) -> Result<File> {
    #[cfg(test)]
    CONTENT_OPENS.with(|count| count.set(count.get() + 1));
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(write)
        .create_new(create)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
        // A malicious source substitution with a FIFO must fail instead of
        // blocking the worker before its opened-handle type check.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = parent.open_with(name, &options)?;
    regular(&file, write || create)?;
    Ok(file)
}

fn sync_dir(dir: &Dir) -> Result<()> {
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        // Linux capability directories use O_PATH, which cannot be fsynced.
        // Reopen the same pinned directory read-only, relative to its handle;
        // setting O_DIRECTORY as a custom flag avoids cap-std's O_PATH-only
        // dir_required mode without introducing an ambient path or following
        // a symlink. Keep the real fsync failure observable to the caller.
        let mut options = OpenOptions::new();
        options
            .read(true)
            .follow(FollowSymlinks::No)
            .custom_flags(libc::O_DIRECTORY);
        dir.open_with(".", &options)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

fn write_new(parent: &Dir, name: &str, bytes: &[u8]) -> Result<()> {
    let mut file = open_file(parent, name, true, true)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    sync_dir(parent)
}

fn write_json(parent: &Dir, name: &str, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(
        bytes.len() <= MAX_MANIFEST,
        "Transfer metadata is too large"
    );
    let mut last = None;
    for _ in 0..128 {
        let temporary = format!(
            ".journal-{:x}-{:x}",
            std::process::id(),
            TEMP.fetch_add(1, Ordering::Relaxed)
        );
        match open_file(parent, &temporary, true, true) {
            Ok(mut file) => {
                let result = (|| {
                    file.write_all(&bytes)?;
                    file.sync_all()?;
                    drop(file);
                    // This replacement is restricted to this application's
                    // fixed checkpoint name inside its private directory.
                    parent.rename(&temporary, parent, name)?;
                    sync_dir(parent)
                })();
                if result.is_err() {
                    let _ = parent.remove_file(&temporary);
                }
                return result;
            }
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::AlreadyExists) =>
            {
                last = Some(error)
            }
            Err(error) => return Err(error),
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("Could not create transfer metadata")))
}

fn read_json<T: DeserializeOwned>(parent: &Dir, name: &str, limit: usize) -> Result<T> {
    let file = open_file(parent, name, false, false)?;
    ensure!(
        regular(&file, true)?.len() <= limit as u64,
        "Transfer metadata is too large"
    );
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= limit, "Transfer metadata is too large");
    serde_json::from_slice(&bytes).context("Transfer metadata is damaged")
}

fn exists(parent: &Dir, name: &str) -> Result<bool> {
    match parent.symlink_metadata(name) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn missing(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

fn hash_prefix(file: &mut File, length: u64, cancel: &AtomicBool) -> Result<Sha256> {
    file.seek(SeekFrom::Start(0))?;
    let mut left = length;
    let mut buffer = [0; BUFFER];
    let mut hash = Sha256::new();
    while left != 0 {
        cancelled(cancel)?;
        let count = usize::try_from(left.min(BUFFER as u64))?;
        file.read_exact(&mut buffer[..count])?;
        hash.update(&buffer[..count]);
        left -= count as u64;
        #[cfg(test)]
        {
            HASH_BUFFERS.with(|count| count.set(count.get() + 1));
            AFTER_HASH_BUFFER.with(|hook| {
                if let Some(hook) = hook.borrow_mut().take() {
                    hook();
                }
            });
        }
    }
    Ok(hash)
}

fn finished_hash(hash: &Sha256) -> String {
    format!("{:x}", hash.clone().finalize())
}

fn file_name(index: u32) -> String {
    format!("f{index:08x}")
}
fn checkpoint_name(index: u32) -> String {
    format!("p{index:08x}.json")
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparationOwner {
    format: u16,
    id: String,
    purpose: String,
}

#[derive(Serialize, Deserialize)]
struct ExportOwner {
    format: u16,
    id: String,
    digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

impl FileIdentity {
    fn of(metadata: &Metadata) -> Result<Self> {
        use cap_fs_ext::MetadataExt;
        no_links(metadata)?;
        // cap-std's Windows no-follow stat uses access_mode(0), which supplies
        // handle identities without requesting FILE_READ_DATA. The extension
        // returns those handle-derived IDs on Windows and dev/inode on Unix.
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceMetadata {
    identity: FileIdentity,
    directory: bool,
    length: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    #[cfg(unix)]
    changed: (i64, i64),
}

impl SourceMetadata {
    fn of(metadata: &Metadata) -> Result<Self> {
        no_links(metadata)?;
        ensure!(
            metadata.is_dir() || metadata.is_file(),
            "Only regular files and folders can be transferred"
        );
        Ok(Self {
            identity: FileIdentity::of(metadata)?,
            directory: metadata.is_dir(),
            length: metadata.len(),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
            #[cfg(unix)]
            changed: {
                use cap_std::fs::MetadataExt;
                (metadata.ctime(), metadata.ctime_nsec())
            },
        })
    }

    fn check(&self, metadata: &Metadata) -> Result<()> {
        ensure!(
            self == &Self::of(metadata)?,
            "A copied file or folder changed; copy the selection again"
        );
        Ok(())
    }
}

struct SourceEntry {
    name: OsString,
    path: Vec<String>,
    metadata: SourceMetadata,
    children: Vec<SourceEntry>,
}

struct SourceRoot {
    parent: Dir,
    parent_path: PathBuf,
    parent_identity: FileIdentity,
    entry: SourceEntry,
}

impl SourceRoot {
    fn check_binding(&self) -> Result<()> {
        let current_parent = source_absolute_directory(&self.parent_path)?;
        ensure!(
            FileIdentity::of(&current_parent.dir_metadata()?)? == self.parent_identity
                && FileIdentity::of(&self.parent.dir_metadata()?)? == self.parent_identity,
            "The copied source folder moved or was replaced; copy the selection again"
        );
        self.entry
            .metadata
            .check(&self.parent.symlink_metadata(&self.entry.name)?)?;
        if self.entry.metadata.directory {
            // Reopen for this check instead of retaining a child directory
            // handle. On Windows a live child handle can block renaming the
            // selected folder's parent while a clipboard offer waits.
            let directory = source_directory(&self.parent, &self.entry.name)?;
            self.entry.metadata.check(&directory.dir_metadata()?)?;
        }
        Ok(())
    }

    fn pin_for_read(&self) -> Result<(Dir, Option<Dir>)> {
        self.check_binding()?;
        // cap-std intentionally prevents Windows directory replacement while
        // its confined operations resolve relative names. Upgrade capture's
        // share-delete metadata pins for the bounded, demanded snapshot only.
        let parent = absolute_directory(&self.parent_path, false)?;
        ensure!(
            FileIdentity::of(&parent.dir_metadata()?)? == self.parent_identity,
            "The copied source folder moved or was replaced; copy the selection again"
        );
        self.entry
            .metadata
            .check(&parent.symlink_metadata(&self.entry.name)?)?;
        let directory = if self.entry.metadata.directory {
            let directory = parent.open_dir_nofollow(&self.entry.name)?;
            self.entry.metadata.check(&directory.dir_metadata()?)?;
            Some(directory)
        } else {
            None
        };
        Ok((parent, directory))
    }
}

/// An in-memory selection containing display metadata and directory capabilities
/// only. Capturing it never opens source files for content, hashes them, creates
/// transfer storage, or retains an ambient path as authority to read later.
pub struct SourceSelection {
    pub description: Description,
    captured_description: Description,
    roots: Vec<SourceRoot>,
}

impl SourceSelection {
    pub fn capture(paths: &[PathBuf], cancel: &AtomicBool) -> Result<Self> {
        cancelled(cancel)?;
        ensure!(
            !paths.is_empty() && paths.len() <= MAX_ENTRIES,
            "Choose files or folders to send"
        );
        let mut description = Description {
            name: String::new(),
            roots: Vec::new(),
            entries: Vec::new(),
            total: 0,
        };
        let mut roots = Vec::new();
        let mut used = BTreeSet::new();
        // Reserve the envelope and display name, and account for the later
        // manifest's checksums now so paste cannot exceed the metadata limit.
        let mut metadata_bytes = 1024_usize;
        for source in paths {
            cancelled(cancel)?;
            let name = source
                .file_name()
                .and_then(OsStr::to_str)
                .context("File names must be valid Unicode")?;
            protocol::valid_component(name)?;
            let mut candidate = name.to_owned();
            let mut unique = false;
            for suffix in 2..=MAX_ENTRIES + 1 {
                if used.insert(candidate.to_lowercase()) {
                    unique = true;
                    break;
                }
                candidate = format!("{name} ({suffix})");
                protocol::valid_component(&candidate)?;
            }
            ensure!(unique, "File names collide");
            metadata_bytes += serde_json::to_vec(&candidate)?.len() + 1;
            ensure!(
                metadata_bytes <= MAX_MANIFEST,
                "Transfer metadata is too large"
            );
            let parent_path = source
                .parent()
                .context("Choose a file or folder")?
                .to_owned();
            let parent = source_absolute_directory(&parent_path)?;
            let parent_identity = FileIdentity::of(&parent.dir_metadata()?)?;
            let metadata = SourceMetadata::of(&parent.symlink_metadata(name)?)?;
            let directory = if metadata.directory {
                let directory = source_directory(&parent, OsStr::new(name))?;
                metadata.check(&directory.dir_metadata()?)?;
                Some(directory)
            } else {
                None
            };
            let entry = capture_source_entry(
                &parent,
                OsStr::new(name),
                vec![candidate.clone()],
                directory.as_ref(),
                &mut description,
                &mut metadata_bytes,
                cancel,
            )?;
            let root = SourceRoot {
                parent,
                parent_path,
                parent_identity,
                entry,
            };
            root.check_binding()?;
            roots.push(root);
            description.roots.push(candidate);
        }
        description.name = if description.roots.len() == 1 {
            description.roots[0].clone()
        } else {
            format!("{} items", description.roots.len())
        };
        description.validate()?;
        cancelled(cancel)?;
        Ok(Self {
            captured_description: description.clone(),
            description,
            roots,
        })
    }

    /// Read and spool the selected bytes only after a receiver demands them.
    /// The same captured identities are checked around every source read.
    pub fn prepare(
        &self,
        state_dir: &Path,
        id: &str,
        max_bytes: u64,
        cancel: &AtomicBool,
        progress: impl FnMut(u64),
    ) -> Result<Export> {
        Export::prepare_selection(state_dir, id, self, max_bytes, cancel, progress)
    }
}

fn capture_source_entry(
    parent: &Dir,
    name: &OsStr,
    path: Vec<String>,
    pinned: Option<&Dir>,
    description: &mut Description,
    metadata_bytes: &mut usize,
    cancel: &AtomicBool,
) -> Result<SourceEntry> {
    cancelled(cancel)?;
    ensure!(
        path.len() <= 64 && description.entries.len() < MAX_ENTRIES,
        "The transfer contains too many files or nested folders"
    );
    ensure!(
        path.iter().map(String::len).sum::<usize>() + path.len() <= 4096,
        "The file transfer path is too deep or too long"
    );
    let metadata = SourceMetadata::of(&parent.symlink_metadata(name)?)?;
    let kind = if metadata.directory {
        DescriptorKind::Directory
    } else {
        description.total = description
            .total
            .checked_add(metadata.length)
            .filter(|total| *total <= MAX_TOTAL)
            .context("The copied files exceed their sharing limits")?;
        DescriptorKind::File {
            size: metadata.length,
        }
    };
    charge_entry(
        &Entry {
            path: path.clone(),
            kind: match kind {
                DescriptorKind::Directory => Kind::Directory,
                DescriptorKind::File { size } => Kind::File {
                    size,
                    sha256: "0".repeat(64),
                },
            },
        },
        metadata_bytes,
    )?;
    description.entries.push(Descriptor {
        path: path.clone(),
        kind,
    });
    let mut children = Vec::new();
    if metadata.directory {
        let directory = match pinned {
            Some(directory) => directory.try_clone()?,
            None => source_directory(parent, name)?,
        };
        metadata.check(&directory.dir_metadata()?)?;
        // Retain only the bounded captured tree. Do not collect an unbounded
        // sibling list, sort it, or open any child file to enumerate metadata.
        for entry in directory.entries()? {
            cancelled(cancel)?;
            let name = entry?.file_name();
            let text = name.to_str().context("File names must be valid Unicode")?;
            protocol::valid_component(text)?;
            let mut next = path.clone();
            next.push(text.to_owned());
            children.push(capture_source_entry(
                &directory,
                &name,
                next,
                None,
                description,
                metadata_bytes,
                cancel,
            )?);
        }
        metadata.check(&directory.dir_metadata()?)?;
    }
    metadata.check(&parent.symlink_metadata(name)?)?;
    Ok(SourceEntry {
        name: name.to_owned(),
        path,
        metadata,
        children,
    })
}

pub struct Export {
    pub manifest: Manifest,
    // Keep the owned directory pinned for the lifetime of this export. In
    // particular, Windows must not allow its namespace to be replaced mid-read.
    _root: Dir,
    blobs: Dir,
}

impl Export {
    /// Garbage-collect an owned, terminal export without reopening all of its
    /// data files. Only metadata grants deletion authority; recursive unlink
    /// stays inside the already opened directory and never follows child links.
    pub fn discard(state_dir: &Path, id: &str) -> Result<()> {
        protocol::valid_id(id)?;
        let state = match absolute_directory(state_dir, false) {
            Ok(state) => state,
            Err(error) if missing(&error) => return Ok(()),
            Err(error) => return Err(error),
        };
        let mut parent = state;
        let mut containing = None;
        for (index, name) in ["file-transfer", "exports", id].into_iter().enumerate() {
            if index == 2 {
                containing = Some(parent.try_clone()?);
            }
            parent = match directory(&parent, OsStr::new(name), false) {
                Ok(child) => child,
                Err(error) if missing(&error) => return Ok(()),
                Err(error) => return Err(error),
            };
            private_directory(&parent)?;
        }
        if exists(&parent, "preparing.json")? {
            let owner: PreparationOwner = read_json(&parent, "preparing.json", CHECKPOINT_LIMIT)?;
            ensure!(
                owner.format == FORMAT
                    && owner.id == id
                    && owner.purpose == "desk-switch-export-preparation",
                "The unfinished export ownership marker is invalid"
            );
        } else {
            let manifest: Manifest = read_json(&parent, "manifest.json", MAX_MANIFEST)?;
            manifest.validate()?;
            let owner: ExportOwner = read_json(&parent, "owner.json", CHECKPOINT_LIMIT)?;
            ensure!(
                owner.format == FORMAT
                    && owner.id == id
                    && manifest.id == id
                    && owner.digest == manifest.digest()?,
                "The saved export does not match its manifest"
            );
        }
        parent.remove_open_dir_all()?;
        if let Some(containing) = containing {
            sync_dir(&containing)?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn prepare(
        state_dir: &Path,
        id: &str,
        paths: &[PathBuf],
        cancel: &AtomicBool,
        progress: impl FnMut(u64),
    ) -> Result<Self> {
        Self::prepare_with_limit(state_dir, id, paths, MAX_TOTAL, cancel, progress)
    }

    /// The service serializes preparations and subtracts existing exports from
    /// its workspace spool budget before calling this method.
    pub fn prepare_with_limit(
        state_dir: &Path,
        id: &str,
        paths: &[PathBuf],
        max_bytes: u64,
        cancel: &AtomicBool,
        progress: impl FnMut(u64),
    ) -> Result<Self> {
        SourceSelection::capture(paths, cancel)?.prepare(state_dir, id, max_bytes, cancel, progress)
    }

    fn prepare_selection(
        state_dir: &Path,
        id: &str,
        selection: &SourceSelection,
        max_bytes: u64,
        cancel: &AtomicBool,
        mut progress: impl FnMut(u64),
    ) -> Result<Self> {
        cancelled(cancel)?;
        ensure!(
            max_bytes <= MAX_TOTAL,
            "Invalid file transfer storage budget"
        );
        protocol::valid_id(id)?;
        selection.description.validate()?;
        ensure!(
            selection.description == selection.captured_description,
            "The copied file selection metadata changed"
        );
        ensure!(
            selection.description.total <= max_bytes,
            "The transfer exceeds the available file sharing storage budget"
        );
        let mut source_pins = Vec::with_capacity(selection.roots.len());
        for source in &selection.roots {
            cancelled(cancel)?;
            let pins = source.pin_for_read()?;
            check_source_entry(&pins.0, &source.entry, pins.1.as_ref(), cancel)?;
            source_pins.push(pins);
        }
        let state = absolute_directory(state_dir, true)?;
        let transfer = directory(&state, OsStr::new("file-transfer"), true)?;
        private_directory(&transfer)?;
        let exports = directory(&transfer, OsStr::new("exports"), true)?;
        private_directory(&exports)?;
        // Never reuse a partially prepared or independently created directory.
        exports.create_dir_with(id, &private_builder())?;
        let root = exports.open_dir_nofollow(id)?;
        private_directory(&root)?;
        let result = (|| {
            // Commit deletion authority before storing any copied source bytes.
            // A process restart can then remove an interrupted preparation
            // without treating an arbitrary pre-existing folder as our own.
            write_new(
                &root,
                "preparing.json",
                &serde_json::to_vec(&PreparationOwner {
                    format: FORMAT,
                    id: id.into(),
                    purpose: "desk-switch-export-preparation".into(),
                })?,
            )?;
            sync_dir(&exports)?;
            let blobs = directory(&root, OsStr::new("blobs"), true)?;
            let mut manifest = Manifest {
                version: FORMAT,
                id: id.into(),
                name: String::new(),
                roots: Vec::new(),
                entries: Vec::new(),
                total: 0,
            };
            // Reserve the fixed envelope and display-name budget. Charge each
            // serialized entry before copying its bytes, rather than allowing a
            // large local directory to build an oversized manifest in memory.
            let mut metadata_bytes = 1024_usize;
            for (source, (source_parent, source_directory)) in
                selection.roots.iter().zip(&source_pins)
            {
                cancelled(cancel)?;
                source.check_binding()?;
                let candidate = &source.entry.path[0];
                metadata_bytes += serde_json::to_vec(&candidate)?.len() + 1;
                ensure!(
                    metadata_bytes <= MAX_MANIFEST,
                    "Transfer metadata is too large"
                );
                snapshot_entry(
                    source_parent,
                    &source.entry,
                    source_directory.as_ref(),
                    &blobs,
                    &mut manifest,
                    &mut SnapshotControl {
                        metadata_bytes: &mut metadata_bytes,
                        max_bytes,
                        cancel,
                        progress: &mut progress,
                    },
                )?;
                source.check_binding()?;
                manifest.roots.push(candidate.clone());
            }
            manifest.name = if manifest.roots.len() == 1 {
                manifest.roots[0].clone()
            } else {
                format!("{} items", manifest.roots.len())
            };
            manifest.validate()?;
            ensure!(
                manifest.description() == selection.description,
                "The copied file selection changed before the transfer"
            );
            for (source, (source_parent, source_directory)) in
                selection.roots.iter().zip(&source_pins)
            {
                source.check_binding()?;
                check_source_entry(
                    source_parent,
                    &source.entry,
                    source_directory.as_ref(),
                    cancel,
                )?;
            }
            cancelled(cancel)?;
            sync_dir(&blobs)?;
            let encoded = serde_json::to_vec(&manifest)?;
            ensure!(
                encoded.len() <= MAX_MANIFEST,
                "Transfer metadata is too large"
            );
            write_new(&root, "manifest.json", &encoded)?;
            write_json(
                &root,
                "owner.json",
                &ExportOwner {
                    format: FORMAT,
                    id: id.into(),
                    digest: manifest.digest()?,
                },
            )?;
            root.remove_file("preparing.json")?;
            sync_dir(&root)?;
            sync_dir(&exports)?;
            Ok::<_, anyhow::Error>((manifest, blobs))
        })();
        match result {
            Ok((manifest, blobs)) => Ok(Self {
                manifest,
                _root: root,
                blobs,
            }),
            Err(error) => {
                // `root` is the directory just created by this call. No
                // path received from a peer participates in cleanup.
                let _ = root.remove_open_dir_all();
                Err(error)
            }
        }
    }

    pub fn load(state_dir: &Path, id: &str) -> Result<Self> {
        protocol::valid_id(id)?;
        let state = absolute_directory(state_dir, false)?;
        let transfer = directory(&state, OsStr::new("file-transfer"), false)?;
        private_directory(&transfer)?;
        let exports = directory(&transfer, OsStr::new("exports"), false)?;
        private_directory(&exports)?;
        let root = directory(&exports, OsStr::new(id), false)?;
        private_directory(&root)?;
        let manifest: Manifest = read_json(&root, "manifest.json", MAX_MANIFEST)?;
        manifest.validate()?;
        let owner: ExportOwner = read_json(&root, "owner.json", CHECKPOINT_LIMIT)?;
        ensure!(
            owner.format == FORMAT
                && owner.id == id
                && manifest.id == id
                && owner.digest == manifest.digest()?,
            "The saved export does not match its manifest"
        );
        let blobs = directory(&root, OsStr::new("blobs"), false)?;
        for (index, entry) in manifest.entries.iter().enumerate() {
            if let Kind::File { size, .. } = &entry.kind {
                let file = open_file(&blobs, file_name(index as u32), false, false)?;
                ensure!(
                    regular(&file, true)?.len() == *size,
                    "The saved export has changed"
                );
            }
        }
        Ok(Self {
            manifest,
            _root: root,
            blobs,
        })
    }

    fn file(&self, index: u32) -> Result<(File, u64)> {
        let entry = self
            .manifest
            .entries
            .get(index as usize)
            .context("Unknown exported file")?;
        let Kind::File { size, .. } = &entry.kind else {
            bail!("The requested entry is not a file");
        };
        let file = open_file(&self.blobs, file_name(index), false, false)?;
        ensure!(
            regular(&file, true)?.len() == *size,
            "The saved export has changed"
        );
        Ok((file, *size))
    }

    pub fn read(&self, index: u32, offset: u64, limit: usize) -> Result<Vec<u8>> {
        ensure!((1..=CHUNK).contains(&limit), "Invalid transfer chunk size");
        let (mut file, size) = self.file(index)?;
        ensure!(offset <= size, "Invalid exported file offset");
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; usize::try_from((size - offset).min(limit as u64))?];
        file.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    #[cfg(test)]
    pub fn prefix(&self, index: u32, length: u64) -> Result<String> {
        self.prefix_with_cancel(index, length, &NO_CANCEL)
    }

    pub fn prefix_with_cancel(
        &self,
        index: u32,
        length: u64,
        cancel: &AtomicBool,
    ) -> Result<String> {
        cancelled(cancel)?;
        let (mut file, size) = self.file(index)?;
        ensure!(length <= size, "Invalid exported file offset");
        Ok(finished_hash(&hash_prefix(&mut file, length, cancel)?))
    }
}

fn private_builder() -> DirBuilder {
    #[cfg(unix)]
    {
        use cap_std::fs::DirBuilderExt;
        let mut builder = DirBuilder::new();
        builder.mode(0o700);
        builder
    }
    #[cfg(not(unix))]
    {
        DirBuilder::new()
    }
}

struct SnapshotControl<'a> {
    metadata_bytes: &'a mut usize,
    max_bytes: u64,
    cancel: &'a AtomicBool,
    progress: &'a mut dyn FnMut(u64),
}

fn snapshot_entry(
    source: &Dir,
    selected: &SourceEntry,
    pinned: Option<&Dir>,
    blobs: &Dir,
    manifest: &mut Manifest,
    control: &mut SnapshotControl<'_>,
) -> Result<()> {
    cancelled(control.cancel)?;
    let name = &selected.name;
    let path = &selected.path;
    ensure!(
        path.len() <= 64 && manifest.entries.len() < MAX_ENTRIES,
        "The transfer contains too many files or nested folders"
    );
    ensure!(
        path.iter().map(String::len).sum::<usize>() + path.len() <= 4096,
        "The file transfer path is too deep or too long"
    );
    let metadata = source.symlink_metadata(name)?;
    selected.metadata.check(&metadata)?;
    if metadata.is_dir() {
        let child = match pinned {
            Some(directory) => directory.try_clone()?,
            None => source.open_dir_nofollow(name)?,
        };
        selected.metadata.check(&child.dir_metadata()?)?;
        check_source_children(&child, &selected.children, control.cancel)?;
        let entry = Entry {
            path: path.clone(),
            kind: Kind::Directory,
        };
        charge_entry(&entry, control.metadata_bytes)?;
        manifest.entries.push(entry);
        // Use the captured order, rather than trusting a later filesystem
        // enumeration to preserve the manifest's numeric entry identities.
        for entry in &selected.children {
            snapshot_entry(&child, entry, None, blobs, manifest, control)?;
        }
        check_source_children(&child, &selected.children, control.cancel)?;
        selected.metadata.check(&child.dir_metadata()?)?;
        selected.metadata.check(&source.symlink_metadata(name)?)?;
        return Ok(());
    }
    ensure!(
        metadata.is_file(),
        "Only regular files and folders can be transferred"
    );
    let mut file = open_file(source, name, false, false)?;
    let before = regular(&file, false)?;
    selected.metadata.check(&before)?;
    let size = before.len();
    ensure!(
        manifest
            .total
            .checked_add(size)
            .is_some_and(|total| total <= control.max_bytes),
        "The transfer exceeds the available file sharing storage budget"
    );
    let mut entry = Entry {
        path: path.clone(),
        kind: Kind::File {
            size,
            sha256: "0".repeat(64),
        },
    };
    charge_entry(&entry, control.metadata_bytes)?;
    let mut target = open_file(blobs, file_name(manifest.entries.len() as u32), true, true)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; BUFFER];
    let mut count = 0_u64;
    while count < size {
        cancelled(control.cancel)?;
        let length = usize::try_from((size - count).min(BUFFER as u64))?;
        file.read_exact(&mut buffer[..length])
            .context("A source file changed while preparing the transfer")?;
        target.write_all(&buffer[..length])?;
        hash.update(&buffer[..length]);
        count += length as u64;
        (control.progress)(manifest.total + count);
    }
    let mut extra = [0];
    ensure!(
        file.read(&mut extra)? == 0,
        "A source file changed while preparing the transfer"
    );
    let after = regular(&file, false)?;
    selected.metadata.check(&after)?;
    selected.metadata.check(&source.symlink_metadata(name)?)?;
    target.sync_all()?;
    manifest.total += size;
    entry.kind = Kind::File {
        size,
        sha256: format!("{:x}", hash.finalize()),
    };
    manifest.entries.push(entry);
    Ok(())
}

fn check_source_children(
    directory: &Dir,
    children: &[SourceEntry],
    cancel: &AtomicBool,
) -> Result<()> {
    let mut names = children
        .iter()
        .map(|child| child.name.clone())
        .collect::<BTreeSet<_>>();
    for entry in directory.entries()? {
        cancelled(cancel)?;
        ensure!(
            names.remove(&entry?.file_name()),
            "A copied folder changed; copy the selection again"
        );
    }
    ensure!(
        names.is_empty(),
        "A copied folder changed; copy the selection again"
    );
    Ok(())
}

fn check_source_entry(
    parent: &Dir,
    entry: &SourceEntry,
    pinned: Option<&Dir>,
    cancel: &AtomicBool,
) -> Result<()> {
    cancelled(cancel)?;
    entry
        .metadata
        .check(&parent.symlink_metadata(&entry.name)?)?;
    if entry.metadata.directory {
        let directory = match pinned {
            Some(directory) => directory.try_clone()?,
            None => parent.open_dir_nofollow(&entry.name)?,
        };
        entry.metadata.check(&directory.dir_metadata()?)?;
        check_source_children(&directory, &entry.children, cancel)?;
        for child in &entry.children {
            check_source_entry(&directory, child, None, cancel)?;
        }
        entry.metadata.check(&directory.dir_metadata()?)?;
    }
    entry
        .metadata
        .check(&parent.symlink_metadata(&entry.name)?)?;
    Ok(())
}

fn charge_entry(entry: &Entry, bytes: &mut usize) -> Result<()> {
    *bytes = bytes
        .checked_add(serde_json::to_vec(entry)?.len() + 1)
        .context("Transfer metadata is too large")?;
    ensure!(*bytes <= MAX_MANIFEST, "Transfer metadata is too large");
    Ok(())
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct Binding {
    format: u16,
    id: String,
    peer: String,
    workspace: String,
    manifest: String,
}

#[derive(Deserialize, Serialize)]
struct ReceiverOwner {
    binding: Binding,
    cancelled: bool,
    bundle: Option<String>,
    finished: bool,
}

#[derive(Deserialize, Serialize)]
struct Checkpoint {
    format: u16,
    id: String,
    index: u32,
    offset: u64,
    sha256: String,
}

pub struct Receive {
    pub manifest: Manifest,
    base: Dir,
    base_path: PathBuf,
    stage: Option<Dir>,
    blobs: Option<Dir>,
    owner: ReceiverOwner,
    checked: BTreeMap<u32, (u64, Sha256)>,
    // Pin the tree that was actually verified, not a later path supplied by a
    // native clipboard consumer or a replaced public Downloads namespace.
    finished_tree: Option<Dir>,
}

impl Receive {
    pub fn is_complete(&self) -> bool {
        self.owner.finished
    }

    pub fn open(
        downloads: &Path,
        manifest: Manifest,
        peer: &str,
        workspace_id: &str,
    ) -> Result<Self> {
        manifest.validate()?;
        ensure!(
            !peer.is_empty()
                && peer.len() <= 256
                && !workspace_id.is_empty()
                && workspace_id.len() <= 256,
            "Invalid transfer membership binding"
        );
        let binding = Binding {
            format: FORMAT,
            id: manifest.id.clone(),
            peer: peer.into(),
            workspace: workspace_id.into(),
            manifest: manifest.digest()?,
        };
        let base = absolute_directory(downloads, true)?;
        let cancelled_name = format!(".cancelled-{}.json", manifest.id);
        if exists(&base, &cancelled_name)? {
            Self::cleanup_cancelled(downloads, &manifest, peer, workspace_id)?;
            bail!("The file transfer was cancelled");
        }
        let stage_name = format!(".partial-{}", manifest.id);
        let owner;
        let stage = match base.create_dir_with(&stage_name, &private_builder()) {
            Ok(()) => {
                let stage = directory(&base, OsStr::new(&stage_name), false)?;
                owner = ReceiverOwner {
                    binding,
                    cancelled: false,
                    bundle: None,
                    finished: false,
                };
                write_json(&stage, "owner.json", &owner)?;
                write_new(&stage, "manifest.json", &serde_json::to_vec(&manifest)?)?;
                sync_dir(&base)?;
                stage
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let stage = directory(&base, OsStr::new(&stage_name), false)?;
                private_directory(&stage)?;
                owner = read_json::<ReceiverOwner>(&stage, "owner.json", CHECKPOINT_LIMIT)?;
                ensure!(
                    owner.binding == binding,
                    "The saved transfer belongs to a different export or computer"
                );
                let saved: Manifest = read_json(&stage, "manifest.json", MAX_MANIFEST)?;
                ensure!(saved == manifest, "The saved transfer manifest has changed");
                ensure!(!owner.cancelled, "The file transfer was cancelled");
                stage
            }
            Err(error) => return Err(error.into()),
        };
        private_directory(&stage)?;
        let blobs = directory(&stage, OsStr::new("blobs"), true)?;
        let mut receive = Self {
            manifest,
            base,
            base_path: downloads.to_owned(),
            stage: Some(stage),
            blobs: Some(blobs),
            owner,
            checked: BTreeMap::new(),
            finished_tree: None,
        };
        if receive.owner.finished {
            receive.finished_tree = Some(receive.completed_tree()?);
        }
        Ok(receive)
    }

    fn completed_tree(&self) -> Result<Dir> {
        ensure!(
            self.owner.finished && !self.owner.cancelled,
            "The file transfer is not complete"
        );
        let name = self
            .owner
            .bundle
            .as_ref()
            .context("The completed receiving folder is missing")?;
        self.valid_bundle_name(name)?;
        let bundle = directory(&self.base, OsStr::new(name), false)?;
        private_directory(&bundle)?;
        ensure!(
            read_json::<Binding>(&bundle, "transfer.json", CHECKPOINT_LIMIT)? == self.owner.binding,
            "The receiving folder does not belong to this transfer"
        );
        directory(&bundle, OsStr::new("files"), false)
    }

    /// Supply a native virtual-file consumer with a read-only handle to one
    /// completed manifest entry. No caller-controlled path is accepted. Hash
    /// the pinned file again because the public finished copy is user-editable.
    pub fn open_verified(&self, index: usize) -> Result<std::fs::File> {
        self.open_verified_cancellable(index, &NO_CANCEL)
    }

    pub fn open_verified_cancellable(
        &self,
        index: usize,
        cancel: &AtomicBool,
    ) -> Result<std::fs::File> {
        cancelled(cancel)?;
        ensure!(
            self.owner.finished && !self.owner.cancelled,
            "The file transfer is not complete"
        );
        ensure!(
            self.manifest.digest()? == self.owner.binding.manifest,
            "The completed file manifest changed"
        );
        let entry = self
            .manifest
            .entries
            .get(index)
            .context("Unknown received file")?;
        let Kind::File { size, sha256 } = &entry.kind else {
            bail!("The requested entry is not a file");
        };
        let tree = self
            .finished_tree
            .as_ref()
            .context("The verified receiving folder is closed")?;
        let (parent, name) = path_parent(tree, &entry.path, false)?;
        let mut file = open_file(&parent, &name, false, false)?;
        let before = SourceMetadata::of(&regular(&file, true)?)?;
        ensure!(
            before.length == *size,
            "The completed transfer file changed"
        );
        ensure!(
            finished_hash(&hash_prefix(&mut file, *size, cancel)?) == *sha256,
            "The completed transfer file failed verification"
        );
        cancelled(cancel)?;
        let mut extra = [0];
        ensure!(
            file.read(&mut extra)? == 0,
            "The completed transfer file changed"
        );
        before.check(&regular(&file, true)?)?;
        before.check(&parent.symlink_metadata(&name)?)?;
        file.seek(SeekFrom::Start(0))?;
        cancelled(cancel)?;
        Ok(file.into_std())
    }

    fn active(&self) -> Result<(&Dir, &Dir)> {
        ensure!(!self.owner.cancelled, "The file transfer was cancelled");
        ensure!(
            !self.owner.finished,
            "The file transfer is already complete"
        );
        Ok((
            self.stage
                .as_ref()
                .context("The transfer storage is closed")?,
            self.blobs
                .as_ref()
                .context("The transfer storage is closed")?,
        ))
    }

    fn size_hash(&self, index: u32) -> Result<(u64, &str)> {
        let entry = self
            .manifest
            .entries
            .get(index as usize)
            .context("Unknown received file")?;
        let Kind::File { size, sha256 } = &entry.kind else {
            bail!("The requested entry is not a file");
        };
        Ok((*size, sha256))
    }

    pub fn position(&mut self, index: u32) -> Result<(u64, String)> {
        self.position_with_cancel(index, &NO_CANCEL)
    }

    pub fn position_with_cancel(
        &mut self,
        index: u32,
        cancel: &AtomicBool,
    ) -> Result<(u64, String)> {
        cancelled(cancel)?;
        let (size, expected) = self.size_hash(index)?;
        let expected = expected.to_owned();
        let (stage, blobs) = self.active()?;
        if let Some((offset, hash)) = self.checked.get(&index) {
            let file = open_file(blobs, file_name(index), true, false)?;
            let length = regular(&file, true)?.len();
            ensure!(length >= *offset, "The saved transfer data is incomplete");
            if length > *offset {
                file.set_len(*offset)?;
                file.sync_all()?;
            }
            return Ok((*offset, finished_hash(hash)));
        }
        let checkpoint = if exists(stage, &checkpoint_name(index))? {
            read_json::<Checkpoint>(stage, &checkpoint_name(index), CHECKPOINT_LIMIT)?
        } else {
            Checkpoint {
                format: FORMAT,
                id: self.manifest.id.clone(),
                index,
                offset: 0,
                sha256: digest(&[]),
            }
        };
        ensure!(
            checkpoint.format == FORMAT
                && checkpoint.id == self.manifest.id
                && checkpoint.index == index
                && checkpoint.offset <= size,
            "The saved transfer checkpoint is invalid"
        );
        let name = file_name(index);
        let found = exists(blobs, &name)?;
        let mut file = if found {
            open_file(blobs, &name, true, false)?
        } else {
            ensure!(checkpoint.offset == 0, "The saved transfer data is missing");
            open_file(blobs, &name, true, true)?
        };
        ensure!(
            regular(&file, true)?.len() >= checkpoint.offset,
            "The saved transfer data is incomplete"
        );
        let hash = hash_prefix(&mut file, checkpoint.offset, cancel)?;
        ensure!(
            finished_hash(&hash) == checkpoint.sha256,
            "The saved transfer data failed verification"
        );
        if checkpoint.offset == size {
            ensure!(
                finished_hash(&hash) == expected,
                "The saved transfer file failed verification"
            );
        }
        // Bytes after the last flushed checkpoint were never acknowledged.
        file.set_len(checkpoint.offset)?;
        file.sync_all()?;
        if !found {
            sync_dir(blobs)?;
        }
        let result = (checkpoint.offset, checkpoint.sha256);
        self.checked.insert(index, (checkpoint.offset, hash));
        Ok(result)
    }

    pub fn write(&mut self, index: u32, offset: u64, bytes: &[u8], sha256: &str) -> Result<u64> {
        ensure!(
            !bytes.is_empty() && bytes.len() <= CHUNK && digest(bytes) == sha256,
            "The received file chunk failed verification"
        );
        let (size, expected) = self.size_hash(index)?;
        let expected = expected.to_owned();
        let (committed, _) = self.position(index)?;
        ensure!(
            offset == committed
                && offset
                    .checked_add(bytes.len() as u64)
                    .is_some_and(|next| next <= size),
            "The received file chunk has an invalid offset"
        );
        let mut hash = self.checked[&index].1.clone();
        hash.update(bytes);
        let next = offset + bytes.len() as u64;
        let checksum = finished_hash(&hash);
        if next == size {
            ensure!(
                checksum == expected,
                "The received file failed verification"
            );
        }
        let (stage, blobs) = self.active()?;
        let mut file = open_file(blobs, file_name(index), true, false)?;
        ensure!(
            regular(&file, true)?.len() == offset,
            "The saved transfer data changed while receiving"
        );
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        write_json(
            stage,
            &checkpoint_name(index),
            &Checkpoint {
                format: FORMAT,
                id: self.manifest.id.clone(),
                index,
                offset: next,
                sha256: checksum,
            },
        )?;
        self.checked.insert(index, (next, hash));
        Ok(next)
    }

    fn persist(&self) -> Result<()> {
        write_json(
            self.stage
                .as_ref()
                .context("The transfer storage is closed")?,
            "owner.json",
            &self.owner,
        )
    }

    fn bundle(&mut self) -> Result<(String, Dir)> {
        if let Some(name) = &self.owner.bundle {
            self.valid_bundle_name(name)?;
            let bundle = directory(&self.base, OsStr::new(name), false)?;
            private_directory(&bundle)?;
            ensure!(
                read_json::<Binding>(&bundle, "transfer.json", CHECKPOINT_LIMIT)?
                    == self.owner.binding,
                "The receiving folder does not belong to this transfer"
            );
            return Ok((name.clone(), bundle));
        }
        let prefix = format!("Transfer {}", &self.manifest.id[..12]);
        for suffix in 0..10000 {
            let name = if suffix == 0 {
                prefix.clone()
            } else {
                format!("{prefix} ({suffix})")
            };
            match self.base.create_dir_with(&name, &private_builder()) {
                Ok(()) => {
                    let bundle = directory(&self.base, OsStr::new(&name), false)?;
                    write_json(&bundle, "transfer.json", &self.owner.binding)?;
                    self.owner.bundle = Some(name.clone());
                    self.persist()?;
                    sync_dir(&self.base)?;
                    return Ok((name, bundle));
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
        bail!("Could not choose an unused receiving folder")
    }

    fn valid_bundle_name(&self, name: &str) -> Result<()> {
        protocol::valid_component(name)?;
        let prefix = format!("Transfer {}", &self.manifest.id[..12]);
        ensure!(
            name == prefix
                || name
                    .strip_prefix(&format!("{prefix} ("))
                    .and_then(|suffix| suffix.strip_suffix(')'))
                    .and_then(|suffix| suffix.parse::<u32>().ok())
                    .is_some_and(|suffix| (1..10000).contains(&suffix)),
            "The saved receiving folder name is invalid"
        );
        Ok(())
    }

    /// All verified roots appear together when `payload` is renamed into the
    /// exclusively reserved bundle. Existing Downloads content is never used as
    /// an extraction directory or opened for writing.
    #[cfg(test)]
    pub fn finish(&mut self) -> Result<Vec<PathBuf>> {
        self.finish_with_cancel(&NO_CANCEL)
    }

    pub fn finish_with_cancel(&mut self, cancel: &AtomicBool) -> Result<Vec<PathBuf>> {
        ensure!(!self.owner.cancelled, "The file transfer was cancelled");
        let (bundle_name, bundle) = self.bundle()?;
        let files = if exists(&bundle, "files")? {
            let files = directory(&bundle, OsStr::new("files"), false)?;
            // Once the atomic rename committed, cancellation must never remove
            // or abandon the user's verified files.
            verify_tree(&files, &self.manifest, &NO_CANCEL)?;
            files
        } else {
            cancelled(cancel)?;
            ensure!(
                !self.owner.finished,
                "The completed receiving folder is missing"
            );
            for index in 0..self.manifest.entries.len() {
                if let Kind::File { size, .. } = self.manifest.entries[index].kind {
                    ensure!(
                        self.position_with_cancel(index as u32, cancel)?.0 == size,
                        "The file transfer is not complete"
                    );
                }
            }
            let (stage, blobs) = self.active()?;
            if exists(stage, "payload")? {
                // This is our unfinished materialization, not a user folder.
                let payload = directory(stage, OsStr::new("payload"), false)?;
                private_directory(&payload)?;
                payload.remove_open_dir_all()?;
            }
            let payload = directory(stage, OsStr::new("payload"), true)?;
            materialize(&payload, blobs, &self.manifest, cancel)?;
            verify_tree(&payload, &self.manifest, cancel)?;
            sync_dir(&payload)?;
            drop(payload);
            cancelled(cancel)?;
            // `bundle` was created exclusively and has an ownership receipt;
            // directory replacement cannot overwrite a pre-existing file tree.
            ensure!(
                !exists(&bundle, "files")?,
                "The receiving folder is no longer empty"
            );
            stage.rename("payload", &bundle, "files")?;
            sync_dir(&bundle)?;
            sync_dir(stage)?;
            directory(&bundle, OsStr::new("files"), false)?
        };
        self.owner.finished = true;
        self.persist()?;
        self.finished_tree = Some(files);
        let paths = self
            .manifest
            .roots
            .iter()
            .map(|name| self.base_path.join(&bundle_name).join("files").join(name))
            .collect();
        // Keep only the small receipt for idempotent completion/restart; the
        // immutable public copy no longer shares storage with partial blobs.
        if let Some(blobs) = self.blobs.take() {
            let _ = blobs.remove_open_dir_all();
        }
        Ok(paths)
    }

    /// Remove a completed native-paste cache and its staging directory.
    /// The caller must have exclusive ownership: this is used only for the
    /// private native cache, never for a user-visible Downloads bundle.
    pub fn remove_cache(&mut self) -> Result<()> {
        // Drop every retained capability before removing the directories it
        // points at; Windows refuses to delete a tree with live child handles.
        self.stage = None;
        self.blobs = None;
        self.finished_tree = None;
        if let Some(name) = self.owner.bundle.clone() {
            self.valid_bundle_name(&name)?;
            if exists(&self.base, &name)? {
                let bundle = directory(&self.base, OsStr::new(&name), false)?;
                private_directory(&bundle)?;
                bundle.remove_open_dir_all()?;
            }
        }
        let stage_name = format!(".partial-{}", self.manifest.id);
        if exists(&self.base, &stage_name)? {
            let stage = directory(&self.base, OsStr::new(&stage_name), false)?;
            private_directory(&stage)?;
            stage.remove_open_dir_all()?;
        }
        sync_dir(&self.base)
    }

    pub fn cancel(&mut self) -> Result<()> {
        ensure!(
            !self.owner.finished,
            "Completed transfers cannot be cancelled"
        );
        let bundle = if let Some(name) = &self.owner.bundle {
            self.valid_bundle_name(name)?;
            if exists(&self.base, name)? {
                let bundle = directory(&self.base, OsStr::new(name), false)?;
                ensure!(
                    read_json::<Binding>(&bundle, "transfer.json", CHECKPOINT_LIMIT)?
                        == self.owner.binding,
                    "The receiving folder does not belong to this transfer"
                );
                // A crash immediately after atomic finalization must not turn
                // a later cancel into deletion of user-visible files.
                ensure!(
                    !exists(&bundle, "files")?,
                    "Completed transfers cannot be cancelled"
                );
                Some(bundle)
            } else {
                None
            }
        } else {
            None
        };
        let cancelled_name = format!(".cancelled-{}.json", self.manifest.id);
        if !exists(&self.base, &cancelled_name)? {
            write_new(
                &self.base,
                &cancelled_name,
                &serde_json::to_vec(&self.owner.binding)?,
            )?;
        } else {
            ensure!(
                read_json::<Binding>(&self.base, &cancelled_name, CHECKPOINT_LIMIT)?
                    == self.owner.binding,
                "The cancelled transfer belongs to another export"
            );
        }
        self.owner.cancelled = true;
        if self.stage.is_some() {
            self.persist()?;
        }
        // Commit the tombstone and cancellation receipt before destructive
        // cleanup. Restarted cleanup tolerates a reservation already removed.
        if let Some(bundle) = bundle {
            bundle.remove_open_dir_all()?;
        }
        self.checked.clear();
        drop(self.blobs.take());
        if let Some(stage) = self.stage.take() {
            stage.remove_open_dir_all()?;
        }
        sync_dir(&self.base)
    }

    /// Retry cleanup after a process exits between durable cancellation and
    /// removing its owned files. This never reopens a cancelled transfer for use.
    pub fn cleanup_cancelled(
        downloads: &Path,
        manifest: &Manifest,
        peer: &str,
        workspace_id: &str,
    ) -> Result<()> {
        manifest.validate()?;
        let binding = Binding {
            format: FORMAT,
            id: manifest.id.clone(),
            peer: peer.into(),
            workspace: workspace_id.into(),
            manifest: manifest.digest()?,
        };
        let base = absolute_directory(downloads, false)?;
        let name = format!(".cancelled-{}.json", manifest.id);
        ensure!(
            read_json::<Binding>(&base, &name, CHECKPOINT_LIMIT)? == binding,
            "The cancelled transfer belongs to another export"
        );
        let stage_name = format!(".partial-{}", manifest.id);
        if !exists(&base, &stage_name)? {
            return Ok(());
        }
        let stage = directory(&base, OsStr::new(&stage_name), false)?;
        private_directory(&stage)?;
        let owner: ReceiverOwner = read_json(&stage, "owner.json", CHECKPOINT_LIMIT)?;
        ensure!(
            owner.binding == binding
                && read_json::<Manifest>(&stage, "manifest.json", MAX_MANIFEST)? == *manifest,
            "The saved transfer belongs to a different export or computer"
        );
        let mut receive = Self {
            manifest: manifest.clone(),
            base,
            base_path: downloads.to_owned(),
            stage: Some(stage),
            blobs: None,
            owner,
            checked: BTreeMap::new(),
            finished_tree: None,
        };
        receive.cancel()
    }
}

fn path_parent(root: &Dir, path: &[String], create: bool) -> Result<(Dir, String)> {
    let (name, parents) = path
        .split_last()
        .context("An empty relative path cannot be transferred")?;
    let mut current = root.try_clone()?;
    for parent in parents {
        protocol::valid_component(parent)?;
        current = directory(&current, OsStr::new(parent), create)?;
    }
    protocol::valid_component(name)?;
    Ok((current, name.clone()))
}

fn materialize(root: &Dir, blobs: &Dir, manifest: &Manifest, cancel: &AtomicBool) -> Result<()> {
    for (index, entry) in manifest.entries.iter().enumerate() {
        cancelled(cancel)?;
        let (parent, name) = path_parent(root, &entry.path, true)?;
        match &entry.kind {
            Kind::Directory => {
                directory(&parent, OsStr::new(&name), true)?;
            }
            Kind::File { size, sha256 } => {
                let mut source = open_file(blobs, file_name(index as u32), false, false)?;
                ensure!(
                    regular(&source, true)?.len() == *size,
                    "The verified transfer file has changed"
                );
                let mut target = open_file(&parent, &name, true, true)?;
                let mut hash = Sha256::new();
                let mut buffer = [0; BUFFER];
                let mut left = *size;
                while left != 0 {
                    cancelled(cancel)?;
                    let count = usize::try_from(left.min(BUFFER as u64))?;
                    source.read_exact(&mut buffer[..count])?;
                    hash.update(&buffer[..count]);
                    target.write_all(&buffer[..count])?;
                    left -= count as u64;
                }
                ensure!(
                    format!("{:x}", hash.finalize()) == *sha256,
                    "The verified transfer file has changed"
                );
                target.sync_all()?;
                sync_dir(&parent)?;
            }
        }
    }
    Ok(())
}

fn verify_tree(root: &Dir, manifest: &Manifest, cancel: &AtomicBool) -> Result<()> {
    for entry in &manifest.entries {
        cancelled(cancel)?;
        let (parent, name) = path_parent(root, &entry.path, false)?;
        match &entry.kind {
            Kind::Directory => {
                directory(&parent, OsStr::new(&name), false)?;
            }
            Kind::File { size, sha256 } => {
                let mut file = open_file(&parent, &name, false, false)?;
                ensure!(
                    regular(&file, true)?.len() == *size
                        && finished_hash(&hash_prefix(&mut file, *size, cancel)?) == *sha256,
                    "The completed transfer file failed verification"
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "desk-switch-transfer-storage-{}-{}",
                std::process::id(),
                TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
        fn prepare(&self, paths: &[PathBuf]) -> Export {
            fs::create_dir_all(self.path("state")).unwrap();
            Export::prepare(
                &self.path("state"),
                "0123456789abcdef0123456789abcdef",
                paths,
                &AtomicBool::new(false),
                |_| {},
            )
            .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn clipboard_inventory_never_opens_file_contents_or_creates_transfer_storage() {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.path("source/empty")).unwrap();
        fs::write(fixture.path("source/data"), b"read only after paste").unwrap();
        let before = CONTENT_OPENS.with(std::cell::Cell::get);
        let selection = SourceSelection::capture(&[fixture.path("source")], &NO_CANCEL).unwrap();
        assert_eq!(CONTENT_OPENS.with(std::cell::Cell::get), before);
        assert!(!fixture.path("state").exists());
        assert_eq!(selection.description.total, 21);
        assert_eq!(selection.description.entries.len(), 3);
        assert!(selection.description.entries.iter().any(|entry| {
            entry.path == ["source", "empty"] && entry.kind == DescriptorKind::Directory
        }));
        let mut progress = Vec::new();
        let export = selection
            .prepare(
                &fixture.path("state"),
                "0123456789abcdef0123456789abcdef",
                MAX_TOTAL,
                &NO_CANCEL,
                |bytes| progress.push(bytes),
            )
            .unwrap();
        assert_eq!(export.manifest.description(), selection.description);
        assert_eq!(progress.last(), Some(&21));
        assert!(CONTENT_OPENS.with(std::cell::Cell::get) > before);
    }

    #[cfg(windows)]
    #[test]
    fn metadata_inventory_succeeds_when_windows_denies_content_opens() {
        use std::os::windows::fs::OpenOptionsExt;
        let fixture = Fixture::new();
        let path = fixture.path("source");
        fs::write(&path, b"content is locked").unwrap();
        let guard = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        let before = CONTENT_OPENS.with(std::cell::Cell::get);
        let selection = SourceSelection::capture(&[path], &NO_CANCEL).unwrap();
        assert_eq!(CONTENT_OPENS.with(std::cell::Cell::get), before);
        assert!(
            selection
                .prepare(
                    &fixture.path("state"),
                    "0123456789abcdef0123456789abcdef",
                    MAX_TOTAL,
                    &NO_CANCEL,
                    |_| {},
                )
                .is_err()
        );
        drop(guard);
        assert!(
            selection
                .prepare(
                    &fixture.path("state"),
                    "0123456789abcdef0123456789abcdef",
                    MAX_TOTAL,
                    &NO_CANCEL,
                    |_| {},
                )
                .is_ok()
        );
    }

    #[test]
    fn inventory_rejects_changed_or_replaced_files_before_reading_any_contents() {
        for replace in [false, true] {
            let fixture = Fixture::new();
            let path = fixture.path("source");
            fs::write(&path, b"original").unwrap();
            let selection =
                SourceSelection::capture(std::slice::from_ref(&path), &NO_CANCEL).unwrap();
            if replace {
                fs::rename(&path, fixture.path("old-source")).unwrap();
                fs::write(&path, b"original").unwrap();
            } else {
                fs::write(&path, b"changed length").unwrap();
            }
            let before = CONTENT_OPENS.with(std::cell::Cell::get);
            assert!(
                selection
                    .prepare(
                        &fixture.path("state"),
                        "0123456789abcdef0123456789abcdef",
                        MAX_TOTAL,
                        &NO_CANCEL,
                        |_| {},
                    )
                    .is_err()
            );
            assert_eq!(CONTENT_OPENS.with(std::cell::Cell::get), before);
            assert!(!fixture.path("state").exists());
        }
    }

    #[test]
    fn clipboard_capture_allows_local_folder_renames_and_rejects_rebound_sources() {
        for rename_parent in [false, true] {
            let fixture = Fixture::new();
            fs::create_dir_all(fixture.path("parent/source")).unwrap();
            fs::write(fixture.path("parent/source/data"), b"original").unwrap();
            let selection =
                SourceSelection::capture(&[fixture.path("parent/source")], &NO_CANCEL).unwrap();
            if rename_parent {
                fs::rename(fixture.path("parent"), fixture.path("old-parent")).unwrap();
                fs::create_dir_all(fixture.path("parent/source")).unwrap();
                fs::write(fixture.path("parent/source/data"), b"replacement").unwrap();
            } else {
                fs::rename(
                    fixture.path("parent/source"),
                    fixture.path("parent/old-source"),
                )
                .unwrap();
                fs::create_dir(fixture.path("parent/source")).unwrap();
                fs::write(fixture.path("parent/source/data"), b"replacement").unwrap();
            }
            assert!(
                selection
                    .prepare(
                        &fixture.path("state"),
                        "0123456789abcdef0123456789abcdef",
                        MAX_TOTAL,
                        &NO_CANCEL,
                        |_| {},
                    )
                    .is_err()
            );
            assert!(!fixture.path("state").exists());
        }
    }

    #[test]
    fn inventory_rejects_added_children_budget_changes_and_mutated_display_metadata() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.path("source")).unwrap();
        fs::write(fixture.path("source/data"), b"payload").unwrap();
        let selection = SourceSelection::capture(&[fixture.path("source")], &NO_CANCEL).unwrap();
        assert!(
            selection
                .prepare(
                    &fixture.path("state"),
                    "0123456789abcdef0123456789abcdef",
                    6,
                    &NO_CANCEL,
                    |_| {},
                )
                .is_err()
        );
        assert!(!fixture.path("state").exists());
        fs::write(fixture.path("source/new-data"), b"new").unwrap();
        assert!(
            selection
                .prepare(
                    &fixture.path("state"),
                    "0123456789abcdef0123456789abcdef",
                    MAX_TOTAL,
                    &NO_CANCEL,
                    |_| {},
                )
                .is_err()
        );
        let mut selection =
            SourceSelection::capture(&[fixture.path("source/data")], &NO_CANCEL).unwrap();
        selection.description.name = "different".into();
        assert!(
            selection
                .prepare(
                    &fixture.path("state"),
                    "0123456789abcdef0123456789abcdef",
                    MAX_TOTAL,
                    &NO_CANCEL,
                    |_| {},
                )
                .is_err()
        );
        assert!(!fixture.path("state").exists());
        assert!(
            SourceSelection::capture(&[fixture.path("source")], &AtomicBool::new(true)).is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn inventory_rejects_directory_links_and_special_files_without_content_opens() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let fixture = Fixture::new();
        fs::create_dir(fixture.path("source")).unwrap();
        fs::create_dir(fixture.path("outside")).unwrap();
        symlink(fixture.path("outside"), fixture.path("source/link")).unwrap();
        let before = CONTENT_OPENS.with(std::cell::Cell::get);
        assert!(SourceSelection::capture(&[fixture.path("source")], &NO_CANCEL).is_err());
        fs::remove_file(fixture.path("source/link")).unwrap();
        fs::write(fixture.path("source/unreadable"), b"private bytes").unwrap();
        fs::set_permissions(
            fixture.path("source/unreadable"),
            fs::Permissions::from_mode(0o0),
        )
        .unwrap();
        let selection = SourceSelection::capture(&[fixture.path("source")], &NO_CANCEL).unwrap();
        assert_eq!(selection.description.total, 13);
        assert_eq!(CONTENT_OPENS.with(std::cell::Cell::get), before);
        let fifo =
            std::ffi::CString::new(fixture.path("fifo").as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(SourceSelection::capture(&[fixture.path("fifo")], &NO_CANCEL).is_err());
        assert_eq!(CONTENT_OPENS.with(std::cell::Cell::get), before);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn directory_durability_reopens_a_capability_only_descriptor_for_real_fsync() {
        use std::os::fd::AsRawFd;
        let fixture = Fixture::new();
        let parent = absolute_directory(&fixture.0, false).unwrap();
        let child = directory(&parent, OsStr::new("durable"), true).unwrap();
        let flags = unsafe { libc::fcntl(child.as_raw_fd(), libc::F_GETFL) };
        assert!(
            flags >= 0 && flags & libc::O_PATH != 0,
            "The regression must exercise an actual O_PATH capability descriptor"
        );
        write_new(&child, "receipt.json", b"durably written").unwrap();
        sync_dir(&child).unwrap();
        drop(child);
        let reopened = directory(&parent, OsStr::new("durable"), false).unwrap();
        assert_eq!(
            open_file(&reopened, "receipt.json", false, false)
                .unwrap()
                .metadata()
                .unwrap()
                .len(),
            15
        );
        sync_dir(&reopened).unwrap();
    }
    fn copy(export: &Export, receive: &mut Receive) {
        for (index, entry) in export.manifest.entries.iter().enumerate() {
            if let Kind::File { size, .. } = entry.kind {
                let mut offset = receive.position(index as u32).unwrap().0;
                while offset < size {
                    let bytes = export.read(index as u32, offset, CHUNK).unwrap();
                    offset = receive
                        .write(index as u32, offset, &bytes, &digest(&bytes))
                        .unwrap();
                }
            }
        }
    }

    #[test]
    fn native_consumers_receive_readonly_verified_handles_after_completion_and_restart() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.path("source")).unwrap();
        fs::write(fixture.path("source/data"), b"payload").unwrap();
        let export = fixture.prepare(&[fixture.path("source")]);
        let index = export
            .manifest
            .entries
            .iter()
            .position(|entry| matches!(entry.kind, Kind::File { .. }))
            .unwrap();
        let downloads = fixture.path("downloads");
        let mut receiving =
            Receive::open(&downloads, export.manifest.clone(), "peer", "workspace").unwrap();
        assert!(receiving.open_verified(index).is_err());
        copy(&export, &mut receiving);
        receiving.finish().unwrap();
        assert!(receiving.open_verified(0).is_err());
        assert!(receiving.open_verified(usize::MAX).is_err());
        let mut file = receiving.open_verified(index).unwrap();
        assert!(file.write_all(b"forbidden").is_err());
        assert!(file.set_len(0).is_err());
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"payload");
        drop(file);
        drop(receiving);
        let receiving =
            Receive::open(&downloads, export.manifest.clone(), "peer", "workspace").unwrap();
        let mut file = receiving.open_verified(index).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"payload");
    }

    #[test]
    fn native_verified_open_cancels_before_open_and_between_hash_buffers() {
        let fixture = Fixture::new();
        let bytes = vec![0xd3; BUFFER * 3 + 17];
        fs::write(fixture.path("source"), &bytes).unwrap();
        let export = fixture.prepare(&[fixture.path("source")]);
        let mut receiving = Receive::open(
            &fixture.path("downloads"),
            export.manifest.clone(),
            "peer",
            "workspace",
        )
        .unwrap();
        copy(&export, &mut receiving);
        let roots = receiving.finish().unwrap();
        let cancel = std::sync::Arc::new(AtomicBool::new(true));
        let before_opens = CONTENT_OPENS.with(std::cell::Cell::get);
        assert!(receiving.open_verified_cancellable(0, &cancel).is_err());
        assert_eq!(CONTENT_OPENS.with(std::cell::Cell::get), before_opens);

        cancel.store(false, Ordering::Release);
        let before_buffers = HASH_BUFFERS.with(std::cell::Cell::get);
        let request_cancel = cancel.clone();
        AFTER_HASH_BUFFER.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                request_cancel.store(true, Ordering::Release);
            }));
        });
        let result = receiving.open_verified_cancellable(0, &cancel);
        // Clear the test hook before any assertion, including on a regression
        // that returns before reaching the first verification buffer.
        AFTER_HASH_BUFFER.with(|hook| {
            hook.borrow_mut().take();
        });
        assert!(cancel.load(Ordering::Acquire));
        assert!(result.is_err());
        assert_eq!(HASH_BUFFERS.with(std::cell::Cell::get), before_buffers + 1);
        assert_eq!(fs::read(&roots[0]).unwrap(), bytes);

        cancel.store(false, Ordering::Release);
        let mut file = receiving.open_verified_cancellable(0, &cancel).unwrap();
        assert!(file.write_all(b"forbidden").is_err());
        let mut verified = Vec::new();
        file.read_to_end(&mut verified).unwrap();
        assert_eq!(verified, bytes);
    }

    #[test]
    fn native_verified_open_rehashes_public_files_and_rejects_mutated_manifests() {
        let fixture = Fixture::new();
        fs::write(fixture.path("source"), b"payload").unwrap();
        let export = fixture.prepare(&[fixture.path("source")]);
        let mut receiving = Receive::open(
            &fixture.path("downloads"),
            export.manifest.clone(),
            "peer",
            "workspace",
        )
        .unwrap();
        copy(&export, &mut receiving);
        let paths = receiving.finish().unwrap();
        fs::write(&paths[0], b"changed").unwrap();
        assert!(receiving.open_verified(0).is_err());
        fs::write(&paths[0], b"payload").unwrap();
        assert!(receiving.open_verified(0).is_ok());
        receiving.manifest.entries[0].path = vec!["outside".into()];
        assert!(receiving.open_verified(0).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn native_verified_open_rejects_links_and_keeps_the_actual_finished_tree_pinned() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.path("source/nested")).unwrap();
        fs::write(fixture.path("source/nested/data"), b"payload").unwrap();
        fs::write(fixture.path("outside"), b"private").unwrap();
        let export = fixture.prepare(&[fixture.path("source")]);
        let index = export
            .manifest
            .entries
            .iter()
            .position(|entry| matches!(entry.kind, Kind::File { .. }))
            .unwrap();
        let mut receiving = Receive::open(
            &fixture.path("downloads"),
            export.manifest.clone(),
            "peer",
            "workspace",
        )
        .unwrap();
        copy(&export, &mut receiving);
        let roots = receiving.finish().unwrap();
        let data = roots[0].join("nested/data");
        fs::remove_file(&data).unwrap();
        symlink(fixture.path("outside"), &data).unwrap();
        assert!(receiving.open_verified(index).is_err());
        fs::remove_file(&data).unwrap();
        fs::write(&data, b"payload").unwrap();
        fs::rename(roots[0].join("nested"), roots[0].join("old-nested")).unwrap();
        symlink(roots[0].join("old-nested"), roots[0].join("nested")).unwrap();
        assert!(receiving.open_verified(index).is_err());
        fs::remove_file(roots[0].join("nested")).unwrap();
        fs::rename(roots[0].join("old-nested"), roots[0].join("nested")).unwrap();
        let files = roots[0].parent().unwrap();
        let pinned = files.with_file_name("original-files");
        fs::rename(files, &pinned).unwrap();
        fs::create_dir_all(files.join("source/nested")).unwrap();
        fs::write(files.join("source/nested/data"), b"private").unwrap();
        let mut file = receiving.open_verified(index).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"payload");
        assert_eq!(fs::read(fixture.path("outside")).unwrap(), b"private");
    }

    #[test]
    fn recursive_snapshots_preserve_empty_folders_and_never_read_changed_originals() {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.path("source/empty")).unwrap();
        fs::create_dir_all(fixture.path("source/nested")).unwrap();
        fs::write(
            fixture.path("source/nested/sample.txt"),
            b"original contents",
        )
        .unwrap();
        let export = fixture.prepare(&[fixture.path("source")]);
        fs::write(
            fixture.path("source/nested/sample.txt"),
            b"different later contents",
        )
        .unwrap();
        let mut receiving = Receive::open(
            &fixture.path("Downloads/Desk Switch"),
            export.manifest.clone(),
            "peer",
            "workspace",
        )
        .unwrap();
        copy(&export, &mut receiving);
        let roots = receiving.finish().unwrap();
        assert!(roots[0].join("empty").is_dir());
        assert_eq!(
            fs::read(roots[0].join("nested/sample.txt")).unwrap(),
            b"original contents"
        );
        assert_eq!(receiving.finish().unwrap(), roots);
    }
    #[test]
    fn receiver_survives_restart_checks_prefixes_and_does_not_overwrite_collisions() {
        let fixture = Fixture::new();
        let bytes = vec![123; CHUNK * 2 + 17];
        fs::write(fixture.path("source.bin"), &bytes).unwrap();
        let export = fixture.prepare(&[fixture.path("source.bin")]);
        let downloads = fixture.path("Downloads/Desk Switch");
        let manifest = export.manifest.clone();
        let mut receiving =
            Receive::open(&downloads, manifest.clone(), "peer", "workspace").unwrap();
        let first = export.read(0, 0, CHUNK).unwrap();
        receiving.write(0, 0, &first, &digest(&first)).unwrap();
        drop(receiving);
        drop(export);
        let export = Export::load(&fixture.path("state"), &manifest.id).unwrap();
        let mut receiving =
            Receive::open(&downloads, manifest.clone(), "peer", "workspace").unwrap();
        assert_eq!(
            receiving.position(0).unwrap(),
            (CHUNK as u64, digest(&first))
        );
        assert_eq!(export.prefix(0, CHUNK as u64).unwrap(), digest(&first));
        let collision = downloads.join(format!("Transfer {}", &manifest.id[..12]));
        fs::create_dir(&collision).unwrap();
        fs::write(collision.join("keep.txt"), b"never overwrite").unwrap();
        copy(&export, &mut receiving);
        let roots = receiving.finish().unwrap();
        assert_eq!(fs::read(&roots[0]).unwrap(), bytes);
        assert_eq!(
            fs::read(collision.join("keep.txt")).unwrap(),
            b"never overwrite"
        );
        assert!(roots[0].to_string_lossy().contains("(1)"));
    }
    #[test]
    fn corrupt_chunks_prefixes_and_rebound_sessions_are_rejected() {
        let fixture = Fixture::new();
        fs::write(fixture.path("source"), vec![1; CHUNK + 1]).unwrap();
        let export = fixture.prepare(&[fixture.path("source")]);
        let downloads = fixture.path("downloads");
        let mut receiving =
            Receive::open(&downloads, export.manifest.clone(), "peer", "workspace").unwrap();
        let bytes = export.read(0, 0, CHUNK).unwrap();
        assert!(receiving.write(0, 0, &bytes, "wrong").is_err());
        assert_eq!(receiving.position(0).unwrap().0, 0);
        receiving.write(0, 0, &bytes, &digest(&bytes)).unwrap();
        assert!(
            Receive::open(
                &downloads,
                export.manifest.clone(),
                "other-peer",
                "workspace"
            )
            .is_err()
        );
        assert!(
            Receive::open(
                &downloads,
                export.manifest.clone(),
                "peer",
                "other-workspace"
            )
            .is_err()
        );
        drop(receiving);
        let blob = downloads
            .join(format!(".partial-{}", export.manifest.id))
            .join("blobs/f00000000");
        fs::write(blob, vec![2; CHUNK]).unwrap();
        let mut receiving =
            Receive::open(&downloads, export.manifest.clone(), "peer", "workspace").unwrap();
        assert!(receiving.position(0).is_err());
    }
    #[test]
    fn cancellation_removes_only_owned_partials_and_cannot_be_resumed() {
        let fixture = Fixture::new();
        fs::write(fixture.path("source"), b"copy").unwrap();
        let export = fixture.prepare(&[fixture.path("source")]);
        let downloads = fixture.path("downloads");
        let mut receiving =
            Receive::open(&downloads, export.manifest.clone(), "peer", "workspace").unwrap();
        fs::write(downloads.join("keep.txt"), b"user data").unwrap();
        receiving.position(0).unwrap();
        receiving.cancel().unwrap();
        assert!(Receive::open(&downloads, export.manifest.clone(), "peer", "workspace").is_err());
        assert!(
            !downloads
                .join(format!(".partial-{}", export.manifest.id))
                .exists()
        );
        assert_eq!(fs::read(downloads.join("keep.txt")).unwrap(), b"user data");
    }
    #[test]
    fn unowned_partial_folders_and_unsafe_manifests_are_never_modified() {
        let fixture = Fixture::new();
        fs::write(fixture.path("source"), b"copy").unwrap();
        let export = fixture.prepare(&[fixture.path("source")]);
        let downloads = fixture.path("downloads");
        let partial = downloads.join(format!(".partial-{}", export.manifest.id));
        fs::create_dir_all(&partial).unwrap();
        fs::write(partial.join("keep.txt"), b"user data").unwrap();
        assert!(Receive::open(&downloads, export.manifest.clone(), "peer", "workspace").is_err());
        assert_eq!(fs::read(partial.join("keep.txt")).unwrap(), b"user data");
        let mut manifest = export.manifest.clone();
        manifest.entries[0].path = vec!["..".into(), "outside".into()];
        assert!(Receive::open(&downloads, manifest, "peer", "workspace").is_err());
    }
    #[test]
    fn duplicate_root_names_are_preserved_with_distinct_portable_names() {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.path("a")).unwrap();
        fs::create_dir_all(fixture.path("b")).unwrap();
        fs::write(fixture.path("a/same.txt"), b"first").unwrap();
        fs::write(fixture.path("b/same.txt"), b"second").unwrap();
        let export = fixture.prepare(&[fixture.path("a/same.txt"), fixture.path("b/same.txt")]);
        assert_eq!(export.manifest.roots, ["same.txt", "same.txt (2)"]);
        assert_eq!(export.read(0, 0, CHUNK).unwrap(), b"first");
        assert_eq!(export.read(1, 0, CHUNK).unwrap(), b"second");
    }
    #[test]
    fn uncommitted_bytes_are_trimmed_after_restart_and_empty_files_complete() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.path("source")).unwrap();
        fs::write(fixture.path("source/empty"), []).unwrap();
        fs::write(fixture.path("source/data"), vec![3; CHUNK + 1]).unwrap();
        let export = fixture.prepare(&[fixture.path("source")]);
        let index = export
            .manifest
            .entries
            .iter()
            .position(|entry| entry.path.last().map(String::as_str) == Some("data"))
            .unwrap() as u32;
        let downloads = fixture.path("downloads");
        let mut receiving =
            Receive::open(&downloads, export.manifest.clone(), "peer", "workspace").unwrap();
        let bytes = export.read(index, 0, CHUNK).unwrap();
        receiving.write(index, 0, &bytes, &digest(&bytes)).unwrap();
        drop(receiving);
        let blob = downloads
            .join(format!(".partial-{}", export.manifest.id))
            .join("blobs")
            .join(file_name(index));
        let mut dirty = fs::OpenOptions::new().append(true).open(&blob).unwrap();
        dirty.write_all(b"unacknowledged bytes").unwrap();
        drop(dirty);
        let mut receiving =
            Receive::open(&downloads, export.manifest.clone(), "peer", "workspace").unwrap();
        assert_eq!(receiving.position(index).unwrap().0, CHUNK as u64);
        assert_eq!(fs::metadata(&blob).unwrap().len(), CHUNK as u64);
        copy(&export, &mut receiving);
        let roots = receiving.finish().unwrap();
        assert_eq!(fs::metadata(roots[0].join("empty")).unwrap().len(), 0);
    }
    #[test]
    fn preparation_budget_cancellation_and_saved_manifest_tampering_fail_closed() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.path("state")).unwrap();
        fs::write(fixture.path("source"), b"bytes").unwrap();
        let id = "11111111111111111111111111111111";
        assert!(
            Export::prepare_with_limit(
                &fixture.path("state"),
                id,
                &[fixture.path("source")],
                4,
                &AtomicBool::new(false),
                |_| {}
            )
            .is_err()
        );
        assert!(
            !fixture
                .path(&format!("state/file-transfer/exports/{id}"))
                .exists()
        );
        assert!(
            Export::prepare(
                &fixture.path("state"),
                id,
                &[fixture.path("source")],
                &AtomicBool::new(true),
                |_| {}
            )
            .is_err()
        );
        assert!(
            !fixture
                .path(&format!("state/file-transfer/exports/{id}"))
                .exists()
        );
        let export = fixture.prepare(&[fixture.path("source")]);
        let id = export.manifest.id.clone();
        drop(export);
        let file = fixture.path(&format!("state/file-transfer/exports/{id}/manifest.json"));
        let mut manifest: Manifest = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        manifest.name = "different".into();
        fs::write(file, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(Export::load(&fixture.path("state"), &id).is_err());
    }
    #[test]
    fn final_file_hash_is_checked_and_cancellation_cannot_delete_completed_data() {
        let fixture = Fixture::new();
        fs::write(fixture.path("source"), b"payload").unwrap();
        let export = fixture.prepare(&[fixture.path("source")]);
        let downloads = fixture.path("downloads");
        let mut receiving =
            Receive::open(&downloads, export.manifest.clone(), "peer", "workspace").unwrap();
        let wrong = b"changed";
        assert!(receiving.write(0, 0, wrong, &digest(wrong)).is_err());
        assert_eq!(receiving.position(0).unwrap().0, 0);
        copy(&export, &mut receiving);
        assert!(
            receiving
                .finish_with_cancel(&AtomicBool::new(true))
                .is_err()
        );
        let roots = receiving.finish().unwrap();
        assert!(receiving.cancel().is_err());
        assert_eq!(fs::read(&roots[0]).unwrap(), b"payload");
    }
    #[test]
    fn interrupted_cancellation_cleans_up_before_and_after_reserved_folder_removal() {
        let fixture = Fixture::new();
        fs::write(fixture.path("source"), b"payload").unwrap();
        let export = fixture.prepare(&[fixture.path("source")]);
        for (phase, tombstone, saved_cancelled, removed_bundle) in [
            (0, false, false, true),
            (1, true, false, false),
            (2, true, true, true),
        ] {
            let downloads = fixture.path(&format!("downloads-{phase}"));
            let mut receiving =
                Receive::open(&downloads, export.manifest.clone(), "peer", "workspace").unwrap();
            receiving.position(0).unwrap();
            let (_, bundle) = receiving.bundle().unwrap();
            if tombstone {
                write_new(
                    &receiving.base,
                    &format!(".cancelled-{}.json", export.manifest.id),
                    &serde_json::to_vec(&receiving.owner.binding).unwrap(),
                )
                .unwrap();
            }
            if saved_cancelled {
                receiving.owner.cancelled = true;
                receiving.persist().unwrap();
            }
            if removed_bundle {
                bundle.remove_open_dir_all().unwrap();
            } else {
                drop(bundle);
            }
            drop(receiving);
            if tombstone {
                Receive::cleanup_cancelled(&downloads, &export.manifest, "peer", "workspace")
                    .unwrap();
            } else {
                let mut receiving =
                    Receive::open(&downloads, export.manifest.clone(), "peer", "workspace")
                        .unwrap();
                receiving.cancel().unwrap();
            }
            assert!(
                !downloads
                    .join(format!(".partial-{}", export.manifest.id))
                    .exists()
            );
            assert!(
                Receive::open(&downloads, export.manifest.clone(), "peer", "workspace").is_err()
            );
        }
    }
    #[test]
    fn atomic_finalization_recovers_when_the_completion_receipt_was_not_written() {
        let fixture = Fixture::new();
        fs::write(fixture.path("source"), b"payload").unwrap();
        let export = fixture.prepare(&[fixture.path("source")]);
        let downloads = fixture.path("downloads");
        let mut receiving =
            Receive::open(&downloads, export.manifest.clone(), "peer", "workspace").unwrap();
        copy(&export, &mut receiving);
        let (name, bundle) = receiving.bundle().unwrap();
        let (stage, blobs) = receiving.active().unwrap();
        let payload = directory(stage, OsStr::new("payload"), true).unwrap();
        materialize(&payload, blobs, &export.manifest, &NO_CANCEL).unwrap();
        drop(payload);
        stage.rename("payload", &bundle, "files").unwrap();
        drop(bundle);
        drop(receiving);
        let mut receiving =
            Receive::open(&downloads, export.manifest.clone(), "peer", "workspace").unwrap();
        let roots = receiving.finish().unwrap();
        assert_eq!(roots, vec![downloads.join(name).join("files/source")]);
        assert_eq!(fs::read(&roots[0]).unwrap(), b"payload");
        assert!(receiving.is_complete());
    }
    #[test]
    fn a_source_modified_during_copy_is_rejected_and_its_partial_export_is_removed() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.path("state")).unwrap();
        let source = fixture.path("source");
        fs::write(&source, vec![7; BUFFER * 3]).unwrap();
        let id = "11111111111111111111111111111111";
        let mut changed = false;
        let result = Export::prepare(
            &fixture.path("state"),
            id,
            std::slice::from_ref(&source),
            &NO_CANCEL,
            |_| {
                if !changed {
                    changed = true;
                    fs::write(&source, b"changed during snapshot").unwrap();
                }
            },
        );
        assert!(result.is_err());
        assert!(
            !fixture
                .path(&format!("state/file-transfer/exports/{id}"))
                .exists()
        );
    }
    #[test]
    fn terminal_export_cleanup_checks_ownership_without_requiring_its_data_files() {
        let fixture = Fixture::new();
        fs::write(fixture.path("source"), b"keep original").unwrap();
        let export = fixture.prepare(&[fixture.path("source")]);
        let id = export.manifest.id.clone();
        drop(export);
        let root = fixture.path(&format!("state/file-transfer/exports/{id}"));
        fs::remove_file(root.join("blobs/f00000000")).unwrap();
        assert!(Export::load(&fixture.path("state"), &id).is_err());
        Export::discard(&fixture.path("state"), &id).unwrap();
        Export::discard(&fixture.path("state"), &id).unwrap();
        assert!(!root.exists());
        assert_eq!(fs::read(fixture.path("source")).unwrap(), b"keep original");
        let unowned = fixture.path("state/file-transfer/exports/11111111111111111111111111111111");
        fs::create_dir(&unowned).unwrap();
        fs::write(unowned.join("keep.txt"), b"unrelated").unwrap();
        assert!(
            Export::discard(&fixture.path("state"), "11111111111111111111111111111111").is_err()
        );
        assert_eq!(fs::read(unowned.join("keep.txt")).unwrap(), b"unrelated");
    }
    #[test]
    fn unknown_export_cancellation_is_durable_and_bound_to_workspace_and_pinned_peer() {
        let fixture = Fixture::new();
        let state = fixture.path("state");
        let peer = "ab".repeat(32);
        let other_peer = "cd".repeat(32);
        let id = "11".repeat(16);
        assert!(!was_cancelled(&state, "workspace-a", &peer, &id).unwrap());
        assert!(!state.exists());
        remember_cancel(&state, "workspace-a", &peer, &id).unwrap();
        remember_cancel(&state, "workspace-a", &peer, &id).unwrap();
        assert!(was_cancelled(&state, "workspace-a", &peer, &id).unwrap());
        assert!(!was_cancelled(&state, "workspace-b", &peer, &id).unwrap());
        assert!(!was_cancelled(&state, "workspace-a", &other_peer, &id).unwrap());
        assert!(!was_cancelled(&state, "workspace-a", &peer, &"22".repeat(16)).unwrap());
        assert!(!state.join("file-transfer/exports").exists());
        assert_eq!(
            fs::read_dir(state.join("file-transfer/cancellations"))
                .unwrap()
                .count(),
            1
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(state.join("file-transfer/cancellations"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o077,
                0
            );
        }
    }
    #[test]
    fn cancellation_capacity_never_evicts_or_silently_acknowledges_another_export() {
        let fixture = Fixture::new();
        let state = fixture.path("state");
        let peer = "ab".repeat(32);
        for id in ["11".repeat(16), "22".repeat(16)] {
            remember_cancel_with_limit(&state, "workspace", &peer, &id, 2).unwrap();
        }
        assert!(
            remember_cancel_with_limit(&state, "workspace", &peer, &"33".repeat(16), 2).is_err()
        );
        remember_cancel_with_limit(&state, "workspace", &peer, &"11".repeat(16), 2).unwrap();
        assert!(was_cancelled(&state, "workspace", &peer, &"11".repeat(16)).unwrap());
        assert!(was_cancelled(&state, "workspace", &peer, &"22".repeat(16)).unwrap());
        assert!(!was_cancelled(&state, "workspace", &peer, &"33".repeat(16)).unwrap());
    }
    #[test]
    fn cancellation_binding_corruption_and_oversized_metadata_fail_closed_without_overwrite() {
        let fixture = Fixture::new();
        let state = fixture.path("state");
        let peer = "ab".repeat(32);
        let id = "11".repeat(16);
        remember_cancel(&state, "workspace", &peer, &id).unwrap();
        let (mut binding, name) = cancellation("workspace", &peer, &id).unwrap();
        binding.workspace = "different-workspace".into();
        let metadata = state.join("file-transfer/cancellations").join(name);
        let altered = serde_json::to_vec(&binding).unwrap();
        fs::write(&metadata, &altered).unwrap();
        assert!(was_cancelled(&state, "workspace", &peer, &id).is_err());
        assert!(remember_cancel(&state, "workspace", &peer, &id).is_err());
        assert_eq!(fs::read(&metadata).unwrap(), altered);
        fs::write(&metadata, vec![b' '; CANCELLATION_LIMIT + 1]).unwrap();
        assert!(was_cancelled(&state, "workspace", &peer, &id).is_err());
        assert!(remember_cancel(&state, "workspace", &peer, &id).is_err());
    }
    #[test]
    fn interrupted_preparation_cleanup_requires_its_exact_durable_owner_marker() {
        let fixture = Fixture::new();
        let state_path = fixture.path("state");
        let state = absolute_directory(&state_path, true).unwrap();
        let transfer = directory(&state, OsStr::new("file-transfer"), true).unwrap();
        let exports = directory(&transfer, OsStr::new("exports"), true).unwrap();
        for (id, valid_marker) in [
            ("11111111111111111111111111111111", true),
            ("22222222222222222222222222222222", false),
        ] {
            let root = directory(&exports, OsStr::new(id), true).unwrap();
            write_new(
                &root,
                "preparing.json",
                &serde_json::to_vec(&PreparationOwner {
                    format: FORMAT,
                    id: if valid_marker {
                        id.into()
                    } else {
                        "33333333333333333333333333333333".into()
                    },
                    purpose: "desk-switch-export-preparation".into(),
                })
                .unwrap(),
            )
            .unwrap();
            let blobs = directory(&root, OsStr::new("blobs"), true).unwrap();
            write_new(&blobs, "unfinished", b"partly copied bytes").unwrap();
            drop(blobs);
            drop(root);
            let result = Export::discard(&state_path, id);
            assert_eq!(result.is_ok(), valid_marker);
            assert_eq!(
                fixture
                    .path(&format!("state/file-transfer/exports/{id}"))
                    .exists(),
                !valid_marker
            );
        }
        let unowned_id = "44444444444444444444444444444444";
        let root = directory(&exports, OsStr::new(unowned_id), true).unwrap();
        write_new(&root, "keep.txt", b"not our incomplete export").unwrap();
        drop(root);
        assert!(Export::discard(&state_path, unowned_id).is_err());
        assert_eq!(
            fs::read(fixture.path(&format!(
                "state/file-transfer/exports/{unowned_id}/keep.txt"
            )))
            .unwrap(),
            b"not our incomplete export"
        );
    }
    #[test]
    fn native_cache_removal_deletes_its_private_stage() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.path("source")).unwrap();
        fs::write(fixture.path("source/data"), b"cached").unwrap();
        let export = fixture.prepare(&[fixture.path("source/data")]);
        let downloads = fixture.path("downloads");
        let stage = downloads.join(format!(".partial-{}", export.manifest.id));
        let mut receive =
            Receive::open(&downloads, export.manifest.clone(), "peer", "workspace").unwrap();
        assert!(stage.exists());
        receive.remove_cache().unwrap();
        assert!(!stage.exists());
        assert!(
            !downloads
                .join(format!("Transfer {}", &export.manifest.id[..12]))
                .exists()
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_sources_ancestors_and_checkpoint_hardlinks_cannot_escape() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let fixture = Fixture::new();
        fs::write(fixture.path("outside"), b"private").unwrap();
        fs::create_dir(fixture.path("source")).unwrap();
        symlink(fixture.path("outside"), fixture.path("source/link")).unwrap();
        fs::create_dir(fixture.path("state")).unwrap();
        let cancel = AtomicBool::new(false);
        assert!(
            Export::prepare(
                &fixture.path("state"),
                "11111111111111111111111111111111",
                &[fixture.path("source")],
                &cancel,
                |_| {}
            )
            .is_err()
        );
        symlink(fixture.path("source"), fixture.path("alias")).unwrap();
        assert!(absolute_directory(&fixture.path("alias"), false).is_err());
        fs::remove_file(fixture.path("source/link")).unwrap();
        fs::write(fixture.path("source/data"), b"safe").unwrap();
        let export = fixture.prepare(&[fixture.path("source/data")]);
        let downloads = fixture.path("downloads");
        let mut receiving =
            Receive::open(&downloads, export.manifest.clone(), "peer", "workspace").unwrap();
        let blobs = downloads
            .join(format!(".partial-{}", export.manifest.id))
            .join("blobs");
        fs::hard_link(fixture.path("outside"), blobs.join("f00000000")).unwrap();
        assert!(receiving.position(0).is_err());
        assert_eq!(fs::read(fixture.path("outside")).unwrap(), b"private");
        let _ = fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o700));
    }
}
