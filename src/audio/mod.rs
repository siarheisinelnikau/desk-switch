mod link;
#[cfg(target_os = "linux")]
mod linux;
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
use anyhow::{Result, bail};

const USAGE: &str = "Desk Switch audio is off until explicitly started.\n\
             audio sources\n\
             audio serve IP:PORT [--source PLAYBACK_SOURCE]\n\
             audio outputs\n\
             audio play IP:PORT [--output PLAYBACK_OUTPUT] [--volume 0-100] [--mute]";

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
            crate::lifecycle::install();
            crate::runtime()?.block_on(sender::serve(
                address,
                identity,
                peer,
                args.get(3).copied(),
            ))?;
        }
        ["outputs"] => list()?,
        ["play", address, options @ ..] => {
            let address = crate::parse_address(address)?;
            let mut output = None;
            let mut mix = playout::Mix {
                volume: 100,
                muted: false,
            };
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
            crate::runtime()?.block_on(player::play(address, identity, peer, output, mix))?;
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

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
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
