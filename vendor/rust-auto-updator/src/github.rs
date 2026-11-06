use crate::Config;
use semver::Version;
use serde::Deserialize;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Release {
    pub(crate) source: String,
    pub version: Version,
    pub notes: String,
    pub page: String,
    pub(crate) executable: Asset,
    pub(crate) checksum: Option<Asset>,
}

impl Release {
    pub fn executable_name(&self) -> &str {
        &self.executable.name
    }
    pub fn executable_url(&self) -> &str {
        &self.executable.browser_download_url
    }
    pub fn executable_size(&self) -> u64 {
        self.executable.size
    }
    pub fn size(&self) -> u64 {
        self.executable_size()
    }
    pub fn checksum_url(&self) -> Option<&str> {
        self.checksum
            .as_ref()
            .map(|asset| asset.browser_download_url.as_str())
    }
    pub fn sha256_digest(&self) -> Option<&str> {
        self.executable
            .digest
            .as_deref()
            .and_then(|value| value.strip_prefix("sha256:"))
    }
}

pub(crate) fn asset_digest(asset: &Asset) -> Result<Option<String>, String> {
    let Some(digest) = &asset.digest else {
        return Ok(None);
    };
    let digest = digest
        .strip_prefix("sha256:")
        .ok_or("The release asset has an invalid SHA-256 digest.")?;
    if digest.len() != 64 || !digest.bytes().all(|value| value.is_ascii_hexdigit()) {
        return Err("The release asset has an invalid SHA-256 digest.".into());
    }
    Ok(Some(digest.to_ascii_lowercase()))
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Asset {
    pub(crate) name: String,
    pub(crate) browser_download_url: String,
    pub(crate) size: u64,
    pub(crate) digest: Option<String>,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

pub(crate) fn agent(config: &Config) -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(5)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(120)))
        .timeout_per_read(Some(Duration::from_secs(20)))
        .user_agent(format!(
            "{}/{}",
            config.product_name, config.current_version
        ))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .new_agent()
}

pub(crate) fn read_text(agent: &ureq::Agent, url: &str, limit: u64) -> Result<String, String> {
    let mut response = agent
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(network_error)?;
    response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_string()
        .map_err(network_error)
}

pub(crate) fn network_error(e: ureq::Error) -> String {
    if matches!(e, ureq::Error::StatusCode(403 | 429)) {
        "GitHub's request limit was reached. Try again later.".into()
    } else {
        format!("Could not contact GitHub: {e}")
    }
}

pub fn check_latest(config: &Config) -> Result<Option<Release>, String> {
    config.validate()?;
    parse_release(
        &read_text(&agent(config), &config.api_url(), 1024 * 1024)?,
        config,
    )
}

pub fn parse_release(json: &str, config: &Config) -> Result<Option<Release>, String> {
    config.validate()?;
    let repo = config.repository_url();
    let release: ApiRelease = serde_json::from_str(json)
        .map_err(|e| format!("GitHub returned an invalid release: {e}"))?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let version = Version::parse(
        release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name),
    )
    .map_err(|_| "The release version is invalid.")?;
    if !version.pre.is_empty() || version.cmp_precedence(&config.current_version).is_le() {
        return Ok(None);
    }
    let tag = format!("v{version}");
    if release.tag_name != tag || release.html_url != format!("{repo}/releases/tag/{tag}") {
        return Err("The release does not belong to the expected GitHub repository.".into());
    }
    let asset = |name: &str| -> Result<Asset, String> {
        let mut matches = release.assets.iter().filter(|a| a.name == name);
        let asset = matches
            .next()
            .ok_or_else(|| format!("The release is missing {name}."))?;
        if matches.next().is_some()
            || asset.browser_download_url != format!("{repo}/releases/download/{tag}/{name}")
        {
            return Err("The release contains an unexpected download address.".into());
        }
        Ok(asset.clone())
    };
    let executable_name = config
        .executable_names(&version)
        .into_iter()
        .find(|name| {
            release
                .assets
                .iter()
                .any(|candidate| candidate.name == *name)
        })
        .ok_or_else(|| {
            format!(
                "The release is missing {}.",
                config.executable_name(&version)
            )
        })?;
    let executable = asset(&executable_name)?;
    if executable.size < 1024 || executable.size > config.max_executable_size {
        return Err("The release executable has an unexpected size.".into());
    }
    let checksum_name = config.checksum_filename(&executable_name);
    let checksum = if release
        .assets
        .iter()
        .any(|candidate| candidate.name == checksum_name)
    {
        Some(asset(&checksum_name)?)
    } else if config.allow_asset_digest && asset_digest(&executable)?.is_some() {
        None
    } else {
        return Err(format!("The release is missing {checksum_name}."));
    };
    // A supplied digest is always validated, even with a separate checksum file.
    asset_digest(&executable)?;
    Ok(Some(Release {
        source: repo.clone(),
        version,
        notes: release
            .body
            .unwrap_or_default()
            .chars()
            .take(12000)
            .collect(),
        page: release.html_url,
        executable,
        checksum,
    }))
}

