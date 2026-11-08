use crate::config::{self, CaptureSettings, EdgeSettings, FileSettings, MonitorSettings};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Preferences {
    pub language: String,
    pub theme: String,
    pub discovery: bool,
    pub autostart: bool,
    pub auto_connect: bool,
    pub auto_audio: bool,
    pub auto_update: bool,
    pub show_remote_cursor: bool,
    pub role: String,
    pub address: String,
    pub audio_mode: String,
    pub audio_address: String,
    pub audio_source: String,
    pub audio_output: String,
    pub volume: u8,
    pub muted: bool,
    pub native_volume: u8,
    pub native_muted: bool,
    pub workspace: crate::workspace::Preferences,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            language: "en".into(),
            theme: "dark".into(),
            discovery: false,
            autostart: true,
            auto_connect: true,
            auto_audio: false,
            auto_update: true,
            show_remote_cursor: true,
            role: if cfg!(windows) { "host" } else { "receiver" }.into(),
            address: "192.168.1.20:45876".into(),
            audio_mode: "receive".into(),
            audio_address: "192.168.1.20:45877".into(),
            audio_source: String::new(),
            audio_output: String::new(),
            volume: 50,
            muted: false,
            native_volume: 100,
            native_muted: false,
            workspace: crate::workspace::Preferences::default(),
        }
    }
}

