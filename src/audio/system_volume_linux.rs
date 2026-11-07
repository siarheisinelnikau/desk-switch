use super::super::pulse::pactl;
use super::Backend;
use crate::workspace::AudioLevel;
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
struct Channel {
    value: u32,
}
#[derive(Deserialize)]
struct Sink {
    name: String,
    mute: bool,
    volume: BTreeMap<String, Channel>,
    channel_map: String,
}
impl Sink {
    fn channels(&self) -> Result<Vec<u32>> {
        let channels = self
            .channel_map
            .split(',')
            .map(|name| {
                self.volume
                    .get(name.trim())
                    .map(|channel| channel.value)
                    .context("missing output channel volume")
            })
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            !channels.is_empty() && channels.len() <= 32,
            "invalid output channel count"
        );
        Ok(channels)
    }
}

pub(super) struct Native {
    output: String,
}
impl Native {
    pub fn open(output: Option<&str>) -> Result<Self> {
        let output = match output {
            Some(output) => output.to_owned(),
            None => String::from_utf8(pactl(&["get-default-sink".into()])?)?
                .trim()
                .to_owned(),
        };
        ensure!(
            !output.is_empty() && !output.starts_with('-') && output.len() <= 2048,
            "invalid system output"
        );
        Ok(Self { output })
    }

    fn sink(&self) -> Result<Sink> {
        let sinks: Vec<Sink> = serde_json::from_slice(&pactl(&[
            "--format=json".into(),
            "list".into(),
            "sinks".into(),
        ])?)?;
        ensure!(sinks.len() <= 1024, "too many audio outputs");
        sinks
            .into_iter()
            .find(|sink| sink.name == self.output)
            .context("the selected system output is unavailable")
    }
}

fn scaled_channels(channels: &[u32], volume: u8) -> Vec<String> {
    let maximum = channels.iter().copied().max().unwrap_or(0);
    channels
        .iter()
        .map(|&channel| {
            let value = if maximum == 0 {
                f64::from(volume) * 65536.0 / 100.0
            } else {
                f64::from(channel) / f64::from(maximum) * f64::from(volume) * 65536.0 / 100.0
            };
            (value.round() as u32).to_string()
        })
        .collect()
}

impl Backend for Native {
    fn read(&mut self) -> Result<AudioLevel> {
        let sink = self.sink()?;
        let maximum = sink.channels()?.into_iter().max().unwrap_or(0);
        Ok(AudioLevel {
            volume: (f64::from(maximum) / 65536.0 * 100.0)
                .round()
                .clamp(0.0, 100.0) as u8,
            muted: sink.mute,
        })
    }

    fn set_volume(&mut self, volume: u8) -> Result<()> {
        let channels = self.sink()?.channels()?;
        let mut args = vec!["set-sink-volume".into(), self.output.clone()];
        args.extend(scaled_channels(&channels, volume));
        pactl(&args)?;
        Ok(())
    }

    fn set_muted(&mut self, muted: bool) -> Result<()> {
        pactl(&[
            "set-sink-mute".into(),
            self.output.clone(),
            u8::from(muted).to_string(),
        ])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_output_is_pinned_and_option_like_ids_are_rejected() {
        assert_eq!(
            Native::open(Some("headphones with spaces")).unwrap().output,
            "headphones with spaces"
        );
        assert!(Native::open(Some("--invalid-output")).is_err());
        assert!(Native::open(Some("")).is_err());
    }
    #[test]
    fn system_volume_preserves_channel_order_and_balance() {
        let sink: Sink = serde_json::from_str(r#"{"name":"headphones","mute":true,"channel_map":"front-right,front-left","volume":{"front-left":{"value":65536},"front-right":{"value":32768}}}"#).unwrap();
        assert_eq!(
            scaled_channels(&sink.channels().unwrap(), 90),
            ["29491", "58982"]
        );
        assert_eq!(scaled_channels(&[0, 0], 50), ["32768", "32768"]);
        assert_eq!(scaled_channels(&[32768, 65536], 0), ["0", "0"]);
    }
}
