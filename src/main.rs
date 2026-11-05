#![cfg_attr(windows, windows_subsystem = "windows")]

use anyhow::{Context, Result, bail, ensure};
use std::net::SocketAddr;

mod audio;
mod config;
mod controller;
mod edge;
mod gate;
mod gui;
#[cfg(any(windows, test))]
mod keys;
mod lifecycle;
#[cfg(any(windows, target_os = "linux"))]
mod platform;
mod transport;
mod video;
mod wire;

const USAGE: &str = "Desk Switch\n\
    gui [--tray]         Open the desktop window, optionally in the system tray\n\
    init                 Create this computer's identity and print its public key\n\
    public-key           Print the existing public key\n\
    pair PUBLIC_KEY      Pin the other computer's public key\n\
    state-dir            Print the settings and identity directory\n\
    probe [MONITOR_ID]   Read DDC/CI VCP 0x60 and enumerate advertised inputs\n\
    portal-check         Ubuntu only; query Wayland portal capabilities\n\
    portal-check --consent  Ubuntu only; open EIS and list edge regions, without injecting input\n\
    run [--role host|receiver] IP:PORT  Select the keyboard host or receiving PC\n\
    audio help           Opt in to independent system audio between either OS";

fn main() {
    #[cfg(windows)]
    if std::env::args().len() > 1
        && !std::env::args().nth(1).is_some_and(|arg| arg == "gui")
        && !lifecycle::managed()
    {
        // Keep explicit CLI commands usable from a terminal without a console on GUI launch.
        unsafe {
            windows_sys::Win32::System::Console::AttachConsole(u32::MAX);
        }
    }
    if let Err(error) = run() {
        eprintln!("Desk Switch: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    match args.as_slice() {
        [] | ["gui"] => gui::run(false)?,
        ["gui", "--tray"] => gui::run(true)?,
        ["help" | "--help" | "-h"] => println!("{USAGE}"),
        ["--version" | "-V"] => println!("Desk Switch {}", env!("CARGO_PKG_VERSION")),
        ["state-dir"] => println!("{}", config::state_dir()?.display()),
        ["init"] => println!(
            "{}",
            config::hex(&config::create_identity(&config::state_dir()?)?.public)
        ),
        ["public-key"] => println!(
            "{}",
            config::hex(&config::load_identity(&config::state_dir()?)?.public)
        ),
        ["pair", key] => {
            config::pair(&config::state_dir()?, key)?;
            println!("Peer pinned.");
        }
        ["probe"] => platform::probe(None)?,
        ["probe", "--json"] => println!(
            "@desk-switch-data {}",
            serde_json::to_string(&platform::discover()?)?
        ),
        ["probe", id] => platform::probe(Some(id))?,
        #[cfg(target_os = "linux")]
        ["portal-check"] => {
            runtime()?.block_on(platform::portal::check())?;
        }
        #[cfg(target_os = "linux")]
        ["portal-check", "--consent"] | ["portal-check", "--consent", "--json"] => {
            let settings = config::Settings::load(&config::state_dir()?)?;
            runtime()?.block_on(async {
                let portal = platform::portal::Portal::open(settings.edge).await?;
                if args.len() == 3 {
                    println!(
                        "@desk-switch-data {}",
                        serde_json::to_string(&portal.screen_ids())?
                    );
                }
                portal.close().await;
                Ok::<_, anyhow::Error>(())
            })?;
        }
        ["audio", args @ ..] => audio::run(args)?,
        ["run", address] | ["run", "--role", _, address] => {
            let role = args
                .get(2)
                .filter(|_| args.len() == 4)
                .map(|role| config::Role::parse(role))
                .transpose()?
                .unwrap_or_else(config::Role::default_for_platform);
            let address = parse_address(address)?;
            if role == config::Role::Host {
                ensure!(
                    !address.ip().is_unspecified(),
                    "host must connect to the receiver's LAN IP"
                );
            }
            let dir = config::state_dir()?;
            let settings = config::Settings::load(&dir)?;
            let identity = config::load_identity(&dir)?;
            let peer = config::load_peer(&dir)?;
            let _lock = config::lock(&dir, "run.lock")?;
            lifecycle::install();
            platform::run(role, address, identity, peer, settings)?;
        }
        _ => bail!("invalid command\n\n{USAGE}"),
    }
    Ok(())
}

fn parse_address(address: &str) -> Result<SocketAddr> {
    let address: SocketAddr = address
        .parse()
        .context("use a numeric IP:PORT or [IPv6]:PORT")?;
    ensure!(address.port() != 0, "port must be nonzero");
    Ok(address)
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?)
}