impl Preferences {
    pub fn load(dir: &Path) -> Result<Self> {
        match std::fs::read_to_string(dir.join("desktop.toml")) {
            Ok(data) => {
                let mut settings: Self = toml::from_str(&data)?;
                if toml::from_str::<toml::Value>(&data)?
                    .get("workspace")
                    .is_none()
                {
                    settings.workspace.shared.volume = settings.volume;
                    settings.workspace.shared.muted = settings.muted;
                }
                ensure!(
                    settings.volume <= 100
                        && settings.native_volume <= 100
                        && settings.workspace.shared.volume <= 100
                        && settings
                            .workspace
                            .shared
                            .audio_levels
                            .values()
                            .all(|v| v.volume <= 100),
                    "audio volume must be between 0 and 100"
                );
                config::Role::parse(&settings.role)?;
                ensure!(
                    ["send", "receive"].contains(&settings.audio_mode.as_str()),
                    "invalid audio mode"
                );
                Ok(settings)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        config::write_atomic(&dir.join("desktop.toml"), &toml::to_string_pretty(self)?)
    }

    pub fn input_args(&self) -> Result<Vec<String>> {
        config::Role::parse(&self.role)?;
        let address = crate::parse_address(&self.address)?;
        if self.role == "host" {
            ensure!(
                !address.ip().is_unspecified(),
                "укажите адрес принимающего компьютера"
            );
        }
        Ok(vec![
            "run".into(),
            "--role".into(),
            self.role.clone(),
            address.to_string(),
        ])
    }

    pub fn audio_args(&self) -> Result<Vec<String>> {
        let address = crate::parse_address(&self.audio_address)?;
        let mut args = vec!["audio".into()];
        if self.audio_mode == "send" {
            args.extend(["serve".into(), address.to_string()]);
            if !self.audio_source.trim().is_empty() {
                args.extend(["--source".into(), self.audio_source.clone()]);
            }
        } else {
            ensure!(
                !address.ip().is_unspecified(),
                "укажите адрес компьютера, передающего звук"
            );
            args.extend([
                "play".into(),
                address.to_string(),
                "--volume".into(),
                self.volume.to_string(),
            ]);
            if self.muted {
                args.push("--mute".into());
            }
            // Adopt the actual output volume on startup, including media-key changes.
            if !self.audio_output.trim().is_empty() {
                args.extend(["--output".into(), self.audio_output.clone()]);
            }
        }
        Ok(args)
    }
}

pub struct Form {
    pub monitor_enabled: bool,
    pub monitor_id: String,
    pub host_code: String,
    pub peer_code: String,
    pub edge_mode: String,
    pub edge_screen: String,
    pub edge_side: String,
    pub capture_devices: String,
    pub capture_keyboard: Option<PathBuf>,
    pub capture_mouse: Option<PathBuf>,
    pub sharing: config::InputSharing,
}

impl Form {
    pub fn from_document(settings: FileSettings) -> Self {
        let monitor_enabled = settings.monitor.is_some();
        let monitor = settings.monitor;
        let edge = settings.edge;
        Self {
            monitor_enabled,
            monitor_id: monitor.as_ref().map(|m| m.id.clone()).unwrap_or_default(),
            host_code: monitor
                .as_ref()
                .map(|m| format!("{:02X}", m.host))
                .unwrap_or_default(),
            peer_code: monitor
                .as_ref()
                .map(|m| format!("{:02X}", m.peer))
                .unwrap_or_default(),
            edge_mode: edge
                .as_ref()
                .map(|e| e.mode.clone())
                .unwrap_or_else(|| "off".into()),
            edge_screen: edge
                .as_ref()
                .and_then(|e| e.screen.clone())
                .unwrap_or_default(),
            edge_side: edge
                .as_ref()
                .and_then(|e| e.side.clone())
                .unwrap_or_else(|| "right".into()),
            capture_devices: settings
                .capture
                .as_ref()
                .map(|c| {
                    c.devices
                        .iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default(),
            capture_keyboard: settings.capture.as_ref().and_then(|c| c.keyboard.clone()),
            capture_mouse: settings.capture.as_ref().and_then(|c| c.mouse.clone()),
            sharing: settings.capture.map(|c| c.sharing).unwrap_or_default(),
        }
    }

    pub fn document(&self) -> Result<FileSettings> {
        let monitor = if self.monitor_enabled {
            Some(MonitorSettings {
                id: self.monitor_id.clone(),
                host: input_code(&self.host_code).context("вход компьютера с клавиатурой")?,
                peer: input_code(&self.peer_code).context("вход второго компьютера")?,
            })
        } else {
            None
        };
        let edge = Some(EdgeSettings {
            mode: self.edge_mode.clone(),
            screen: (!self.edge_screen.trim().is_empty()).then(|| self.edge_screen.clone()),
            side: Some(self.edge_side.clone()),
        });
        let devices = self
            .capture_devices
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        let capture = Some(CaptureSettings {
            devices,
            keyboard: self.capture_keyboard.clone(),
            mouse: self.capture_mouse.clone(),
            sharing: self.sharing,
        });
        Ok(FileSettings {
            routes: None,
            monitor,
            edge,
            capture,
        })
    }
}

pub fn input_code(value: &str) -> Result<u16> {
    let value = value.trim();
    ensure!(!value.is_empty(), "укажите код входа монитора");
    u16::from_str_radix(
        value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))
            .unwrap_or(value),
        16,
    )
    .context("код VCP должен быть шестнадцатеричным, например 11 или 0F")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_preferences_deserialize_with_new_appearance_defaults() {
        let prefs: Preferences =
            toml::from_str("role='receiver'\naddress='192.168.1.20:45876'\nvolume=25\nmuted=true")
                .unwrap();
        assert!(prefs.autostart && prefs.auto_connect && prefs.auto_update);
        assert!(!prefs.auto_audio);
        assert!(!prefs.discovery);
        assert_eq!(prefs.theme, "dark");
        assert_eq!(prefs.language, "en");
        assert_eq!(prefs.volume, 25);
        assert!(prefs.muted);
    }

    #[test]
    fn loading_an_old_profile_preserves_language_theme_and_audio_mix() {
        let dir = std::env::temp_dir().join(format!(
            "desk-switch-prefs-migration-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("desktop.toml"),
            "language='ru'\ntheme='light'\nvolume=25\nmuted=true",
        )
        .unwrap();
        let prefs = Preferences::load(&dir).unwrap();
        assert_eq!(
            (prefs.language.as_str(), prefs.theme.as_str()),
            ("ru", "light")
        );
        assert_eq!(prefs.workspace.shared.volume, 25);
        assert!(prefs.workspace.shared.muted);
        prefs.save(&dir).unwrap();
        let saved = Preferences::load(&dir).unwrap();
        assert_eq!(saved.workspace.shared.volume, 25);
        std::fs::remove_dir_all(dir).unwrap();
        let fresh = Preferences::default();
        assert_eq!(
            (fresh.language.as_str(), fresh.theme.as_str()),
            ("en", "dark")
        );
    }

    #[test]
    fn launch_arguments_preserve_literal_device_names_and_role() {
        let prefs = Preferences {
            role: "receiver".into(),
            address: "0.0.0.0:45876".into(),
            audio_output: "Device with spaces & punctuation".into(),
            muted: true,
            ..Default::default()
        };
        assert_eq!(
            prefs.input_args().unwrap(),
            ["run", "--role", "receiver", "0.0.0.0:45876"]
        );
        let args = prefs.audio_args().unwrap();
        assert!(args.contains(&"--mute".to_owned()));
        assert_eq!(args.last().unwrap(), "Device with spaces & punctuation");
    }

    #[test]
    fn unknown_monitor_ports_are_left_for_setup() {
        let form = Form::from_document(FileSettings::default());
        assert!(!form.monitor_enabled && form.host_code.is_empty() && form.peer_code.is_empty());
        assert_eq!(input_code("0x11").unwrap(), 17);
        assert_eq!(input_code("0F").unwrap(), 15);
        assert!(input_code("").is_err());
    }
}