#[cfg(test)]
mod package_tests {
    use super::*;
    fn fixture(config: &Config, version: &str) -> serde_json::Value {
        let name = config.executable_name(&version.parse().unwrap());
        serde_json::json!({"tag_name":format!("v{version}"),"html_url":format!("{}/releases/tag/v{version}",config.repository_url()),"draft":false,"prerelease":false,
            "assets":[{"name":name,"size":4096,"digest":format!("sha256:{}","ab".repeat(32)),"browser_download_url":format!("{}/releases/download/v{version}/{name}",config.repository_url())}]})
    }
    #[test]
    fn debian_discovery_obeys_version_repository_and_digest_contracts() {
        let mut config = Config::debian("owner", "app", "1.0.0", "sample-app").unwrap();
        config.allow_asset_digest = true;
        for version in ["0.9.0", "1.0.0", "1.0.0+build"] {
            assert!(
                parse_release(&fixture(&config, version).to_string(), &config)
                    .unwrap()
                    .is_none()
            );
        }
        let good = fixture(&config, "1.1.0");
        assert!(parse_release(&good.to_string(), &config).unwrap().is_some());
        for field in ["draft", "prerelease"] {
            let mut value = good.clone();
            value[field] = true.into();
            assert!(
                parse_release(&value.to_string(), &config)
                    .unwrap()
                    .is_none()
            );
        }
        let mut value = good.clone();
        value["assets"][0]["digest"] = "sha256:bad".into();
        assert!(parse_release(&value.to_string(), &config).is_err());
        let mut value = good.clone();
        value["assets"][0]["browser_download_url"] = "https://example.invalid/update.deb".into();
        assert!(parse_release(&value.to_string(), &config).is_err());
        let mut value = good.clone();
        value["assets"][0]["name"] = "another-app_1.1.0_amd64.deb".into();
        assert!(parse_release(&value.to_string(), &config).is_err());
        let mut value = good.clone();
        value["assets"]
            .as_array_mut()
            .unwrap()
            .push(good["assets"][0].clone());
        assert!(parse_release(&value.to_string(), &config).is_err());
        config.allow_asset_digest = false;
        assert!(parse_release(&good.to_string(), &config).is_err());
    }
}

