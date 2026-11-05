use super::link::{CHANNELS, RATE};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};

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

pub struct Capture {
    child: tokio::process::Child,
    samples: tokio::process::ChildStdout,
}

impl Capture {
    pub async fn open(source: Option<&str>) -> Result<Self> {
        let graph = Graph::read().await?;
        let sink = graph.select(source)?;
        let mut child = Command::new("pw-record")
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
        let samples = child
            .stdout
            .take()
            .context("pw-record stdout unavailable")?;
        Ok(Self { child, samples })
    }

    pub async fn stop(&mut self) -> Result<()> {
        self.child.kill().await.context("stopping PipeWire capture")
    }
}

impl tokio::io::AsyncRead for Capture {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.samples).poll_read(cx, buffer)
    }
}

pub(super) async fn playback(output: Option<&str>) -> Result<tokio::process::Child> {
    let graph = Graph::read().await?;
    let sink = graph.select(output)?;
    Command::new("pw-play")
        .args(["--raw", "--format=s16", "--channel-map=stereo", "--latency=10ms",
            "--properties", r#"{ "application.name": "Desk Switch", "media.name": "Desk Switch remote audio", "node.dont-reconnect": true, "node.dont-move": true, "node.dont-fallback": true }"#])
        .arg("--rate").arg(RATE.to_string())
        .arg("--channels").arg(CHANNELS.to_string())
        .arg("--target").arg(sink.serial.to_string()).arg("-")
        .kill_on_drop(true)
        .stdin(Stdio::piped()).spawn()
        .context("starting pw-play; install pipewire-bin")
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
