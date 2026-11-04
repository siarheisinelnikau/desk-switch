use super::{
    link::{CHANNELS, Link, RATE},
    stream,
};
use crate::config::{Identity, Key};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{net::SocketAddr, process::Stdio, time::Duration};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, TcpStream},
    process::Command,
    time::timeout,
};

const GRAPH_LIMIT: u64 = 8 * 1024 * 1024;
const CAPTURE_PROPERTIES: &str = r#"{ "application.name": "Desk Switch", "node.name": "desk-switch-capture", "media.name": "Desk Switch system audio", "stream.capture.sink": true, "node.dont-reconnect": true, "node.dont-move": true, "node.dont-fallback": true }"#;

struct Sink {
    name: String,
    description: String,
    serial: u64,
}

struct Graph {
    default: Option<String>,
    sinks: Vec<Sink>,
}

impl Graph {
    async fn read() -> Result<Self> {
        let mut child = Command::new("pw-dump")
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .context("starting pw-dump; install pipewire-bin")?;
        timeout(Duration::from_secs(3), async {
            let mut data = Vec::new();
            child
                .stdout
                .take()
                .context("pw-dump stdout unavailable")?
                .take(GRAPH_LIMIT + 1)
                .read_to_end(&mut data)
                .await?;
            ensure!(
                data.len() as u64 <= GRAPH_LIMIT,
                "PipeWire graph exceeds the size limit"
            );
            ensure!(
                child.wait().await?.success(),
                "pw-dump failed; start audio inside your PipeWire desktop session"
            );
            Self::parse(&data)
        })
        .await
        .context("PipeWire graph query timed out")?
    }

    fn parse(data: &[u8]) -> Result<Self> {
        let objects: Vec<Value> = serde_json::from_slice(data).context("reading pw-dump JSON")?;
        let sinks = objects
            .iter()
            .filter_map(|object| {
                let props = &object["info"]["props"];
                if object["type"] != "PipeWire:Interface:Node"
                    || props["media.class"] != "Audio/Sink"
                {
                    return None;
                }
                let name = props["node.name"].as_str()?;
                let serial = props["object.serial"]
                    .as_u64()
                    .or_else(|| props["object.serial"].as_str()?.parse().ok())?;
                if name.is_empty() || serial == 0 {
                    return None;
                }
                Some(Sink {
                    name: name.into(),
                    description: props["node.description"].as_str().unwrap_or(name).into(),
                    serial,
                })
            })
            .collect();
        let default = objects
            .iter()
            .filter(|object| {
                object["type"] == "PipeWire:Interface:Metadata"
                    && object["props"]["metadata.name"] == "default"
            })
            .filter_map(|object| object["metadata"].as_array())
            .flatten()
            .find(|item| item["subject"] == 0 && item["key"] == "default.audio.sink")
            .and_then(|item| item["value"]["name"].as_str())
            .map(str::to_owned);
        Ok(Self { sinks, default })
    }

    fn select(&self, source: Option<&str>) -> Result<&Sink> {
        let name = source
            .or(self.default.as_deref())
            .context("no default playback sink; use audio sources and --source NODE")?;
        let mut matches = self.sinks.iter().filter(|sink| sink.name == name);
        let sink = matches
            .next()
            .with_context(|| format!("no playback sink named '{name}'; use audio sources"))?;
        ensure!(matches.next().is_none(), "ambiguous playback sink '{name}'");
        Ok(sink)
    }
}

pub async fn list() -> Result<()> {
    let graph = Graph::read().await?;
    ensure!(!graph.sinks.is_empty(), "no PipeWire playback sinks found");
    for sink in &graph.sinks {
        let default = if graph.default.as_ref() == Some(&sink.name) {
            " [default]"
        } else {
            ""
        };
        println!("{}{}\n  {}", sink.description, default, sink.name);
    }
    Ok(())
}

pub async fn serve(
    address: SocketAddr,
    identity: Identity,
    peer: Key,
    source: Option<&str>,
) -> Result<()> {
    let listener = TcpListener::bind(address)
        .await
        .context("binding the Ubuntu audio port")?;
    println!("Ubuntu audio enabled on {address}. Waiting for the paired Windows receiver.");
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    loop {
        let socket = tokio::select! {
            result = &mut shutdown => { result?; break; }
            accepted = listener.accept() => accepted?.0,
        };
        tokio::select! {
            result = &mut shutdown => { result?; break; }
            result = session(socket, &identity, &peer, source) => {
                if let Err(error) = result {
                    eprintln!("Ubuntu audio idle. {error:#}");
                }
            }
        }
    }
    println!("Ubuntu audio off.");
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
    let graph = tokio::select! {
        biased;
        packet = link.receive(&mut incoming) => {
            packet?;
            anyhow::bail!("unexpected packet while selecting a playback sink");
        }
        result = Graph::read() => result?,
    };
    let sink = graph.select(source)?;
    let mut capture = Command::new("pw-record")
        .args([
            "--raw",
            "--format=s16",
            "--channel-map=stereo",
            "--latency=10ms",
            "--properties",
            CAPTURE_PROPERTIES,
        ])
        .arg("--rate")
        .arg(RATE.to_string())
        .arg("--channels")
        .arg(CHANNELS.to_string())
        .arg("--target")
        .arg(sink.serial.to_string())
        .arg("-")
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .context("starting pw-record; install pipewire-bin")?;
    println!(
        "Capturing the playback monitor of {} ({}).",
        sink.description, sink.name
    );
    let mut samples = capture
        .stdout
        .take()
        .context("pw-record stdout unavailable")?;
    let result = stream::transmit(&mut link, &mut samples).await;
    drop(samples);
    capture.kill().await.context("stopping PipeWire capture")?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_selection_only_accepts_playback_sinks() {
        let graph = Graph::parse(
            br#"[
            {"type":"PipeWire:Interface:Node","info":{"props":{
                "media.class":"Audio/Source","node.name":"mic","object.serial":1
            }}},
            {"type":"PipeWire:Interface:Node","info":{"props":{
                "media.class":"Audio/Sink","node.name":"speakers","object.serial":20
            }}},
            {"type":"PipeWire:Interface:Node","info":{"props":{
                "media.class":"Audio/Sink","node.name":"virtual","object.serial":"30"
            }}},
            {"type":"PipeWire:Interface:Metadata","props":{"metadata.name":"default"},
              "metadata":[{"subject":0,"key":"default.audio.sink","value":{"name":"speakers"}}]}
        ]"#,
        )
        .unwrap();
        assert_eq!(graph.select(None).unwrap().serial, 20);
        assert_eq!(graph.select(Some("virtual")).unwrap().serial, 30);
        assert!(graph.select(Some("mic")).is_err());
        assert!(graph.select(Some("missing")).is_err());
    }
}
