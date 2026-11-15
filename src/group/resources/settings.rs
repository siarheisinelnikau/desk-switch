use super::{safe, sync, usb};
use crate::config;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
    pub workspace: String,
    pub peer: String,
    pub root: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::current_dir()
                .unwrap()
                .join("target/resource-fixtures")
                .join(new_id().unwrap());
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            assert!(
                self.0.is_absolute()
                    && self.0.starts_with(
                        std::env::current_dir()
                            .unwrap()
                            .join("target/resource-fixtures")
                    )
                    && self.0.file_name().unwrap().len() == 32
            );
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[test]
    fn saved_pairs_keep_baselines_conflict_choices_and_interrupted_plans() {
        let fixture = Fixture::new();
        let folder = Folder {
            id: new_id().unwrap(),
            name: "Synthetic documents".into(),
            workspace: "11".repeat(32),
            peer: "22".repeat(32),
            root: fixture.0.join("folder"),
        };
        let file = |bytes: &[u8]| sync::Node::File {
            size: bytes.len() as u64,
            sha256: safe::hash(bytes),
        };
        let base = BTreeMap::from([("document".into(), file(b"old"))]);
        let baseline = sync::Baseline {
            local: base.clone(),
            remote: base,
        };
        let local = BTreeMap::from([("document".into(), file(b"left"))]);
        let remote = BTreeMap::from([("document".into(), file(b"right"))]);
        let choices = BTreeMap::from([("document".into(), sync::Resolution::Local)]);
        let plan = sync::compare(local, remote, Some(&baseline), &choices).unwrap();
        let pair = Pair {
            id: new_id().unwrap(),
            name: "Documents".into(),
            local: folder.id.clone(),
            remote: new_id().unwrap(),
            baseline: Some(baseline),
            pending: Some(plan.clone()),
            run_id: new_id().unwrap(),
            choices,
            attempted: true,
        };
        Settings {
            format: 1,
            folders: vec![folder],
            pairs: vec![pair],
            usb: Vec::new(),
        }
        .save(&fixture.0)
        .unwrap();
        let loaded = Settings::load(&fixture.0).unwrap();
        assert!(loaded.pairs[0].attempted);
        assert_eq!(loaded.pairs[0].pending, Some(plan));
        assert_eq!(loaded.pairs[0].choices["document"], sync::Resolution::Local);
        assert!(loaded.pairs[0].baseline.is_some());
    }
    #[test]
    fn corrupt_settings_and_unknown_formats_are_not_overwritten() {
        let fixture = Fixture::new();
        let path = fixture.0.join("resources.json");
        std::fs::write(&path, b"{broken").unwrap();
        assert!(Settings::load(&fixture.0).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{broken");
        let bad = Settings {
            format: 999,
            ..Settings::default()
        };
        assert!(bad.save(&fixture.0).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{broken");
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Pair {
    pub id: String,
    pub name: String,
    pub local: String,
    pub remote: String,
    pub baseline: Option<sync::Baseline>,
    pub pending: Option<sync::Plan>,
    pub run_id: String,
    pub choices: BTreeMap<String, sync::Resolution>,
    pub attempted: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UsbGrant {
    pub workspace: String,
    pub grant: usb::Grant,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Settings {
    pub format: u16,
    pub folders: Vec<Folder>,
    pub pairs: Vec<Pair>,
    pub usb: Vec<UsbGrant>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            format: 1,
            folders: Vec::new(),
            pairs: Vec::new(),
            usb: Vec::new(),
        }
    }
}
pub fn new_id() -> Result<String> {
    Ok(config::hex(
        &snow::Builder::new(config::NOISE.parse()?)
            .generate_keypair()?
            .public[..16],
    ))
}
pub fn name(value: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control),
        "Enter a short folder pair name"
    );
    Ok(())
}
impl Settings {
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join("resources.json");
        let value = match std::fs::read(&path) {
            Ok(bytes) => {
                ensure!(
                    bytes.len() <= 16 * 1024 * 1024,
                    "Folder/USB settings are too large"
                );
                serde_json::from_slice(&bytes)
                    .context("Folder/USB settings are damaged; the original file was retained")?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => return Err(error.into()),
        };
        Self::validate(&value)?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.format == 1
                && self.folders.len() <= 64
                && self.pairs.len() <= 64
                && self.usb.len() <= 64,
            "Unsupported folder/USB settings"
        );
        let mut ids = BTreeSet::new();
        for folder in &self.folders {
            safe::valid_id(&folder.id)?;
            name(&folder.name)?;
            config::key(&folder.peer)?;
            ensure!(
                ids.insert(&folder.id) && folder.root.is_absolute() && !folder.workspace.is_empty(),
                "Invalid saved folder grant"
            );
        }
        for pair in &self.pairs {
            safe::valid_id(&pair.id)?;
            safe::valid_id(&pair.remote)?;
            name(&pair.name)?;
            safe::valid_id(&pair.run_id)?;
            ensure!(
                ids.insert(&pair.id) && self.folders.iter().any(|f| f.id == pair.local),
                "Invalid saved folder pair"
            );
            if let Some(base) = &pair.baseline {
                sync::validate(&base.local)?;
                sync::validate(&base.remote)?;
            }
            if let Some(plan) = &pair.pending {
                sync::validate(&plan.local)?;
                sync::validate(&plan.remote)?;
                ensure!(
                    plan.operations.len() <= safe::MAX_ENTRIES * 2,
                    "Saved sync plan is too large"
                );
                for op in &plan.operations {
                    op.validate()?;
                }
            }
        }
        for grant in &self.usb {
            usb::bus_id(&grant.grant.device.bus_id)?;
            config::key(&grant.grant.peer)?;
            ensure!(
                safe::valid_hash(&grant.grant.device.fingerprint)
                    && safe::valid_hash(&grant.grant.device.descriptor),
                "Invalid saved USB grant"
            );
        }
        Ok(())
    }
    pub fn save(&self, dir: &Path) -> Result<()> {
        self.validate()?;
        let bytes = serde_json::to_string(self)?;
        ensure!(
            bytes.len() <= 16 * 1024 * 1024,
            "Folder/USB settings are too large"
        );
        config::write_atomic(&dir.join("resources.json"), &bytes)
    }
}
