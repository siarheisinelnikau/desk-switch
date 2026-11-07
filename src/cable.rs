//! Official, unmodified VB-CABLE archives, available without a network request.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{Cursor, Read, Write},
    path::{Component, Path, PathBuf},
    sync::mpsc,
};

pub const WEBSITE: &str = "https://vb-audio.com/Cable/";
pub const DONATE: &str = "https://vb-audio.com/Donate/index.htm";
const NOTICE: &str = include_str!("../assets/third-party/vb-cable/NOTICE.txt");
const LIMIT: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum Platform {
    #[default]
    Windows,
    MacOS,
}
impl Platform {
    pub fn label(self) -> &'static str {
        match self {
            Self::Windows => "Windows",
            Self::MacOS => "macOS",
        }
    }
    pub fn archive(self) -> &'static str {
        match self {
            Self::Windows => "VBCABLE_Driver_Pack46.zip",
            Self::MacOS => "VBCable_MACDriver_Pack108.zip",
        }
    }
    fn folder(self) -> &'static str {
        match self {
            Self::Windows => "VB-CABLE-Windows-Pack46",
            Self::MacOS => "VB-CABLE-macOS-Pack108",
        }
    }
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Windows => {
                include_bytes!("../assets/third-party/vb-cable/VBCABLE_Driver_Pack46.zip")
            }
            Self::MacOS => {
                include_bytes!("../assets/third-party/vb-cable/VBCable_MACDriver_Pack108.zip")
            }
        }
    }
    fn sha256(self) -> &'static str {
        match self {
            Self::Windows => "bc44e8974191a5a2427d6c979e2dab08dbb09925f3f435dfa2f0ba3e45b6eb90",
            Self::MacOS => "e46b41c6876995403cb1da37d7c0d566f59edd83aa9fcbcdc3a205eb0b6e05c7",
        }
    }
}

fn unpack(bytes: &[u8], directory: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    ensure!(archive.len() <= 128, "Too many installer files");
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.enclosed_name().context("Unsafe installer path")?;
        ensure!(
            !name.as_os_str().is_empty()
                && name
                    .components()
                    .all(|part| matches!(part, Component::Normal(_))),
            "Unsafe installer path"
        );
        ensure!(
            entry
                .unix_mode()
                .is_none_or(|mode| mode & 0o170000 != 0o120000),
            "Installer links are not supported"
        );
        total = total
            .checked_add(entry.size())
            .context("Invalid installer size")?;
        ensure!(total <= LIMIT, "Installer archive is too large");
        let destination = directory.join(name);
        if entry.is_dir() {
            fs::create_dir_all(destination)?;
            continue;
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        let copied = std::io::copy(&mut (&mut entry).take(LIMIT + 1), &mut file)?;
        ensure!(
            copied == entry.size() && copied <= LIMIT,
            "Invalid installer file size"
        );
        file.flush()?;
    }
    Ok(())
}

fn extract_at(platform: Platform, downloads: &Path) -> Result<PathBuf> {
    let bytes = platform.bytes();
    ensure!(
        format!("{:x}", Sha256::digest(bytes)) == platform.sha256(),
        "The bundled VB-CABLE archive is damaged"
    );
    let parent = downloads.join("Desk Switch");
    fs::create_dir_all(&parent)?;
    let mut destination = None;
    for number in 1..=1000 {
        let folder = parent.join(format!("{}-{number}", platform.folder()));
        match fs::create_dir(&folder) {
            Ok(()) => {
                destination = Some(folder);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    let directory = destination.context("No unused installer folder is available")?;
    let result = (|| {
        fs::write(directory.join(platform.archive()), bytes)?;
        fs::write(directory.join("VB-CABLE-NOTICE.txt"), NOTICE)?;
        unpack(bytes, &directory)?;
        Ok(directory.clone())
    })();
    // This is the fresh directory created by this operation; existing files are never removed.
    if result.is_err() {
        let _ = fs::remove_dir_all(&directory);
    }
    result
}

pub struct Job(mpsc::Receiver<Result<PathBuf, String>>);
impl Job {
    pub fn start(platform: Platform, ctx: eframe::egui::Context) -> Result<Self> {
        let (send, receive) = mpsc::channel();
        std::thread::Builder::new()
            .name("VB-CABLE extraction".into())
            .spawn(move || {
                let result = dirs::download_dir()
                    .context("The Downloads folder is not available")
                    .and_then(|downloads| extract_at(platform, &downloads))
                    .map_err(|error| format!("{error:#}"));
                let _ = send.send(result);
                ctx.request_repaint();
            })?;
        Ok(Self(receive))
    }
    pub fn poll(&self) -> Option<Result<PathBuf, String>> {
        match self.0.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err("Installer extraction stopped".into()))
            }
        }
    }
}

pub fn open_folder(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute() && path.is_dir(),
        "The extracted installer folder is unavailable"
    );
    #[cfg(windows)]
    let command = "explorer.exe";
    #[cfg(target_os = "linux")]
    let command = "xdg-open";
    #[cfg(target_os = "macos")]
    let command = "open";
    std::process::Command::new(command).arg(path).spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_official_archives_extract_without_overwriting_existing_files() {
        let root = std::env::temp_dir().join(format!("desk-switch-cable-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        for platform in [Platform::Windows, Platform::MacOS] {
            let first = extract_at(platform, &root).unwrap();
            assert_eq!(
                fs::read(first.join(platform.archive())).unwrap(),
                platform.bytes()
            );
            let expected = match platform {
                Platform::Windows => "VBCABLE_Setup_x64.exe",
                Platform::MacOS => "VBCable_MACDriver_Pack108.dmg",
            };
            assert!(first.join(expected).is_file());
            fs::write(first.join("keep.txt"), b"existing").unwrap();
            let second = extract_at(platform, &root).unwrap();
            assert_ne!(first, second);
            assert_eq!(fs::read(first.join("keep.txt")).unwrap(), b"existing");
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn extraction_refuses_traversal_and_never_writes_outside_its_folder() {
        use zip::{ZipWriter, write::SimpleFileOptions};
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("../outside", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"bad").unwrap();
        let data = zip.finish().unwrap().into_inner();
        let root =
            std::env::temp_dir().join(format!("desk-switch-cable-unsafe-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        assert!(unpack(&data, &root).is_err());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::remove_dir(root).unwrap();
    }
}
