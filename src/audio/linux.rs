use super::link::{CHANNELS, RATE};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};

fn command(program: &str) -> Command {
    let mut command = Command::new(program);
    let parent = rustix::process::getpid();
    // These helpers must disappear even if the managed service is force-killed.
    // The post-prctl parent check closes the fork/parent-exit race. Only raw
    // syscalls and errno conversion run between fork and exec.
    unsafe {
        command.pre_exec(move || {
            rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::KILL))?;
            if rustix::process::getppid() != Some(parent) {
                return Err(std::io::Error::from_raw_os_error(10)); // ECHILD
            }
            Ok(())
        });
    }
    command.kill_on_drop(true);
    command
}

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
    sources: Vec<Sink>,
    default_source: Option<String>,
}

impl Graph {
    async fn read() -> Result<Self> {
        let mut child = command("pw-dump")
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
        let sources = objects
            .iter()
            .filter_map(|object| {
                let props = &object["info"]["props"];
                let name = props["node.name"].as_str()?;
                if object["type"] != "PipeWire:Interface:Node"
                    || props["media.class"] != "Audio/Source"
                    || name.starts_with("desk-switch-")
                    || props["node.virtual"] == true
                    || props["node.virtual"] == "true"
                    || name.ends_with(".monitor")
                    || props["device.class"] == "monitor"
                {
                    return None;
                }
                let serial = props["object.serial"]
                    .as_u64()
                    .or_else(|| props["object.serial"].as_str()?.parse().ok())?;
                (serial > 0).then(|| Sink {
                    name: name.into(),
                    description: props["node.description"].as_str().unwrap_or(name).into(),
                    serial,
                })
            })
            .collect();
        let default_source = objects
            .iter()
            .filter(|o| {
                o["type"] == "PipeWire:Interface:Metadata"
                    && o["props"]["metadata.name"] == "default"
            })
            .filter_map(|o| o["metadata"].as_array())
            .flatten()
            .find(|m| m["subject"] == 0 && m["key"] == "default.audio.source")
            .and_then(|m| m["value"]["name"].as_str())
            .map(str::to_owned);
        Ok(Self {
            sinks,
            default,
            sources,
            default_source,
        })
    }

    fn select(&self, source: Option<&str>) -> Result<&Sink> {
        let name = source
            .or(self.default.as_deref())
            .context("no default playback sink; use audio sources and --source NODE")?;
        ensure!(
            !name.starts_with("desk-switch-"),
            "the microphone feed cannot be used as a system audio source or speaker"
        );
        let mut matches = self.sinks.iter().filter(|sink| sink.name == name);
        let sink = matches
            .next()
            .with_context(|| format!("no playback sink named '{name}'; use audio sources"))?;
        ensure!(matches.next().is_none(), "ambiguous playback sink '{name}'");
        Ok(sink)
    }
}

pub async fn devices() -> Result<Vec<super::AudioDevice>> {
    let graph = Graph::read().await?;
    ensure!(!graph.sinks.is_empty(), "no PipeWire playback sinks found");
    Ok(graph
        .sinks
        .iter()
        .filter(|s| !s.name.starts_with("desk-switch-"))
        .map(|sink| super::AudioDevice {
            id: sink.name.clone(),
            name: sink.description.clone(),
            default: graph.default.as_ref() == Some(&sink.name),
        })
        .collect())
}

pub async fn microphones() -> Result<Vec<super::AudioDevice>> {
    let graph = Graph::read().await?;
    Ok(graph
        .sources
        .iter()
        .map(|source| super::AudioDevice {
            id: source.name.clone(),
            name: source.description.clone(),
            default: graph.default_source.as_ref() == Some(&source.name),
        })
        .collect())
}

pub async fn list() -> Result<()> {
    for device in devices().await? {
        println!(
            "{}{}\n  {}",
            device.name,
            if device.default { " [default]" } else { "" },
            device.id
        );
    }
    Ok(())
}

pub struct Capture {
    child: tokio::process::Child,
    samples: tokio::process::ChildStdout,
}

