use crate::{edge::Side, video::Inputs, wire::Mode};
use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::Deserialize;
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
    ensure!(bytes.len() == 64 && bytes.iter().all(u8::is_ascii_hexdigit),
        "expected a 64-character public or private key");
    let mut result = [0; 32];
    for (byte, encoded) in result.iter_mut().zip(bytes.chunks_exact(2)) {
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
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    Ok(base.context("set DESK_SWITCH_STATE to a private directory")?.join("desk-switch"))
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
    let mut file = options.open(path).with_context(|| format!("creating {}", path.display()))?;
    if let Err(error) = file.write_all(contents.as_bytes()).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error.into());
    }
    Ok(())
}

pub fn create_identity(dir: &Path) -> Result<Identity> {
    let pair = Builder::new(NOISE.parse()?).generate_keypair()?;
    create_private(&dir.join("identity"), &format!("{}\n{}\n", hex(&pair.private), hex(&pair.public)))?;
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
    ensure!(remote != identity.public, "enter the other computer's public key");
    match create_private(&dir.join("peer"), &hex(&remote)) {
        Ok(()) => Ok(()),
        Err(_) if load_peer(dir).ok() == Some(remote) => Ok(()),
        Err(error) => Err(error.context("a peer is pinned until you explicitly remove the peer file")),
    }
}

pub fn load_peer(dir: &Path) -> Result<Key> {
    key(fs::read_to_string(dir.join("peer")).context("pin the other computer with pair")?.trim())
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileSettings {
    monitor: Option<MonitorSettings>,
    edge: Option<EdgeSettings>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MonitorSettings {
    id: String,
    windows: u16,
    ubuntu: u16,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EdgeSettings {
    mode: String,
    screen: Option<String>,
    side: Option<String>,
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
}

impl Settings {
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join("settings.toml");
        let config = match fs::read_to_string(&path) {
            Ok(data) => toml::from_str::<FileSettings>(&data)
                .with_context(|| format!("parsing {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => FileSettings::default(),
            Err(error) => return Err(error.into()),
        };
        let monitor = config.monitor.map(|m| {
            ensure!(!m.id.trim().is_empty(), "monitor id is empty");
            ensure!(m.windows != m.ubuntu && m.windows != 0 && m.ubuntu != 0,
                "configure distinct nonzero Windows and Ubuntu VCP 0x60 input values");
            Ok::<_, anyhow::Error>((m.id, Inputs { windows: m.windows, ubuntu: m.ubuntu }))
        }).transpose()?;
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
                    screen: edge.screen.ok_or_else(|| anyhow!("edge screen is required"))?,
                    side: Side::parse(&edge.side.context("edge side is required")?)?,
                })
            }
        };
        Ok(Self { monitor, edge })
    }
}
