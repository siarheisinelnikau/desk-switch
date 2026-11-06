use super::{Backend, Session, State};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::Read,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(super) struct Native {
    inputs: BTreeMap<String, (u32, State)>,
}
impl Native {
    pub fn open() -> Result<Self> {
        Ok(Self {
            inputs: BTreeMap::new(),
        })
    }
}

// No shell, bounded output and a bounded server wait. Sink-input indices are
// monotonic server stream IDs, rather than reusable PipeWire global node IDs.
fn pactl(args: &[String]) -> Result<Vec<u8>> {
    let mut child = Command::new("pactl")
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("starting pactl; install pulseaudio-utils")?;
    let output = child.stdout.take().unwrap();
    let error = child.stderr.take().unwrap();
    let read = |stream: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut data = Vec::new();
            stream
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut data)
                .map(|_| data)
        })
    };
    let output = read(Box::new(output));
    let error = read(Box::new(error));
    let until = Instant::now() + Duration::from_secs(2);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let output = output
        .join()
        .map_err(|_| anyhow::anyhow!("pactl output reader stopped"))??;
    let error = error
        .join()
        .map_err(|_| anyhow::anyhow!("pactl error reader stopped"))??;
    ensure!(
        output.len() <= 4 * 1024 * 1024,
        "pactl response is too large"
    );
    ensure!(
        status.is_some_and(|s| s.success()),
        "pactl failed or timed out: {}",
        String::from_utf8_lossy(&error)
            .chars()
            .take(512)
            .collect::<String>()
    );
    Ok(output)
}

fn sessions(data: &[u8]) -> Result<Vec<(u32, Session)>> {
    let inputs: Vec<Value> =
        serde_json::from_slice(data).context("reading audio application volumes")?;
    ensure!(inputs.len() <= 1024, "too many audio applications");
    let mut result = Vec::new();
    for input in inputs {
        if input["has_volume"] == false
            || input["volume_writable"] == false
            || !input["volume"].is_object()
        {
            continue;
        }
        let Some(index) = input["index"].as_u64().and_then(|v| u32::try_from(v).ok()) else {
            continue;
        };
        let props = &input["properties"];
        let name = props["application.name"].as_str().unwrap_or_default();
        let serial = props["object.serial"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| props["object.serial"].as_u64().map(|v| v.to_string()))
            .unwrap_or_default();
        let pid = props["application.process.id"].as_str().unwrap_or_default();
        let ours = name == "Desk Switch" || pid == std::process::id().to_string();
        let group = props["module-stream-restore.id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "{}:{}:{}",
                    props["application.id"].as_str().unwrap_or(name),
                    props["application.process.binary"]
                        .as_str()
                        .unwrap_or_default(),
                    props["media.role"].as_str().unwrap_or_default()
                )
            });
        let Some(channels) = input["channel_map"].as_str() else {
            continue;
        };
        let channels = channels
            .split(',')
            .map(|name| {
                input["volume"][name.trim()]["value"]
                    .as_u64()
                    .map(|value| (value as f64 / 65536.0).powi(3))
            })
            .collect::<Option<Vec<_>>>();
        let Some(channels) = channels else { continue }; // Encoded/passthrough stream.
        let state = State {
            channels,
            muted: input["mute"]
                .as_bool()
                .context("missing audio mute state")?,
        };
        ensure!(
            state.valid() && group.len() <= 4096 && serial.len() <= 128 && pid.len() <= 32,
            "invalid audio application state"
        );
        result.push((
            index,
            Session {
                id: format!("{index}:{serial}:{pid}"),
                group,
                state,
                ours,
            },
        ));
    }
    Ok(result)
}

impl Backend for Native {
    fn sessions(&mut self) -> Result<Vec<Session>> {
        let sessions = sessions(&pactl(&[
            "--format=json".into(),
            "list".into(),
            "sink-inputs".into(),
        ])?)?;
        self.inputs = sessions
            .iter()
            .map(|(index, session)| (session.id.clone(), (*index, session.state.clone())))
            .collect();
        Ok(sessions.into_iter().map(|(_, session)| session).collect())
    }
    fn apply(&mut self, id: &str, state: &State) -> Result<()> {
        let (index, previous) = self
            .inputs
            .get(id)
            .cloned()
            .context("audio application disappeared")?;
        let result = (|| -> Result<()> {
            if !state.same_volume(&previous) {
                let mut args = vec!["set-sink-input-volume".into(), index.to_string()];
                args.extend(
                    state
                        .channels
                        .iter()
                        .map(|v| (v.cbrt() * 65536.0).round().to_string()),
                );
                pactl(&args)?;
            }
            if state.muted != previous.muted {
                pactl(&[
                    "set-sink-input-mute".into(),
                    index.to_string(),
                    if state.muted { "1" } else { "0" }.into(),
                ])?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            if self.sessions().is_ok() && !self.inputs.contains_key(id) {
                return Ok(());
            }
            return Err(error);
        }
        self.inputs.insert(id.into(), (index, state.clone()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pulse_streams_keep_channel_balance_and_exclude_shared_playback() {
        let data = br#"[
          {"index":42,"channel_map":"front-right,front-left","mute":false,"volume":{"front-left":{"value":65536},"front-right":{"value":32768}},"properties":{"application.name":"Browser","application.process.id":"12","object.serial":"700"}},
          {"index":43,"channel_map":"front-left","mute":false,"volume":{"front-left":{"value":65536}},"properties":{"application.name":"Desk Switch"}},
          {"index":44,"channel_map":"front-left","mute":false,"volume":{},"properties":{"application.name":"Encoded stream"}}
        ]"#;
        let sessions = sessions(data).unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].1.state.channels, [0.125, 1.0]);
        assert!(!sessions[0].1.ours);
        assert!(sessions[1].1.ours);
        assert_eq!(sessions[0].1.id, "42:700:12");
    }
}
