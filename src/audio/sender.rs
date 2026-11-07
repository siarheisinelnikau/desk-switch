use super::{link::Link, stream};
use crate::config::{Identity, Key};
use anyhow::{Context, Result};
use std::net::SocketAddr;
use tokio::net::TcpStream;

#[cfg(target_os = "linux")]
use super::linux::Capture;
#[cfg(target_os = "macos")]
use super::macos::Capture;
#[cfg(windows)]
use super::windows_capture::Capture;

pub async fn serve(
    address: SocketAddr,
    identity: Identity,
    peer: Key,
    source: Option<&str>,
) -> Result<()> {
    serve_media(address, identity, peer, source, super::Media::System).await
}

pub async fn serve_media(
    address: SocketAddr,
    identity: Identity,
    peer: Key,
    source: Option<&str>,
    media: super::Media,
) -> Result<()> {
    let listener = crate::listen::tcp(address).context("binding the audio sender port")?;
    let label = if media == super::Media::System {
        "System audio"
    } else {
        "Microphone"
    };
    println!(
        "{label} enabled on {}. Waiting for the paired receiver.",
        listener.local_addr()?
    );
    let shutdown = crate::lifecycle::shutdown();
    tokio::pin!(shutdown);
    loop {
        let socket = tokio::select! {
            result = &mut shutdown => { result?; break; }
            accepted = listener.accept() => accepted?.0,
        };
        tokio::select! {
            result = &mut shutdown => { result?; break; }
            result = session(socket, &identity, &peer, source, media) => {
                if let Err(error) = result { eprintln!("{label} idle. {error:#}"); }
            }
        }
        crate::lifecycle::report(crate::lifecycle::Event::Audio {
            playing: false,
            message: "waiting for the paired receiver".into(),
        });
    }
    println!("{label} off.");
    Ok(())
}

async fn session(
    socket: TcpStream,
    identity: &Identity,
    peer: &Key,
    source: Option<&str>,
    media: super::Media,
) -> Result<()> {
    let mut link = if media == super::Media::System {
        Link::open(socket, identity, peer, false).await?
    } else {
        Link::open_media(socket, identity, peer, false, media).await?
    };
    stream::request_start(&mut link).await?;
    let mut incoming = stream::SILENCE;
    let mut capture = tokio::select! {
        biased;
        packet = link.receive(&mut incoming) => {
            packet?;
            anyhow::bail!("unexpected packet before capture readiness");
        }
        result = async {
            if media == super::Media::System { Capture::open(source).await } else { Capture::open_media(source, media).await }
        } => result?,
    };
    crate::lifecycle::report(crate::lifecycle::Event::Audio {
        playing: true,
        message: if media == super::Media::System {
            "transmitting the selected system mix"
        } else {
            "Shared microphone active"
        }
        .into(),
    });
    let result = stream::transmit(&mut link, &mut capture).await;
    capture.stop().await?;
    result
}
