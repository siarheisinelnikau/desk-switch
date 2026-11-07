#[cfg(any(windows, test))]
mod capture_frames;
mod link;
#[cfg(target_os = "linux")]
mod linux;
mod microphone;
mod mixer;
pub use microphone::run as run_microphone;
#[cfg(windows)]
#[path = "windows_output.rs"]
mod output;
#[cfg(target_os = "linux")]
#[path = "linux_output.rs"]
mod output;
mod player;
mod playout;
mod sender;
mod stream;
#[cfg(windows)]
mod windows_capture;

use crate::config;
use anyhow::ensure;
use anyhow::{Context, Result, bail};

const USAGE: &str = "Desk Switch audio is off until explicitly started.\n\
             audio sources\n\
             audio serve IP:PORT [--source PLAYBACK_SOURCE]\n\
             audio outputs\n\
             audio play IP:PORT [--output PLAYBACK_OUTPUT] [--volume 0-100] [--mute] [--native-volume 0-100] [--native-mute]";

pub fn run(args: &[&str]) -> Result<()> {
    match args {
        [] | ["--help" | "-h" | "help"] => println!("{USAGE}"),
        ["sources"] => list()?,
        ["sources" | "outputs", "--json"] => {
            println!("@desk-switch-data {}", serde_json::to_string(&devices()?)?);
        }
        ["serve", address] | ["serve", address, "--source", _] => {
            let address = crate::parse_address(address)?;
            let dir = config::state_dir()?;
            let identity = config::load_identity(&dir)?;
            let peer = config::load_peer(&dir)?;
            let _lock = config::lock(&dir, "audio.lock")?;
            mixer::restore_pending(&dir)?;
            crate::lifecycle::install();
            crate::runtime()?.block_on(async {
                // A previous listening role may have left recovery entries for
                // apps that were closed. Restore them if they reopen while sending.
                let mut native = mixer::NativeMixer::start(dir.clone(), Default::default())?;
                native.wait_ready().await?;
                let mut native_done = false;
                let result = tokio::select! {
                    result = sender::serve(address, identity, peer, args.get(3).copied()) => result,
                    result = &mut native.done => {
                        native_done = true;
                        result.context("native audio recovery stopped")?.and_then(|_| Err(anyhow::anyhow!("native audio recovery stopped unexpectedly")))
                    }
                };
                let restored = if native_done { Ok(()) } else { native.stop().await };
                result.and(restored)
            })?;
        }
        ["outputs"] => list()?,
        ["play", address, options @ ..] => {
            let address = crate::parse_address(address)?;
            let mut output = None;
            let mut mix = playout::Mix {
                volume: 100,
                muted: false,
            };
            let mut native = crate::workspace::AudioLevel::default();
            let mut options = options.iter().copied();
            while let Some(option) = options.next() {
                match option {
                    "--output" if output.is_none() => {
                        let Some(id) = options.next() else {
                            bail!("--output requires an endpoint ID");
                        };
                        ensure!(
                            !id.is_empty() && !id.starts_with("--"),
                            "--output requires an endpoint ID"
                        );
                        output = Some(id.into());
                    }
                    "--volume" => mix.volume = volume(options.next().unwrap_or_default())?,
                    "--mute" => mix.muted = true,
                    "--native-volume" => {
                        native.volume = volume(options.next().unwrap_or_default())?
                    }
                    "--native-mute" => native.muted = true,
                    _ => bail!("invalid audio option '{option}'\n{USAGE}"),
                }
            }
            ensure!(
                !address.ip().is_unspecified(),
                "audio player must connect to the sender's LAN IP"
            );
            let dir = config::state_dir()?;
            let identity = config::load_identity(&dir)?;
            let peer = config::load_peer(&dir)?;
            let _lock = config::lock(&dir, "audio.lock")?;
            crate::runtime()?.block_on(player::play(
                address, identity, peer, output, mix, native, dir,
            ))?;
        }
        _ => bail!("invalid audio command for this platform\n{USAGE}"),
    }
    Ok(())
}

fn volume(value: &str) -> Result<u8> {
    value
        .parse()
        .ok()
        .filter(|&number| number <= 100)
        .ok_or_else(|| anyhow::anyhow!("volume must be a whole number from 0 to 100"))
}

#[cfg(test)]
mod tests;

fn list() -> Result<()> {
    #[cfg(windows)]
    return output::list();
    #[cfg(target_os = "linux")]
    crate::runtime()?.block_on(linux::list())
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub default: bool,
}

pub fn devices() -> Result<Vec<AudioDevice>> {
    #[cfg(windows)]
    return output::devices();
    #[cfg(target_os = "linux")]
    crate::runtime()?.block_on(linux::devices())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Media {
    System,
    Microphone,
}
impl Media {
    fn purpose(self) -> &'static [u8] {
        match self {
            Self::System => b"Desk Switch audio 1",
            Self::Microphone => b"Desk Switch microphone 1",
        }
    }
}

pub fn microphones() -> Result<Vec<AudioDevice>> {
    #[cfg(windows)]
    return output::microphones();
    #[cfg(target_os = "linux")]
    crate::runtime()?.block_on(linux::microphones())
}

pub fn microphone_bridges() -> Result<Vec<AudioDevice>> {
    #[cfg(windows)]
    return output::microphone_bridges();
    #[cfg(target_os = "linux")]
    Ok(vec![AudioDevice {
        id: "pipewire-virtual-microphone".into(),
        name: "Desk Switch microphone".into(),
        default: true,
    }])
}

#[cfg(windows)]
const SESSION_GROUP: windows::core::GUID =
    windows::core::GUID::from_u128(0x879598a0_7a44_485b_9ad3_03e5cbce55b1);
