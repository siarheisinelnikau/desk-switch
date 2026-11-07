use super::Backend;
use crate::workspace::AudioLevel;
use anyhow::{Context, Result, ensure};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Stdio},
    sync::mpsc,
    time::Duration,
};

pub(super) struct Native {
    child: Child,
    commands: ChildStdin,
    responses: mpsc::Receiver<std::result::Result<AudioLevel, String>>,
}
impl Native {
    pub fn open(output: Option<&str>) -> Result<Self> {
        let mut child = crate::macos_native::command("volume-server")?
            .arg(output.unwrap_or("-"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let commands = child
            .stdin
            .take()
            .context("The system-volume command pipe is unavailable")?;
        let output = child.stdout.take().unwrap();
        let errors = child.stderr.take().unwrap();
        let (send, responses) = mpsc::sync_channel(1);
        let (ready, receive) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let response = line.map_err(|error| error.to_string()).and_then(|line| {
                    let value: serde_json::Value =
                        serde_json::from_str(&line).map_err(|error| error.to_string())?;
                    if let Some(error) = value["error"].as_str() {
                        return Err(error.into());
                    }
                    serde_json::from_value(value).map_err(|error| error.to_string())
                });
                if send.send(response).is_err() {
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            let mut first = true;
            for line in BufReader::new(errors).lines() {
                let Ok(line) = line else { break };
                if first {
                    first = false;
                    let success = serde_json::from_str::<serde_json::Value>(&line)
                        .is_ok_and(|value| value["ready"].is_string());
                    let _ = ready.send(if success { Ok(()) } else { Err(line) });
                } else {
                    eprintln!("macOS output volume: {line}");
                }
            }
        });
        match receive.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self {
                child,
                commands,
                responses,
            }),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!(
                    "{}",
                    match result {
                        Ok(Err(error)) => error,
                        _ => "The system-volume helper did not become ready".into(),
                    }
                );
            }
        }
    }
    fn change(&mut self, volume: Option<u8>, muted: Option<bool>) -> Result<AudioLevel> {
        serde_json::to_writer(
            &mut self.commands,
            &serde_json::json!({"volume":volume.map(|value|value.to_string()).unwrap_or_else(||"-".into()),"muted":muted.map(|value|value.to_string()).unwrap_or_else(||"-".into())}),
        )?;
        self.commands.write_all(b"\n")?;
        self.commands.flush()?;
        match self.responses.recv_timeout(Duration::from_secs(2)) {
            Ok(result) => result.map_err(anyhow::Error::msg),
            Err(error) => {
                let _ = self.child.kill();
                anyhow::bail!("The system-volume helper stopped responding: {error}");
            }
        }
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Backend for Native {
    fn read(&mut self) -> Result<AudioLevel> {
        self.change(None, None)
    }
    fn set_volume(&mut self, volume: u8) -> Result<()> {
        ensure!(volume <= 100, "Volume must be between 0 and 100");
        self.change(Some(volume), None)?;
        Ok(())
    }
    fn set_muted(&mut self, muted: bool) -> Result<()> {
        self.change(None, Some(muted))?;
        Ok(())
    }
}
