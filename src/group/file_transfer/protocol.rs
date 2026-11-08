//! Versioned transfer metadata. Wire paths are display metadata, never read requests.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const VERSION: u16 = 1;
pub const CHUNK: usize = 48 * 1024;
pub const MAX_ENTRIES: usize = 10_000;
pub const MAX_TOTAL: u64 = 64 * 1024 * 1024 * 1024;
pub const MAX_MANIFEST: usize = 4 * 1024 * 1024;

pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn valid_id(id: &str) -> Result<()> {
    ensure!(
        id.len() == 32
            && id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "Invalid file transfer identity"
    );
    Ok(())
}
pub fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn valid_component(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 240
            && name != "."
            && name != ".."
            && !name.ends_with(['.', ' '])
            && !name
                .chars()
                .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c)),
        "A file or folder name cannot be transferred safely between these computers"
    );
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    ensure!(
        !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$")
            && !(stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9')),
        "A file or folder name is reserved by another operating system"
    );
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u16,
    pub id: String,
    pub name: String,
    pub roots: Vec<String>,
    pub entries: Vec<Entry>,
    pub total: u64,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub path: Vec<String>,
    pub kind: Kind,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Kind {
    Directory,
    File { size: u64, sha256: String },
}
impl Manifest {
    pub fn validate(&self) -> Result<()> {
        valid_id(&self.id)?;
        ensure!(
            self.version == VERSION,
            "Unsupported file transfer protocol"
        );
        ensure!(
            !self.entries.is_empty()
                && self.entries.len() <= MAX_ENTRIES
                && !self.roots.is_empty()
                && self.roots.len() <= self.entries.len()
                && !self.name.is_empty()
                && self.name.len() <= 512
                && !self.name.chars().any(char::is_control)
                && self.total <= MAX_TOTAL,
            "The file transfer exceeds its session limits"
        );
        let mut roots = BTreeSet::new();
        for root in &self.roots {
            valid_component(root)?;
            ensure!(
                roots.insert(root.to_lowercase()),
                "Duplicate file transfer root"
            );
        }
        let mut paths = BTreeMap::new();
        let mut total = 0_u64;
        for entry in &self.entries {
            ensure!(
                (1..=64).contains(&entry.path.len())
                    && entry.path.iter().map(String::len).sum::<usize>() + entry.path.len() <= 4096,
                "The file transfer path is too deep or too long"
            );
            for component in &entry.path {
                valid_component(component)?;
            }
            ensure!(
                self.roots.contains(&entry.path[0]),
                "Unknown file transfer root"
            );
            let normalized = entry
                .path
                .iter()
                .map(|v| v.to_lowercase())
                .collect::<Vec<_>>();
            ensure!(
                paths.insert(normalized, entry).is_none(),
                "File transfer paths collide"
            );
            if let Kind::File { size, sha256 } = &entry.kind {
                ensure!(valid_hash(sha256), "Invalid file transfer checksum");
                total = total
                    .checked_add(*size)
                    .filter(|v| *v <= MAX_TOTAL)
                    .ok_or_else(|| {
                        anyhow::anyhow!("The file transfer exceeds its session limits")
                    })?;
            }
        }
        ensure!(total == self.total, "Invalid file transfer total size");
        for root in roots {
            ensure!(
                paths.contains_key(&vec![root]),
                "Missing file transfer root"
            );
        }
        for path in paths.keys() {
            if path.len() > 1 {
                ensure!(
                    paths
                        .get(&path[..path.len() - 1])
                        .is_some_and(|parent| matches!(parent.kind, Kind::Directory)
                            && parent.path == paths[path].path[..path.len() - 1]),
                    "A file transfer parent folder is missing"
                );
            }
        }
        ensure!(
            serde_json::to_vec(self)?.len() <= MAX_MANIFEST,
            "The file transfer manifest is too large"
        );
        Ok(())
    }
    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        Ok(hash(&serde_json::to_vec(self)?))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Origin {
    Send,
    Clipboard,
}
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct Stamp {
    pub counter: u64,
    pub author: String,
}

/// Stop-and-wait chunks bound memory and give the receiver durable backpressure.
/// A reconnect begins with the same manifest and receiver-verified prefix.
#[derive(Debug, Deserialize, Serialize)]
pub(crate) enum Packet {
    Announce {
        id: String,
        name: String,
        stamp: Stamp,
    },
    Offer {
        manifest: Manifest,
        origin: Origin,
        stamp: Option<Stamp>,
    },
    Accepted,
    Ready {
        index: u32,
        offset: u64,
        prefix: String,
    },
    Chunk {
        index: u32,
        offset: u64,
        bytes: Vec<u8>,
        digest: String,
    },
    Ack {
        index: u32,
        offset: u64,
    },
    Finish,
    Complete,
    Cancel,
    Resume {
        id: String,
    },
    CancelTransfer {
        id: String,
    },
    Queued,
    Working,
    Busy,
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Manifest {
        Manifest {
            version: VERSION,
            id: "12".repeat(16),
            name: "Folder".into(),
            roots: vec!["Folder".into()],
            entries: vec![
                Entry {
                    path: vec!["Folder".into()],
                    kind: Kind::Directory,
                },
                Entry {
                    path: vec!["Folder".into(), "empty".into()],
                    kind: Kind::File {
                        size: 0,
                        sha256: hash(&[]),
                    },
                },
            ],
            total: 0,
        }
    }
    #[test]
    fn rejects_cross_platform_path_escapes_collisions_and_missing_parents() {
        for path in [
            "../private",
            "..",
            "C:",
            "file:stream",
            "CON.txt",
            "a\\b",
            "foo.",
            "foo ",
            "\0bad",
        ] {
            assert!(valid_component(path).is_err(), "{path:?}");
        }
        assert!(fixture().validate().is_ok());
        let mut bad = fixture();
        bad.entries.push(Entry {
            path: vec!["folder".into(), "EMPTY".into()],
            kind: Kind::Directory,
        });
        assert!(bad.validate().is_err());
        let mut bad = fixture();
        bad.entries[1].path.insert(1, "missing".into());
        assert!(bad.validate().is_err());
        let mut bad = fixture();
        bad.total = 1;
        assert!(bad.validate().is_err());
    }
}
