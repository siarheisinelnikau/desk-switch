use anyhow::{Context, Result, bail, ensure};
use std::net::SocketAddr;

mod config;
mod edge;
#[cfg(any(windows, test))]
mod gate;
mod transport;
mod video;
mod wire;
#[cfg(any(windows, test))]
mod controller;
#[cfg(any(windows, target_os = "linux"))]
mod platform;

const USAGE: &str = "Desk Switch\n\
    init                 Create this computer's identity and print its public key\n\
    public-key           Print the existing public key\n\
    pair PUBLIC_KEY      Pin the other computer's public key\n\
    state-dir            Print the settings and identity directory\n\
    probe [MONITOR_ID]   Read DDC/CI VCP 0x60 and enumerate advertised inputs\n\
    portal-check         Ubuntu only; query Wayland portal capabilities\n\
    portal-check --consent  Ubuntu only; open EIS and list edge regions, without injecting input\n\
    run IP:PORT          Ubuntu listens on this address; Windows connects to it";

fn main() {
    if let Err(error) = run() {
        eprintln!("Desk Switch: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    match args.as_slice() {
        [] | ["help" | "--help" | "-h"] => println!("{USAGE}"),
        ["--version" | "-V"] => println!("Desk Switch {}", env!("CARGO_PKG_VERSION")),
        ["state-dir"] => println!("{}", config::state_dir()?.display()),
        ["init"] => println!("{}", config::hex(&config::create_identity(&config::state_dir()?)?.public)),
        ["public-key"] => println!("{}", config::hex(&config::load_identity(&config::state_dir()?)?.public)),
        ["pair", key] => {
            config::pair(&config::state_dir()?, key)?;
            println!("Peer pinned.");
        }
        ["probe"] => platform::probe(None)?,
        ["probe", id] => platform::probe(Some(id))?,
        #[cfg(target_os = "linux")]
        ["portal-check"] => {
            runtime()?.block_on(platform::portal::check())?;
        }
        #[cfg(target_os = "linux")]
        ["portal-check", "--consent"] => {
            let settings = config::Settings::load(&config::state_dir()?)?;
            runtime()?.block_on(async {
                platform::portal::Portal::open(settings.edge).await?.close().await;
                Ok::<_, anyhow::Error>(())
            })?;
        }
        ["run", address] => {
            let address: SocketAddr = address.parse().context("use a numeric IP:PORT or [IPv6]:PORT")?;
            ensure!(address.port() != 0, "port must be nonzero");
            #[cfg(windows)]
            ensure!(!address.ip().is_unspecified(), "use the Ubuntu computer's LAN IP address");
            let dir = config::state_dir()?;
            let settings = config::Settings::load(&dir)?;
            let identity = config::load_identity(&dir)?;
            let peer = config::load_peer(&dir)?;
            platform::run(address, identity, peer, settings)?;
        }
        _ => bail!("invalid command\n\n{USAGE}"),
    }
    Ok(())
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_current_thread().enable_all().build()?)
}