#[cfg(test)]
mod filename_migration_tests {
    use super::*;
    fn release(names: &[&str]) -> serde_json::Value {
        serde_json::json!({"tag_name":"v2.0.0", "html_url":"https://github.com/owner/app/releases/tag/v2.0.0",
            "draft":false,"prerelease":false,"assets":names.iter().map(|name| serde_json::json!({
                "name":name,"size":4096,"digest":format!("sha256:{}","ab".repeat(32)),
                "browser_download_url":format!("https://github.com/owner/app/releases/download/v2.0.0/{name}")
            })).collect::<Vec<_>>()})
    }
    #[test]
    fn both_platforms_choose_most_specific_present_name_independent_of_asset_order() {
        let windows = Config::new("owner", "app", "1.0.0", "App").unwrap();
        let mut ubuntu = Config::debian("owner", "app", "1.0.0", "sample-app").unwrap();
        ubuntu.product_name = "App".into();
        for (mut config, platform, names) in [
            (
                windows,
                "windows",
                vec![
                    "App-v2.0.0-windows-amd64.exe",
                    "App-v2.0.0-windows-x64.exe",
                    "App-v2.0.0-windows-x86_64.exe",
                    "App-windows-amd64.exe",
                    "App-windows-x64.exe",
                    "App-windows-x86_64.exe",
                    "App-v2.0.0-amd64.exe",
                    "App-v2.0.0-x64.exe",
                    "App-v2.0.0-x86_64.exe",
                    "App-amd64.exe",
                    "App-x64.exe",
                    "App-x86_64.exe",
                    "App-v2.0.0-windows.exe",
                    "App-windows.exe",
                    "App-v2.0.0.exe",
                    "App.exe",
                ],
            ),
            (
                ubuntu,
                "ubuntu",
                vec![
                    "App-v2.0.0-ubuntu-amd64.deb",
                    "App-v2.0.0-ubuntu-x64.deb",
                    "App-v2.0.0-ubuntu-x86_64.deb",
                    "App-ubuntu-amd64.deb",
                    "App-ubuntu-x64.deb",
                    "App-ubuntu-x86_64.deb",
                    "App-v2.0.0-amd64.deb",
                    "App-v2.0.0-x64.deb",
                    "App-v2.0.0-x86_64.deb",
                    "App-amd64.deb",
                    "App-x64.deb",
                    "App-x86_64.deb",
                    "sample-app_2.0.0_amd64.deb",
                    "App-v2.0.0-ubuntu.deb",
                    "App-ubuntu.deb",
                    "App-v2.0.0.deb",
                    "App.deb",
                ],
            ),
        ] {
            config.architecture = crate::Architecture::X64;
            config.asset_platform = Some(platform.into());
            config.allow_asset_digest = true;
            config.legacy_executable_names = vec![names.last().unwrap().to_string()];
            assert_eq!(config.executable_names(&"2.0.0".parse().unwrap()), names);
            for first in 0..names.len() {
                let shuffled: Vec<_> = names[first..].iter().copied().rev().collect();
                let parsed = parse_release(&release(&shuffled).to_string(), &config)
                    .unwrap()
                    .unwrap();
                assert_eq!(parsed.executable_name(), names[first]);
            }
            let mut damaged = release(&names);
            damaged["assets"][0]["digest"] = "sha256:bad".into();
            assert!(parse_release(&damaged.to_string(), &config).is_err());
            let mut duplicate = release(&names);
            duplicate["assets"]
                .as_array_mut()
                .unwrap()
                .push(release(&names)["assets"][0].clone());
            assert!(parse_release(&duplicate.to_string(), &config).is_err());
            let foreign = release(&[
                "App-v2.0.0-windows-arm64.exe",
                "App-v2.0.0-ubuntu-arm64.deb",
                "Other.exe",
                "Other.deb",
            ]);
            assert!(parse_release(&foreign.to_string(), &config).is_err());
        }
    }
    #[test]
    fn unversioned_architecture_aliases_select_the_current_build_not_another_cpu() {
        for (architecture, aliases, wrong) in [
            (
                crate::Architecture::X64,
                vec!["amd64", "x64", "x86_64"],
                vec!["i386", "x86", "i686", "arm64", "aarch64"],
            ),
            (
                crate::Architecture::X86,
                vec!["i386", "x86", "i686"],
                vec!["amd64", "x64", "x86_64", "arm64", "aarch64"],
            ),
            (
                crate::Architecture::Arm64,
                vec!["arm64", "aarch64"],
                vec!["amd64", "x64", "x86_64", "i386", "x86", "i686"],
            ),
        ] {
            for platform in ["windows", "ubuntu"] {
                let (mut config, extension) = if platform == "windows" {
                    (Config::new("owner", "app", "1.0.0", "App").unwrap(), "exe")
                } else {
                    (
                        Config::debian("owner", "app", "1.0.0", "sample-app").unwrap(),
                        "deb",
                    )
                };
                config.product_name = "App".into();
                config.asset_platform = Some(platform.into());
                config.architecture = architecture;
                config.allow_asset_digest = true;
                let foreign: Vec<_> = wrong
                    .iter()
                    .flat_map(|arch| {
                        [
                            format!("App-{arch}.{extension}"),
                            format!("App-{platform}-{arch}.{extension}"),
                        ]
                    })
                    .collect();
                let foreign: Vec<_> = foreign.iter().map(String::as_str).collect();
                assert!(parse_release(&release(&foreign).to_string(), &config).is_err());
                for alias in &aliases {
                    for name in [
                        format!("App-{alias}.{extension}"),
                        format!("App-{platform}-{alias}.{extension}"),
                        format!("App-v2.0.0-{platform}-{alias}.{extension}"),
                    ] {
                        let generic = format!("App-{platform}.{extension}");
                        let mut candidates = foreign.clone();
                        candidates.extend([generic.as_str(), name.as_str()]);
                        let parsed = parse_release(&release(&candidates).to_string(), &config)
                            .unwrap()
                            .unwrap();
                        assert_eq!(parsed.executable_name(), name);
                    }
                }
                for name in [
                    format!("App-{platform}.{extension}"),
                    format!("App.{extension}"),
                ] {
                    assert_eq!(
                        parse_release(&release(&[&name]).to_string(), &config)
                            .unwrap()
                            .unwrap()
                            .executable_name(),
                        name
                    );
                }
            }
        }
    }

    #[test]
    fn migration_preserves_existing_consumers_and_rejects_unsafe_or_cross_format_aliases() {
        let mut config = Config::debian("owner", "app", "1.0.0", "sample-app").unwrap();
        config.architecture = crate::Architecture::X64;
        assert_eq!(
            config.executable_name(&"2.0.0".parse().unwrap()),
            "sample-app_2.0.0_amd64.deb"
        );
        config.asset_platform = Some("ubuntu".into());
        config.architecture = crate::Architecture::Arm64;
        assert_eq!(
            config.executable_name(&"2.0.0".parse().unwrap()),
            "sample-app-v2.0.0-ubuntu-arm64.deb"
        );
        config.legacy_executable_names = vec!["old-{version}.deb".into()];
        assert!(config.validate().is_ok());
        for name in ["old.exe", "../old.deb", "old-{unknown}.deb"] {
            config.legacy_executable_names = vec![name.into()];
            assert!(config.validate().is_err());
        }
        config.legacy_executable_names.clear();
        for platform in ["", "../ubuntu", "Ubuntu", "ubuntu/{arch}"] {
            config.asset_platform = Some(platform.into());
            assert!(config.validate().is_err());
        }
    }
}
