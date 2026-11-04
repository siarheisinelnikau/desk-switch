mod link;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(windows, test))]
mod playout;
mod stream;
#[cfg(windows)]
mod windows;

use crate::config;
#[cfg(windows)]
use anyhow::ensure;
use anyhow::{Result, bail};

const USAGE: &str = "Desk Switch audio is off until explicitly started.\n\
    Ubuntu   audio sources\n\
    Ubuntu   audio serve IP:PORT [--source PIPEWIRE_SINK]\n\
    Windows  audio outputs\n\
    Windows  audio play IP:PORT [--output ENDPOINT_ID] [--volume 0-100] [--mute]";

pub fn run(args: &[&str]) -> Result<()> {
    match args {
        [] | ["--help" | "-h" | "help"] => println!("{USAGE}"),
        #[cfg(target_os = "linux")]
        ["sources"] => crate::runtime()?.block_on(linux::list())?,
        #[cfg(target_os = "linux")]
        ["serve", address] | ["serve", address, "--source", _] => {
            let address = crate::parse_address(address)?;
            let dir = config::state_dir()?;
            let identity = config::load_identity(&dir)?;
            let peer = config::load_peer(&dir)?;
            let _lock = config::lock(&dir, "audio.lock")?;
            crate::runtime()?.block_on(linux::serve(
                address,
                identity,
                peer,
                args.get(3).copied(),
            ))?;
        }
        #[cfg(windows)]
        ["outputs"] => windows::list()?,
        #[cfg(windows)]
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
            let dir = config::state_dir()?;
            let identity = config::load_identity(&dir)?;
            let peer = config::load_peer(&dir)?;
            let _lock = config::lock(&dir, "audio.lock")?;
            crate::runtime()?.block_on(windows::play(address, identity, peer, output, mix))?;
        }
        _ => bail!("invalid audio command for this platform\n{USAGE}"),
    }
    Ok(())
}

#[cfg(windows)]
fn volume(value: &str) -> Result<u8> {
    value
        .parse()
        .ok()
        .filter(|&number| number <= 100)
        .ok_or_else(|| anyhow::anyhow!("volume must be a whole number from 0 to 100"))
}

#[cfg(test)]
mod tests;
