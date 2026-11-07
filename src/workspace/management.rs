//! Stable, authenticated application management, independent of device compatibility.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

pub const PROTOCOL: u16 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Application {
    pub theme: String,
    pub language: String,
    pub autostart: bool,
    pub auto_connect: bool,
    pub auto_update: bool,
    #[serde(default = "enabled")]
    pub show_remote_cursor: bool,
}
fn enabled() -> bool {
    true
}
impl Application {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            ["system", "light", "dark"].contains(&self.theme.as_str()),
            "invalid theme"
        );
        ensure!(
            crate::i18n::LANGUAGES
                .iter()
                .any(|(code, _)| *code == self.language),
            "invalid language"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Update {
    pub version: Option<String>,
    pub channel_version: Option<String>,
    pub ready: bool,
    pub checking: bool,
    pub installing: bool,
    pub failed: bool,
    pub status: String,
    pub progress: Option<crate::updater::Progress>,
}
impl Update {
    pub fn can_install(&self) -> bool {
        self.ready && self.version.is_some() && !self.checking && !self.installing
    }
    fn validate(&self) -> Result<()> {
        for version in [&self.version, &self.channel_version].into_iter().flatten() {
            stable_version(version)?;
        }
        ensure!(
            !self.ready || self.version.is_some(),
            "ready update has no version"
        );
        ensure!(
            self.status.len() <= 2048 && !self.status.chars().any(char::is_control),
            "invalid update status"
        );
        if let Some(progress) = &self.progress {
            ensure!(
                progress.total > 0
                    && progress.downloaded <= progress.total
                    && progress.total <= 128 * 1024 * 1024,
                "invalid download progress"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct State {
    pub application: Application,
    pub update: Update,
}
impl State {
    pub fn validate(&self) -> Result<()> {
        self.application.validate()?;
        self.update.validate()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Message {
    ExtractCable {
        id: u64,
        platform: crate::cable::Platform,
    },
    CheckUpdates {
        id: u64,
    },
    SetApplication {
        id: u64,
        expected: Application,
        application: Application,
    },
    Reply {
        id: u64,
        error: Option<String>,
    },
}
impl Message {
    pub fn id(&self) -> u64 {
        match self {
            Self::ExtractCable { id, .. }
            | Self::CheckUpdates { id }
            | Self::SetApplication { id, .. }
            | Self::Reply { id, .. } => *id,
        }
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(self.id() > 0, "invalid management request");
        match self {
            Self::SetApplication {
                expected,
                application,
                ..
            } => {
                expected.validate()?;
                application.validate()?;
            }
            Self::Reply {
                error: Some(error), ..
            } => {
                ensure!(
                    error.len() <= 1024 && !error.chars().any(char::is_control),
                    "invalid management reply"
                );
            }
            _ => {}
        }
        Ok(())
    }
}

pub fn stable_version(version: &str) -> Result<()> {
    ensure!(version.len() <= 48, "invalid update version");
    let version = semver::Version::parse(version)?;
    ensure!(
        version.pre.is_empty() && version.build.is_empty(),
        "a stable numbered version is required"
    );
    Ok(())
}

pub fn status_text(mut text: String, limit: usize) -> String {
    text.retain(|c| !c.is_control());
    text.truncate(text.floor_char_boundary(limit));
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn application() -> Application {
        Application {
            theme: "dark".into(),
            language: "en".into(),
            autostart: true,
            auto_connect: true,
            auto_update: false,
            show_remote_cursor: true,
        }
    }
    #[test]
    fn application_requests_are_bounded_and_do_not_accept_commands_or_paths() {
        let mut request = Message::SetApplication {
            id: 1,
            expected: application(),
            application: application(),
        };
        assert!(request.validate().is_ok());
        if let Message::SetApplication { application, .. } = &mut request {
            application.theme = "../../command".into();
        }
        assert!(request.validate().is_err());
        assert!(Message::CheckUpdates { id: 0 }.validate().is_err());
        assert!(
            Message::Reply {
                id: 1,
                error: Some("line\nbreak".into())
            }
            .validate()
            .is_err()
        );
    }
    #[test]
    fn remote_installer_action_requires_a_ready_version_and_no_active_job() {
        let mut update = Update {
            ready: true,
            ..Default::default()
        };
        assert!(!update.can_install());
        assert!(update.validate().is_err());
        update.version = Some("0.5.0".into());
        assert!(update.can_install());
        update.installing = true;
        assert!(!update.can_install());
        update.version = Some("99.0.0-alpha".into());
        assert!(update.validate().is_err());
    }
}
