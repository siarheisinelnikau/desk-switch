use super::{link::Link, stream};
use crate::config::{Identity, Key};
use anyhow::{Context, Result};
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};

#[cfg(target_os = "linux")]
use super::linux::Capture;
#[cfg(windows)]
use super::windows_capture::Capture;

pub async fn serve(
    address: SocketAddr,
    identity: Identity,
    peer: Key,
    source: Option<&str>,
) -> Result<()> {
    let listener = TcpListener::bind(address)
        .await
        .context("binding the audio sender port")?;
    println!("System audio enabled on {address}. Waiting for the paired receiver.");
    let shutdown = crate::lifecycle::shutdown();
    tokio::pin!(shutdown);
    loop {
        let socket = tokio::select! {
            result = &mut shutdown => { result?; break; }
            accepted = listener.accept() => accepted?.0,
        };
        tokio::select! {
            result = &mut shutdown => { result?; break; }
            result = session(socket, &identity, &peer, source) => {
                if let Err(error) = result { eprintln!("System audio idle. {error:#}"); }
            }
        }
        crate::lifecycle::report(crate::lifecycle::Event::Audio {
            playing: false,
            message: "waiting for the paired receiver".into(),
        });
    }
    println!("System audio off.");
    Ok(())
}

async fn session(
    socket: TcpStream,
    identity: &Identity,
    peer: &Key,
    source: Option<&str>,
) -> Result<()> {
    let mut link = Link::open(socket, identity, peer, false).await?;
    stream::request_start(&mut link).await?;
    let mut incoming = stream::SILENCE;
    let mut capture = tokio::select! {
        biased;
        packet = link.receive(&mut incoming) => {
            packet?;
            anyhow::bail!("unexpected packet before capture readiness");
        }
        result = Capture::open(source) => result?,
    };
    crate::lifecycle::report(crate::lifecycle::Event::Audio {
        playing: true,
        message: "transmitting the selected system mix".into(),
    });
    let result = stream::transmit(&mut link, &mut capture).await;
    capture.stop().await?;
    result
}
