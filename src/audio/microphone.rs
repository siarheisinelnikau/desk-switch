use super::{Media, link::Link, playout::Mix, stream};
use anyhow::{Context, Result, bail, ensure};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

const HELP: &str = "Microphone sharing is off until explicitly enabled.\n\
    microphone sources\n\
    microphone serve IP:PORT [--source ENDPOINT]\n\
    microphone play IP:PORT [--output VIRTUAL_CABLE] [--volume 0-100]";

pub fn run(args: &[&str]) -> Result<()> {
    match args {
        [] | ["help" | "--help" | "-h"] => {
            println!("{HELP}");
            return Ok(());
        }
        ["sources"] => {
            for device in super::microphones()? {
                println!("{}\n  {}", device.name, device.id);
            }
            return Ok(());
        }
        ["serve" | "play", _, ..] => {}
        _ => bail!("{HELP}"),
    }
    let address = crate::parse_address(args[1])?;
    let mut endpoint = None;
    let mut mix = Mix {
        volume: 100,
        muted: false,
    };
    let mut options = args[2..].iter().copied();
    while let Some(option) = options.next() {
        match option {
            "--source" if args[0] == "serve" && endpoint.is_none() => {
                endpoint = Some(
                    options
                        .next()
                        .context("--source needs a microphone ID")?
                        .to_owned(),
                )
            }
            "--output" if args[0] == "play" && endpoint.is_none() => {
                endpoint = Some(
                    options
                        .next()
                        .context("--output needs a virtual cable ID")?
                        .to_owned(),
                )
            }
            "--volume" if args[0] == "play" => {
                mix.volume = super::volume(options.next().unwrap_or_default())?
            }
            _ => bail!("invalid microphone option: {option}"),
        }
    }
    let dir = crate::config::state_dir()?;
    let identity = crate::config::load_identity(&dir)?;
    let peer = crate::config::load_peer(&dir)?;
    let _lock = crate::config::lock(&dir, "microphone.lock")?;
    crate::lifecycle::install();
    crate::runtime()?.block_on(async {
        if args[0] == "serve" {
            super::sender::serve_media(address, identity, peer, endpoint.as_deref(), Media::Microphone).await
        } else {
            ensure!(!address.ip().is_unspecified(), "enter the microphone source PC's IP");
            let mix = Arc::new(Mutex::new(mix));
            let mut commands = crate::lifecycle::commands();
            let stop = crate::lifecycle::shutdown();
            tokio::pin!(stop);
            loop {
                let receiving = receive(address, &identity, &peer, endpoint.clone(), Arc::clone(&mix));
                tokio::pin!(receiving);
                let result = loop { tokio::select! {
                    result = &mut stop => { result?; return Ok(()); }
                    command = commands.recv() => {
                        if let Ok(crate::lifecycle::Command::MicrophoneVolume(volume)) = command { mix.lock().unwrap().volume = volume; }
                        continue;
                    }
                    result = &mut receiving => break result,
                } };
                let message = result.err().map(|e| format!("{e:#}")).unwrap_or_default();
                eprintln!("Microphone paused: {message}");
                crate::lifecycle::report(crate::lifecycle::Event::Audio { playing: false, message });
                tokio::select! { result = &mut stop => { result?; break; }, _ = tokio::time::sleep(Duration::from_secs(1)) => {} }
            }
            Ok(())
        }
    })
}

async fn receive(
    address: SocketAddr,
    identity: &crate::config::Identity,
    peer: &crate::config::Key,
    endpoint: Option<String>,
    mix: Arc<Mutex<Mix>>,
) -> Result<()> {
    let socket = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::TcpStream::connect(address),
    )
    .await
    .context("connecting to the microphone source timed out")??;
    let mut link = Link::open_media(socket, identity, peer, true, Media::Microphone).await?;
    let mut renderer = super::output::Renderer::start_microphone(endpoint, mix)?;
    if (&mut renderer.ready).await.is_err() {
        (&mut renderer.done)
            .await
            .context("virtual microphone stopped during startup")??;
        bail!("virtual microphone did not start");
    }
    let mut active = false;
    tokio::select! {
        result = stream::receive(&mut link, &renderer.queue, |_| {
            if !active {
                crate::lifecycle::report(crate::lifecycle::Event::Audio { playing: true, message: "Shared microphone active".into() });
                active = true;
            }
        }) => result,
        result = &mut renderer.done => {
            result.context("virtual microphone stopped")??;
            bail!("virtual microphone stopped");
        }
    }
}
