//! Handle-relative storage. Peer paths never become ambient filesystem paths.
use super::{
    safe,
    sync::{self, Change, Manifest, Node, Operation},
};
use crate::group::file_transfer::storage::{
    self as confined, SourceMetadata, source_move::move_no_replace,
};
use anyhow::{Context, Result, bail, ensure};
use cap_std::fs::{Dir, File};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

pub struct Root {
    dir: Dir,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Journal {
    operation: Operation,
    applied: bool,
    #[serde(default)]
    saved_at: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Version {
    pub id: String,
    pub path: String,
    pub value: Node,
    pub saved_at: u64,
}

fn interrupted(cancel: &AtomicBool) -> Result<()> {
    ensure!(
        !cancel.load(Ordering::Acquire),
        "Sync paused; resume to continue"
    );
    Ok(())
}
fn missing(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|v| v.kind() == std::io::ErrorKind::NotFound)
}
fn parent(root: &Dir, path: &str) -> Result<(Dir, String)> {
    sync::path(path)?;
    let mut parts = path.split('/').collect::<Vec<_>>();
    let name = parts.pop().context("Missing sync filename")?.to_string();
    let mut current = root.try_clone()?;
    for part in parts {
        current = confined::directory(&current, OsStr::new(part), false)?;
    }
    Ok((current, name))
}
fn fingerprint(dir: &Dir, name: &str, cancel: &AtomicBool) -> Result<Option<Node>> {
    interrupted(cancel)?;
    let metadata = match dir.symlink_metadata(name) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    confined::no_links(&metadata)?;
    if metadata.is_dir() {
        return Ok(Some(Node::Directory));
    }
    let before = SourceMetadata::of(&metadata)?;
    let mut file = confined::open_file(dir, name, false, false)?;
    before.check(&file.metadata()?)?;
    let length = metadata.len();
    ensure!(length <= safe::MAX_TOTAL, "Sync file exceeds size limit");
    let hash = prefix(&mut file, length, cancel)?;
    let mut extra = [0];
    ensure!(
        file.read(&mut extra)? == 0,
        "Sync source changed; compare again"
    );
    before.check(&file.metadata()?)?;
    before.check(&dir.symlink_metadata(name)?)?;
    Ok(Some(Node::File {
        size: length,
        sha256: hash,
    }))
}
fn prefix(file: &mut File, length: u64, cancel: &AtomicBool) -> Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut left = length;
    let mut bytes = [0; safe::CHUNK];
    while left != 0 {
        interrupted(cancel)?;
        let count = left.min(bytes.len() as u64) as usize;
        file.read_exact(&mut bytes[..count])?;
        hash.update(&bytes[..count]);
        left -= count as u64;
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn move_checked(from: &Dir, source: &str, to: &Dir, target: &str) -> Result<()> {
    move_no_replace(from, OsStr::new(source), to, OsStr::new(target))
        .with_context(|| format!("Moving sync entry {source} to {target} without replacement"))?;
    confined::sync_dir(from)?;
    confined::sync_dir(to)
}

impl Root {
    pub fn open(path: &Path) -> Result<Self> {
        ensure!(
            path.parent().is_some(),
            "Choose a folder, not a volume root"
        );
        Ok(Self {
            dir: confined::absolute_directory(path, false)?,
        })
    }
    fn meta(&self) -> Result<Dir> {
        confined::directory(&self.dir, OsStr::new(sync::META), true)
    }
    fn area(&self, name: &str) -> Result<Dir> {
        confined::directory(&self.meta()?, OsStr::new(name), true)
    }
    pub fn manifest(&self, cancel: &AtomicBool) -> Result<Manifest> {
        fn walk(
            dir: &Dir,
            base: &str,
            result: &mut Manifest,
            cancel: &AtomicBool,
            total: &mut u64,
        ) -> Result<()> {
            for entry in dir.entries()? {
                interrupted(cancel)?;
                let name = entry?
                    .file_name()
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("Sync paths must be valid UTF-8"))?;
                if base.is_empty() && name.eq_ignore_ascii_case(sync::META) {
                    continue;
                }
                let path = if base.is_empty() {
                    name.clone()
                } else {
                    format!("{base}/{name}")
                };
                sync::path(&path)?;
                let node = fingerprint(dir, &name, cancel)?
                    .context("Sync folder changed; compare again")?;
                *total = total
                    .checked_add(node.size())
                    .filter(|v| *v <= safe::MAX_TOTAL)
                    .context("Sync folder exceeds size limit")?;
                ensure!(result.len() < safe::MAX_ENTRIES, "Too many sync entries");
                let directory = node == Node::Directory;
                result.insert(path.clone(), node);
                if directory {
                    walk(
                        &confined::directory(dir, OsStr::new(&name), false)?,
                        &path,
                        result,
                        cancel,
                        total,
                    )?;
                }
            }
            Ok(())
        }
        let mut result = Manifest::new();
        walk(&self.dir, "", &mut result, cancel, &mut 0)?;
        sync::validate(&result)?;
        Ok(result)
    }
    fn node(&self, path: &str, cancel: &AtomicBool) -> Result<Option<Node>> {
        match parent(&self.dir, path) {
            Ok((dir, name)) => fingerprint(&dir, &name, cancel),
            Err(error) if missing(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }
    pub fn read(
        &self,
        path: &str,
        value: &Node,
        offset: u64,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>> {
        let Node::File { size, .. } = value else {
            bail!("Not a sync file");
        };
        ensure!(offset <= *size, "Invalid sync read offset");
        let (dir, name) = parent(&self.dir, path)?;
        let mut file = confined::open_file(&dir, &name, false, false)?;
        ensure!(
            file.metadata()?.len() == *size,
            "Sync source changed; compare again"
        );
        interrupted(cancel)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; (*size - offset).min(safe::CHUNK as u64) as usize];
        file.read_exact(&mut bytes)?;
        Ok(bytes)
    }
    pub fn verify_source(&self, path: &str, value: &Node, cancel: &AtomicBool) -> Result<()> {
        ensure!(
            self.node(path, cancel)?.as_ref() == Some(value),
            "Sync source changed; compare again"
        );
        Ok(())
    }
    /// Returns a durable prefix. The sender must independently verify its hash.
    pub fn begin(
        &self,
        id: &str,
        op: &Operation,
        cancel: &AtomicBool,
    ) -> Result<(bool, u64, String)> {
        ensure!(safe::valid_hash(id), "Invalid sync operation identity");
        op.validate()?;
        let journals = self.area("journal")?;
        if confined::exists(&journals, id)? {
            let journal: Journal = confined::read_json(&journals, id, 32 * 1024)?;
            ensure!(
                journal.operation == *op,
                "Sync operation identity was reused"
            );
            if journal.applied {
                self.verify_applied(op, cancel)?;
                return Ok((true, 0, safe::hash(&[])));
            }
            if self.verify_applied(op, cancel).is_ok()
                && (op.before.is_none() || confined::exists(&self.area("versions")?, id)?)
            {
                self.finish(id, op, cancel)?;
                return Ok((true, 0, safe::hash(&[])));
            }
        } else {
            ensure!(
                self.node(&op.path, cancel)? == op.before,
                "Sync destination changed; compare again"
            );
            ensure!(
                journals.entries()?.take(50_001).count() < 50_000,
                "Sync history is full; recover or archive versions first"
            );
            confined::write_json(
                &journals,
                id,
                &Journal {
                    operation: op.clone(),
                    applied: false,
                    saved_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)?
                        .as_secs(),
                },
            )?;
        }
        if let Change::Set {
            value: Node::File { size, .. },
        } = &op.change
        {
            let stages = self.area("partial")?;
            let mut file = if confined::exists(&stages, id)? {
                confined::open_file(&stages, id, true, false)?
            } else {
                confined::open_file(&stages, id, true, true)?
            };
            let offset = file.metadata()?.len();
            ensure!(offset <= *size, "Sync partial is damaged");
            Ok((false, offset, prefix(&mut file, offset, cancel)?))
        } else {
            Ok((false, 0, safe::hash(&[])))
        }
    }
    pub fn chunk(
        &self,
        id: &str,
        op: &Operation,
        offset: u64,
        bytes: &[u8],
        cancel: &AtomicBool,
    ) -> Result<u64> {
        interrupted(cancel)?;
        ensure!(safe::valid_hash(id), "Invalid sync operation identity");
        let Change::Set {
            value: Node::File { size, .. },
        } = &op.change
        else {
            bail!("Not a sync upload");
        };
        ensure!(
            !bytes.is_empty()
                && bytes.len() <= safe::CHUNK
                && offset
                    .checked_add(bytes.len() as u64)
                    .is_some_and(|v| v <= *size),
            "Invalid sync chunk"
        );
        let journal: Journal = confined::read_json(&self.area("journal")?, id, 32 * 1024)?;
        ensure!(
            !journal.applied && journal.operation == *op,
            "Unknown sync upload"
        );
        let mut file = confined::open_file(&self.area("partial")?, id, true, false)?;
        let length = file.metadata()?.len();
        ensure!(offset <= length, "Reordered sync chunk");
        if offset < length {
            ensure!(
                offset + bytes.len() as u64 <= length,
                "Overlapping sync chunk"
            );
            file.seek(SeekFrom::Start(offset))?;
            let mut previous = vec![0; bytes.len()];
            file.read_exact(&mut previous)?;
            ensure!(previous == bytes, "Duplicate sync chunk differs");
            return Ok(length);
        }
        file.seek(SeekFrom::Start(length))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(length + bytes.len() as u64)
    }
    fn verify_applied(&self, op: &Operation, cancel: &AtomicBool) -> Result<()> {
        ensure!(
            self.node(&op.path, cancel)?.as_ref() == op.after(),
            "A completed sync destination changed; compare again"
        );
        if let Change::Rename { from, .. } = &op.change {
            ensure!(
                self.node(from, cancel)?.is_none(),
                "A renamed sync source reappeared"
            );
        }
        Ok(())
    }
    pub fn finish(&self, id: &str, op: &Operation, cancel: &AtomicBool) -> Result<()> {
        ensure!(safe::valid_hash(id), "Invalid sync operation identity");
        let journals = self.area("journal")?;
        let mut journal: Journal = confined::read_json(&journals, id, 32 * 1024)?;
        ensure!(journal.operation == *op, "Unknown sync operation");
        if journal.applied {
            return self.verify_applied(op, cancel);
        }
        let (target, name) = parent(&self.dir, &op.path)?;
        if let Change::Rename { from, value } = &op.change {
            if self.node(&op.path, cancel)?.as_ref() != Some(value)
                || self.node(from, cancel)?.is_some()
            {
                self.verify_source(from, value, cancel)?;
                ensure!(
                    self.node(&op.path, cancel)?.is_none(),
                    "Sync rename destination changed"
                );
                let (source, old) = parent(&self.dir, from)?;
                move_checked(&source, &old, &target, &name)?;
            }
        } else {
            let versions = self.area("versions")?;
            let stages = self.area("partial")?;
            // Verify the entire new file before touching the existing destination.
            if let Change::Set {
                value: value @ Node::File { .. },
            } = &op.change
                && fingerprint(&stages, id, cancel)?.as_ref() != Some(value)
            {
                // Crash after install, before the receipt: no stage remains.
                ensure!(
                    !confined::exists(&stages, id)?
                        && self.node(&op.path, cancel)?.as_ref() == Some(value),
                    "Sync file failed verification; compare again"
                );
            }
            if let Some(before) = &op.before {
                if !confined::exists(&versions, id)? {
                    ensure!(
                        self.node(&op.path, cancel)?.as_ref() == Some(before),
                        "Sync destination changed; compare again"
                    );
                    if before == &Node::Directory {
                        ensure!(
                            confined::directory(&target, OsStr::new(&name), false)?
                                .entries()?
                                .next()
                                .is_none(),
                            "Sync directory still contains files"
                        );
                    }
                    move_checked(&target, &name, &versions, id)?;
                }
                ensure!(
                    fingerprint(&versions, id, cancel)?.as_ref() == Some(before),
                    "Sync recovery version changed"
                );
            }
            match &op.change {
                Change::Set {
                    value: Node::Directory,
                } => {
                    if self.node(&op.path, cancel)?.is_none() {
                        target.create_dir(&name)?;
                        confined::sync_dir(&target)?;
                    }
                }
                Change::Set { value } => {
                    if self.node(&op.path, cancel)?.as_ref() != Some(value) {
                        ensure!(
                            self.node(&op.path, cancel)?.is_none(),
                            "Sync target collision; previous version is recoverable"
                        );
                        move_checked(&stages, id, &target, &name)?;
                    }
                }
                Change::Delete => ensure!(
                    self.node(&op.path, cancel)?.is_none(),
                    "Sync deletion destination reappeared"
                ),
                Change::Rename { .. } => unreachable!(),
            }
        }
        self.verify_applied(op, cancel)?;
        journal.applied = true;
        confined::write_json(&journals, id, &journal)
    }
    pub fn source_prefix(&self, path: &str, offset: u64, cancel: &AtomicBool) -> Result<String> {
        let (dir, name) = parent(&self.dir, path)?;
        prefix(
            &mut confined::open_file(&dir, &name, false, false)?,
            offset,
            cancel,
        )
    }
    pub fn versions(&self, cancel: &AtomicBool) -> Result<Vec<Version>> {
        let versions = self.area("versions")?;
        let journals = self.area("journal")?;
        let mut result = Vec::new();
        for entry in versions.entries()? {
            let id = entry?
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("Invalid sync history"))?;
            ensure!(safe::valid_hash(&id), "Invalid sync version identity");
            let journal: Journal = confined::read_json(&journals, &id, 32 * 1024)?;
            let value = fingerprint(&versions, &id, cancel)?.context("Sync version disappeared")?;
            ensure!(
                journal.operation.before.as_ref() == Some(&value),
                "Sync version failed verification"
            );
            result.push(Version {
                id,
                path: journal.operation.path,
                value,
                saved_at: journal.saved_at,
            });
        }
        Ok(result)
    }
    /// Restore through the same journal, keeping the displaced current version.
    pub fn restore(&self, version: &Version, restore_id: &str, cancel: &AtomicBool) -> Result<()> {
        let actual = self
            .versions(cancel)?
            .into_iter()
            .find(|v| v.id == version.id)
            .context("Unknown sync version")?;
        ensure!(
            actual.path == version.path && actual.value == version.value,
            "Sync version changed"
        );
        ensure!(
            matches!(actual.value, Node::File { .. }),
            "Only file versions can be restored directly"
        );
        let op = Operation {
            side: sync::Side::Local,
            path: actual.path,
            before: self.node(&version.path, cancel)?,
            change: Change::Set {
                value: actual.value.clone(),
            },
        };
        let (_, mut offset, _) = self.begin(restore_id, &op, cancel)?;
        let mut file = confined::open_file(&self.area("versions")?, &version.id, false, false)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = [0; safe::CHUNK];
        loop {
            let count = file.read(&mut bytes)?;
            if count == 0 {
                break;
            }
            offset = self.chunk(restore_id, &op, offset, &bytes[..count], cancel)?;
        }
        self.finish(restore_id, &op, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::group::resources::settings;
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::current_dir()
                .unwrap()
                .join("target/resource-fixtures")
                .join(settings::new_id().unwrap());
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn open(&self) -> Root {
            Root::open(&self.0).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let intended = std::env::current_dir()
                .unwrap()
                .join("target/resource-fixtures");
            assert!(
                self.0.is_absolute()
                    && self.0.starts_with(intended)
                    && self.0.file_name().unwrap().len() == 32
            );
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn node(bytes: &[u8]) -> Node {
        Node::File {
            size: bytes.len() as u64,
            sha256: safe::hash(bytes),
        }
    }
    fn operation(before: Option<Node>, after: &[u8]) -> Operation {
        Operation {
            side: sync::Side::Local,
            path: "document".into(),
            before,
            change: Change::Set { value: node(after) },
        }
    }
    #[test]
    fn partial_chunks_survive_reopen_and_validate_duplicate_offsets_without_replacing_files() {
        let fixture = Fixture::new();
        std::fs::write(fixture.0.join("document"), b"before").unwrap();
        let cancel = AtomicBool::new(false);
        let id = safe::hash(b"interrupted staged content");
        let op = operation(Some(node(b"before")), b"updated document");
        {
            let root = fixture.open();
            assert_eq!(root.begin(&id, &op, &cancel).unwrap().1, 0);
            assert!(root.chunk(&id, &op, 4, b"x", &cancel).is_err());
            assert_eq!(root.chunk(&id, &op, 0, b"updated ", &cancel).unwrap(), 8);
            assert_eq!(root.chunk(&id, &op, 0, b"updated ", &cancel).unwrap(), 8);
            assert!(root.chunk(&id, &op, 0, b"corrupt ", &cancel).is_err());
        }
        let root = fixture.open();
        let (_, offset, prefix) = root.begin(&id, &op, &cancel).unwrap();
        assert_eq!(offset, 8);
        assert_eq!(prefix, safe::hash(b"updated "));
        assert!(
            root.chunk(&id, &op, offset + 1, b"document", &cancel)
                .is_err()
        );
        assert!(root.finish(&id, &op, &cancel).is_err());
        assert_eq!(
            std::fs::read(fixture.0.join("document")).unwrap(),
            b"before"
        );
        cancel.store(true, Ordering::Release);
        assert!(root.chunk(&id, &op, offset, b"document", &cancel).is_err());
        cancel.store(false, Ordering::Release);
        assert_eq!(root.begin(&id, &op, &cancel).unwrap().1, 8);
        // Reusing a job identity for different intent must not adopt its bytes.
        let another = operation(Some(node(b"before")), b"another document");
        assert!(root.begin(&id, &another, &cancel).is_err());
    }
    #[test]
    fn chunks_resume_after_reopen_reject_reordering_and_restore_previous_versions() {
        let fixture = Fixture::new();
        std::fs::write(fixture.0.join("document"), b"before").unwrap();
        let cancel = AtomicBool::new(false);
        let id = safe::hash(b"resume");
        let op = operation(Some(node(b"before")), b"updated document");
        {
            let root = fixture.open();
            assert_eq!(root.begin(&id, &op, &cancel).unwrap().1, 0);
            assert!(root.chunk(&id, &op, 4, b"x", &cancel).is_err());
            assert_eq!(root.chunk(&id, &op, 0, b"updated ", &cancel).unwrap(), 8);
            assert_eq!(root.chunk(&id, &op, 0, b"updated ", &cancel).unwrap(), 8);
            assert!(root.chunk(&id, &op, 0, b"corrupt ", &cancel).is_err());
            assert!(root.finish(&id, &op, &cancel).is_err());
            assert_eq!(
                std::fs::read(fixture.0.join("document")).unwrap(),
                b"before"
            );
        }
        let root = fixture.open();
        let (_, offset, prefix) = root.begin(&id, &op, &cancel).unwrap();
        assert_eq!(offset, 8);
        assert_eq!(prefix, safe::hash(b"updated "));
        root.chunk(&id, &op, offset, b"document", &cancel).unwrap();
        root.finish(&id, &op, &cancel).unwrap();
        assert!(root.begin(&id, &op, &cancel).unwrap().0);
        root.finish(&id, &op, &cancel).unwrap();
        assert_eq!(
            std::fs::read(fixture.0.join("document")).unwrap(),
            b"updated document"
        );
        let versions = root.versions(&cancel).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].value, node(b"before"));
        root.restore(&versions[0], &safe::hash(b"restore"), &cancel)
            .unwrap();
        assert_eq!(
            std::fs::read(fixture.0.join("document")).unwrap(),
            b"before"
        );
        assert_eq!(root.versions(&cancel).unwrap().len(), 2);
    }
    #[test]
    fn crash_between_backup_install_and_receipt_is_recoverable_and_no_collision_is_overwritten() {
        let fixture = Fixture::new();
        let cancel = AtomicBool::new(false);
        let root = fixture.open();
        std::fs::write(fixture.0.join("document"), b"old").unwrap();
        let op = operation(Some(node(b"old")), b"new");
        let id = safe::hash(b"crash");
        root.begin(&id, &op, &cancel).unwrap();
        root.chunk(&id, &op, 0, b"new", &cancel).unwrap();
        let versions = root.area("versions").unwrap();
        move_checked(&root.dir, "document", &versions, &id).unwrap();
        std::fs::write(fixture.0.join("document"), b"concurrent").unwrap();
        assert!(root.finish(&id, &op, &cancel).is_err());
        assert_eq!(
            std::fs::read(fixture.0.join("document")).unwrap(),
            b"concurrent"
        );
        std::fs::remove_file(fixture.0.join("document")).unwrap();
        let stages = root.area("partial").unwrap();
        move_checked(&stages, &id, &root.dir, "document").unwrap();
        // Simulate install succeeding and the process exiting before journaling.
        assert!(root.begin(&id, &op, &cancel).unwrap().0);
        assert_eq!(root.versions(&cancel).unwrap().len(), 1);
    }
    #[test]
    fn containment_changed_sources_nonempty_directory_deletes_and_cancellation_fail_closed() {
        let fixture = Fixture::new();
        let cancel = AtomicBool::new(false);
        let root = fixture.open();
        std::fs::create_dir(fixture.0.join("nested")).unwrap();
        std::fs::write(fixture.0.join("nested/file"), b"keep").unwrap();
        let op = Operation {
            side: sync::Side::Local,
            path: "nested".into(),
            before: Some(Node::Directory),
            change: Change::Delete,
        };
        let id = safe::hash(b"dir");
        root.begin(&id, &op, &cancel).unwrap();
        assert!(root.finish(&id, &op, &cancel).is_err());
        assert!(root.read("../escape", &node(b"keep"), 0, &cancel).is_err());
        assert!(
            root.verify_source("nested/file", &node(b"different"), &cancel)
                .is_err()
        );
        cancel.store(true, Ordering::Release);
        assert!(root.manifest(&cancel).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_roots_ancestors_and_children_are_rejected() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let outside = Fixture::new();
        let cancel = AtomicBool::new(false);
        std::fs::write(outside.0.join("secret"), b"secret").unwrap();
        symlink(&outside.0, fixture.0.join("link")).unwrap();
        assert!(Root::open(&fixture.0.join("link")).is_err());
        assert!(fixture.open().manifest(&cancel).is_err());
        assert!(
            fixture
                .open()
                .read("link/secret", &node(b"secret"), 0, &cancel)
                .is_err()
        );
    }
}
