use crate::{edge::Side, video::Inputs, wire::Mode};
use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::{Deserialize, Serialize};
use snow::Builder;
use std::{
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub const NOISE: &str = "Noise_KK_25519_ChaChaPoly_BLAKE2s";
pub type Key = [u8; 32];

pub struct Identity {
    pub private: Key,
    pub public: Key,
}

pub fn hex(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut result, "{byte:02x}").unwrap();
    }
    result
}

fn key(value: &str) -> Result<Key> {
    let bytes = value.as_bytes();
    ensure!(
        bytes.len() == 64 && bytes.iter().all(u8::is_ascii_hexdigit),
        "expected a 64-character public or private key"
    );
    let mut result = [0; 32];
    for (byte, encoded) in result.iter_mut().zip(bytes.as_chunks::<2>().0) {
        *byte = u8::from_str_radix(std::str::from_utf8(encoded)?, 16)?;
    }
    ensure!(result != [0; 32], "invalid all-zero key");
    Ok(result)
}

pub fn state_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("DESK_SWITCH_STATE") {
        return Ok(path.into());
    }
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    Ok(base
        .context("set DESK_SWITCH_STATE to a private directory")?
        .join("desk-switch"))
}

pub fn lock(dir: &Path, name: &str) -> Result<fs::File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(name))?;
    file.try_lock().with_context(|| {
        format!("another Desk Switch process holds {name} in this state directory")
    })?;
    Ok(file)
}

fn create_private(path: &Path, contents: &str) -> Result<()> {
    let directory = path.parent().context("state path has no parent")?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(directory)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    if let Err(error) = file
        .write_all(contents.as_bytes())
        .and_then(|()| file.sync_all())
    {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error.into());
    }
    Ok(())
}

pub fn create_identity(dir: &Path) -> Result<Identity> {
    let pair = Builder::new(NOISE.parse()?).generate_keypair()?;
    create_private(
        &dir.join("identity"),
        &format!("{}\n{}\n", hex(&pair.private), hex(&pair.public)),
    )?;
    load_identity(dir)
}

pub fn load_identity(dir: &Path) -> Result<Identity> {
    let path = dir.join("identity");
    let data = fs::read_to_string(path).context("run init to create this computer's identity")?;
    let mut lines = data.lines();
    let private = key(lines.next().context("missing private key")?)?;
    let public = key(lines.next().context("missing public key")?)?;
    Ok(Identity { private, public })
}

pub fn pair(dir: &Path, remote: &str) -> Result<()> {
    let identity = load_identity(dir)?;
    let remote = key(remote)?;
    ensure!(
        remote != identity.public,
        "enter the other computer's public key"
    );
    match create_private(&dir.join("peer"), &hex(&remote)) {
        Ok(()) => Ok(()),
        Err(_) if load_peer(dir).ok() == Some(remote) => Ok(()),
        Err(error) => {
            Err(error.context("a peer is pinned until you explicitly remove the peer file"))
        }
    }
}

pub fn load_peer(dir: &Path) -> Result<Key> {
    key(fs::read_to_string(dir.join("peer"))
        .context("pin the other computer with pair")?
        .trim())
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileSettings {
    pub monitor: Option<MonitorSettings>,
    pub edge: Option<EdgeSettings>,
    pub capture: Option<CaptureSettings>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorSettings {
    pub id: String,
    #[serde(alias = "windows")]
    pub host: u16,
    #[serde(alias = "ubuntu")]
    pub peer: u16,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureSettings {
    pub devices: Vec<PathBuf>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeSettings {
    pub mode: String,
    pub screen: Option<String>,
    pub side: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Edge {
    pub mode: Mode,
    pub screen: String,
    pub side: Side,
}

pub struct Settings {
    pub monitor: Option<(String, Inputs)>,
    pub edge: Option<Edge>,
    #[cfg(target_os = "linux")]
    pub capture_devices: Vec<PathBuf>,
}

pub fn read_settings(dir: &Path) -> Result<FileSettings> {
    let path = dir.join("settings.toml");
    match fs::read_to_string(&path) {
        Ok(data) => toml::from_str(&data).with_context(|| format!("parsing {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(FileSettings::default()),
        Err(error) => Err(error.into()),
    }
}

pub fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let directory = path.parent().context("settings path has no directory")?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(directory)?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)?;
        Ok::<_, anyhow::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

pub fn save_settings(dir: &Path, document: &FileSettings) -> Result<()> {
    Settings::from_document(document.clone())?;
    write_atomic(
        &dir.join("settings.toml"),
        &toml::to_string_pretty(document)?,
    )
}

impl Settings {
    pub fn load(dir: &Path) -> Result<Self> {
        Self::from_document(read_settings(dir)?)
    }

    fn from_document(config: FileSettings) -> Result<Self> {
        let monitor = config
            .monitor
            .map(|m| {
                ensure!(!m.id.trim().is_empty(), "monitor id is empty");
                ensure!(
                    m.host != m.peer && m.host != 0 && m.peer != 0,
                    "configure distinct nonzero host and peer VCP 0x60 input values"
                );
                Ok::<_, anyhow::Error>((
                    m.id,
                    Inputs {
                        host: m.host,
                        peer: m.peer,
                    },
                ))
            })
            .transpose()?;
        let edge = match config.edge {
            None => None,
            Some(edge) if edge.mode == "off" => None,
            Some(edge) => {
                let mode = match edge.mode.as_str() {
                    "input" => Mode::Input,
                    "video" => Mode::Video,
                    _ => bail!("edge mode must be off, input, or video"),
                };
                if mode == Mode::Video && monitor.is_none() {
                    bail!("configure the shared monitor before enabling video at the edge");
                }
                Some(Edge {
                    mode,
                    screen: edge
                        .screen
                        .ok_or_else(|| anyhow!("edge screen is required"))?,
                    side: Side::parse(&edge.side.context("edge side is required")?)?,
                })
            }
        };
        let capture_devices = config
            .capture
            .map(|capture| capture.devices)
            .unwrap_or_default();
        ensure!(
            capture_devices.len() <= 16,
            "at most 16 capture devices may be selected"
        );
        Ok(Self {
            monitor,
            edge,
            #[cfg(target_os = "linux")]
            capture_devices,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    Host,
    Receiver,
}

impl Role {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "host" => Ok(Self::Host),
            "receiver" => Ok(Self::Receiver),
            _ => bail!("role must be host (keyboard/mouse attached here) or receiver"),
        }
    }

    pub fn default_for_platform() -> Self {
        if cfg!(windows) {
            Self::Host
        } else {
            Self::Receiver
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_mapping_accepts_role_names_and_existing_configurations() {
        let legacy: FileSettings =
            toml::from_str("[monitor]\nid='shared'\nwindows=17\nubuntu=15").unwrap();
        let roles: FileSettings =
            toml::from_str("[monitor]\nid='shared'\nhost=17\npeer=15").unwrap();
        let (legacy, roles) = (legacy.monitor.unwrap(), roles.monitor.unwrap());
        assert_eq!((legacy.host, legacy.peer), (roles.host, roles.peer));
        assert!(
            toml::from_str::<FileSettings>("[monitor]\nid='shared'\nhost=17\nwindows=18\npeer=15")
                .is_err()
        );
    }

    #[test]
    fn either_role_is_explicitly_selectable() {
        assert_eq!(Role::parse("host").unwrap(), Role::Host);
        assert_eq!(Role::parse("receiver").unwrap(), Role::Receiver);
        assert!(Role::parse("windows").is_err());
    }
}