impl Capture {
    pub async fn open(source: Option<&str>) -> Result<Self> {
        Self::open_media(source, super::Media::System).await
    }
    pub async fn open_media(source: Option<&str>, media: super::Media) -> Result<Self> {
        let graph = Graph::read().await?;
        let sink = match media {
            super::Media::System => graph.select(source)?,
            super::Media::Microphone => {
                let id = source
                    .or(graph.default_source.as_deref())
                    .context("no default physical microphone")?;
                graph
                    .sources
                    .iter()
                    .find(|s| s.name == id)
                    .context("the selected physical microphone is unavailable")?
            }
        };
        let mut child = command("pw-record")
            .args([
                "--raw",
                "--format=s16",
                "--channel-map=stereo",
                "--latency=10ms",
                "--properties",
                if media == super::Media::System { CAPTURE_PROPERTIES } else {
                    r#"{ "application.name": "Desk Switch", "node.name": "desk-switch-microphone-capture", "media.name": "Desk Switch microphone", "node.dont-reconnect": true, "node.dont-move": true, "node.dont-fallback": true }"#
                },
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
            "Capturing {media:?} from {} ({}).",
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
    playback_target(&sink.serial.to_string())
}

fn playback_target(target: &str) -> Result<tokio::process::Child> {
    command("pw-play")
        .args(["--raw", "--format=s16", "--channel-map=stereo", "--latency=10ms",
            "--properties", r#"{ "application.name": "Desk Switch", "media.name": "Desk Switch remote audio", "node.dont-reconnect": true, "node.dont-move": true, "node.dont-fallback": true }"#])
        .arg("--rate").arg(RATE.to_string()).arg("--channels").arg(CHANNELS.to_string())
        .arg("--target").arg(target).arg("-").kill_on_drop(true).stdin(Stdio::piped()).spawn()
        .context("starting pw-play; install pipewire-bin")
}

pub(super) async fn microphone_playback() -> Result<(tokio::process::Child, tokio::process::Child)>
{
    let name = format!("desk-switch-microphone-feed-{}", std::process::id());
    let capture = serde_json::json!({"application.name":"Desk Switch", "node.name":name,
        "node.description":"Desk Switch microphone feed", "media.class":"Audio/Sink", "node.virtual":true,
        "node.autoconnect":false, "node.hidden":true, "priority.session":0});
    let playback = serde_json::json!({"application.name":"Desk Switch", "node.name":"desk-switch-microphone",
        "node.description":"Desk Switch microphone", "media.class":"Audio/Source", "node.virtual":true,
        "node.autoconnect":false, "priority.session":0});
    let mut bridge = command("pw-loopback")
        .args(["--channels=2", "--channel-map=stereo", "--latency=10"])
        .arg("--capture-props")
        .arg(capture.to_string())
        .arg("--playback-props")
        .arg(playback.to_string())
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .context("starting the PipeWire virtual microphone")?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        ensure!(
            bridge.try_wait()?.is_none(),
            "PipeWire virtual microphone stopped during setup"
        );
        if let Ok(graph) = Graph::read().await
            && let Some(sink) = graph.sinks.iter().find(|s| s.name == name)
        {
            let player = playback_target(&sink.serial.to_string())?;
            return Ok((player, bridge));
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "PipeWire virtual microphone did not become ready"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
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
            {"type":"PipeWire:Interface:Node","info":{"props":{"media.class":"Audio/Source","node.name":"virtual-mic","object.serial":2,"node.virtual":"true"}}},
            {"type":"PipeWire:Interface:Node","info":{"props":{"media.class":"Audio/Source","node.name":"desk-switch-microphone","object.serial":3}}},
            {"type":"PipeWire:Interface:Node","info":{"props":{"media.class":"Audio/Source","node.name":"sink.monitor","object.serial":4}}},
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
        assert_eq!(
            graph.sources.len(),
            1,
            "virtual/monitor inputs must never be microphone capture candidates"
        );
        assert_eq!(graph.sources[0].name, "mic");
        assert_eq!(graph.select(None).unwrap().serial, 20);
        assert_eq!(graph.select(Some("virtual")).unwrap().serial, 30);
        assert!(graph.select(Some("mic")).is_err());
        assert!(graph.select(Some("missing")).is_err());
    }
}
