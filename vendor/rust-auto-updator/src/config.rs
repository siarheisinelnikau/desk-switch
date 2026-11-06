use semver::Version;

/// Executable machine type or Debian package architecture to accept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Architecture {
    X64,
    X86,
    Arm64,
}

impl Architecture {
    pub(crate) fn machine(self) -> u16 {
        match self {
            Self::X64 => 0x8664,
            Self::X86 => 0x014c,
            Self::Arm64 => 0xaa64,
        }
    }
}

/// Installation mechanism. Windows keeps its portable replacement protocol;
/// Debian packages are installed by apt under the desktop's polkit prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallMode {
    WindowsPortable,
    DebianPackage { package: String },
}

impl Architecture {
    /// Filename aliases only; package metadata and PE machine checks stay exact.
    fn filename_aliases(self) -> &'static [&'static str] {
        match self {
            Self::X64 => &["amd64", "x64", "x86_64"],
            Self::X86 => &["i386", "x86", "i686"],
            Self::Arm64 => &["arm64", "aarch64"],
        }
    }

    pub fn debian_name(self) -> &'static str {
        match self {
            Self::X64 => "amd64",
            Self::X86 => "i386",
            Self::Arm64 => "arm64",
        }
    }
}

/// Host-specific restart behavior. No application flags are assumed.
#[derive(Clone, Debug)]
pub struct InstallOptions {
    pub restart_args: Vec<String>,
    pub rollback_args: Vec<String>,
    pub stage_prefix: String,
    pub helper_name: String,
    /// Wait for first-frame acknowledgement before discarding rollback files.
    /// A child that exits before acknowledgement is rolled back. A live child
    /// that reaches the timeout is left running with its backup preserved.
    pub startup_health_timeout: Option<std::time::Duration>,
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self {
            restart_args: Vec::new(),
            rollback_args: Vec::new(),
            stage_prefix: ".rust-auto-updator-".into(),
            helper_name: "update-helper.exe".into(),
            startup_health_timeout: None,
        }
    }
}

/// Product configuration. Create it with the consuming application's version,
/// never this library's Cargo version. Stable releases must use `vSEMVER` tags.
#[derive(Clone, Debug)]
pub struct Config {
    pub owner: String,
    pub repository: String,
    pub current_version: Version,
    pub product_name: String,
    pub architecture: Architecture,
    pub mode: InstallMode,
    /// Opt into platform-qualified assets, e.g. `windows` or `ubuntu`.
    /// Discovery tries platform/architecture names before historical names.
    /// None preserves the existing filename contract for other consumers.
    pub asset_platform: Option<String>,
    /// Optional PE subsystem restriction: 2 for Windows GUI, 3 for console.
    pub expected_subsystem: Option<u16>,
    /// Empty for one architecture; e.g. `-x64` for multi-architecture releases.
    pub asset_suffix: String,
    /// Ordered legacy suffixes to try only when the primary filename is absent.
    pub asset_suffixes: Vec<String>,
    /// Additional historical filenames; `{version}` expands to the release version.
    /// The canonical versioned product filename is always preferred.
    pub legacy_executable_names: Vec<String>,
    /// A safe filename, optionally containing `{executable}` for per-asset sums.
    pub checksum_name: String,
    /// Accept GitHub's SHA-256 asset digest when no checksum asset is supplied.
    /// If both are supplied they must agree; a malformed checksum never falls back.
    pub allow_asset_digest: bool,
    pub max_executable_size: u64,
    pub install: InstallOptions,
}

impl Config {
    pub fn new(
        owner: &str,
        repository: &str,
        version: &str,
        product: &str,
    ) -> Result<Self, String> {
        let config = Self {
            owner: owner.into(),
            repository: repository.into(),
            current_version: Version::parse(version).map_err(|e| e.to_string())?,
            product_name: product.into(),
            architecture: if cfg!(target_arch = "aarch64") {
                Architecture::Arm64
            } else if cfg!(target_arch = "x86") {
                Architecture::X86
            } else {
                Architecture::X64
            },
            mode: InstallMode::WindowsPortable,
            asset_platform: None,
            expected_subsystem: None,
            asset_suffix: String::new(),
            asset_suffixes: Vec::new(),
            legacy_executable_names: Vec::new(),
            checksum_name: "SHA256SUMS.txt".into(),
            allow_asset_digest: false,
            max_executable_size: 64 * 1024 * 1024,
            install: InstallOptions::default(),
        };
        config.validate()?;
        Ok(config)
    }

    /// Select canonical Debian assets named `package_VERSION_ARCH.deb`.
    pub fn debian(
        owner: &str,
        repository: &str,
        version: &str,
        package: &str,
    ) -> Result<Self, String> {
        let mut config = Self::new(owner, repository, version, "DebianPackage")?;
        config.product_name = package.replace('+', "-");
        config.mode = InstallMode::DebianPackage {
            package: package.into(),
        };
        config.install.helper_name = "update-helper".into();
        config.validate()?;
        Ok(config)
    }

    pub fn repository_url(&self) -> String {
        format!("https://github.com/{}/{}", self.owner, self.repository)
    }

    pub fn api_url(&self) -> String {
        format!(
            "https://api.github.com/repos/{}/{}/releases/latest",
            self.owner, self.repository
        )
    }

    fn asset_extension(&self) -> &'static str {
        match self.mode {
            InstallMode::WindowsPortable => "exe",
            InstallMode::DebianPackage { .. } => "deb",
        }
    }

    fn legacy_primary_name(&self, version: &Version) -> String {
        match &self.mode {
            InstallMode::WindowsPortable => {
                format!("{}-v{version}{}.exe", self.product_name, self.asset_suffix)
            }
            InstallMode::DebianPackage { package } => format!(
                "{package}_{version}_{}.deb",
                self.architecture.debian_name()
            ),
        }
    }

    pub fn executable_name(&self, version: &Version) -> String {
        match &self.asset_platform {
            Some(platform) => format!(
                "{}-v{version}-{platform}-{}.{}",
                self.product_name,
                self.architecture.debian_name(),
                self.asset_extension()
            ),
            None => self.legacy_primary_name(version),
        }
    }

    /// Exact names in preference order. Never glob or fall back after a chosen
    /// asset fails verification: that could select another platform or hide damage.
    pub fn executable_names(&self, version: &Version) -> Vec<String> {
        let mut names = Vec::new();
        let mut add = |name: String| {
            if !names.contains(&name) {
                names.push(name);
            }
        };
        add(self.executable_name(version));
        if let Some(platform) = &self.asset_platform {
            let product = &self.product_name;
            let extension = self.asset_extension();
            // Keep all matching architecture spellings ahead of names that omit
            // the architecture. This matters when 32- and 64-bit assets coexist.
            for prefix in [format!("{product}-v{version}"), product.clone()] {
                for architecture in self.architecture.filename_aliases() {
                    add(format!("{prefix}-{platform}-{architecture}.{extension}"));
                }
            }
            for prefix in [format!("{product}-v{version}"), product.clone()] {
                for architecture in self.architecture.filename_aliases() {
                    add(format!("{prefix}-{architecture}.{extension}"));
                }
            }
            if matches!(self.mode, InstallMode::DebianPackage { .. }) {
                add(self.legacy_primary_name(version));
            }
            add(format!("{product}-v{version}-{platform}.{extension}"));
            add(format!("{product}-{platform}.{extension}"));
        }
        add(self.legacy_primary_name(version));
        if matches!(self.mode, InstallMode::WindowsPortable) {
            for suffix in &self.asset_suffixes {
                add(format!("{}-v{version}{suffix}.exe", self.product_name));
            }
        } else if self.asset_platform.is_some() {
            add(format!("{}-v{version}.deb", self.product_name));
        }
        if self.asset_platform.is_some() {
            add(format!("{}.{}", self.product_name, self.asset_extension()));
        }
        for template in &self.legacy_executable_names {
            add(template.replace("{version}", &version.to_string()));
        }
        names
    }

    pub fn checksum_filename(&self, executable: &str) -> String {
        self.checksum_name.replace("{executable}", executable)
    }

    pub fn validate(&self) -> Result<(), String> {
        let safe = |text: &str| {
            !text.is_empty()
                && text != "."
                && text != ".."
                && text
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        };
        if let InstallMode::DebianPackage { package } = &self.mode
            && (package.len() < 2
                || !package.as_bytes()[0].is_ascii_alphanumeric()
                || !package
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"+.-".contains(&b))
                || self.expected_subsystem.is_some()
                || !self.asset_suffix.is_empty()
                || !self.asset_suffixes.is_empty())
        {
            return Err("Invalid Debian updater configuration: use a Debian package name and no Windows asset overrides.".into());
        }
        if self.asset_platform.as_ref().is_some_and(|platform| {
            platform.is_empty()
                || !platform
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        }) || !safe(&self.owner)
            || self.install.startup_health_timeout.is_some_and(|timeout| {
                timeout.is_zero() || timeout > std::time::Duration::from_secs(300)
            })
            || self
                .expected_subsystem
                .is_some_and(|value| ![2, 3].contains(&value))
            || !safe(&self.repository)
            || !safe(&self.product_name)
            || (!self.asset_suffix.is_empty() && !safe(&self.asset_suffix))
            || self
                .asset_suffixes
                .iter()
                .any(|suffix| !suffix.is_empty() && !safe(suffix))
            || self.legacy_executable_names.iter().any(|name| {
                !safe(&name.replace("{version}", "1.0.0"))
                    || !name.ends_with(&format!(".{}", self.asset_extension()))
            })
            || !safe(&self.checksum_filename("application.exe"))
            || self.max_executable_size < 1024
            || !safe(&self.install.stage_prefix)
            || !self.install.stage_prefix.starts_with('.')
            || !self.install.stage_prefix.ends_with('-')
            || !safe(&self.install.helper_name)
            || (self.mode == InstallMode::WindowsPortable
                && !self.install.helper_name.ends_with(".exe"))
            || ["ready.exe", "previous.exe", "restore.exe", "failed.exe"]
                .iter()
                .any(|name| self.install.helper_name.eq_ignore_ascii_case(name))
        {
            return Err("Invalid updater configuration: repository and staging names must be plain safe filenames, and the helper must have a distinct EXE name.".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_paths_and_installer_filename_collisions() {
        assert!(Config::new("../owner", "app", "1.0.0", "App").is_err());
        let mut config = Config::new("owner", "app", "1.0.0", "App").unwrap();
        config.install.helper_name = "previous.exe".into();
        assert!(config.validate().is_err());
        config.install.helper_name = "../escape.exe".into();
        assert!(config.validate().is_err());
    }
}

#[cfg(test)]
mod debian_tests {
    use super::*;
    #[test]
    fn debian_assets_are_architecture_specific_and_do_not_fall_back_to_executables() {
        let mut config = Config::debian("owner", "app", "1.0.0", "sample-app").unwrap();
        let version = "2.0.0".parse().unwrap();
        for (architecture, name) in [
            (Architecture::X64, "amd64"),
            (Architecture::Arm64, "arm64"),
            (Architecture::X86, "i386"),
        ] {
            config.architecture = architecture;
            assert_eq!(
                config.executable_names(&version),
                [format!("sample-app_2.0.0_{name}.deb")]
            );
        }
        config.legacy_executable_names.push("sample.exe".into());
        assert!(config.validate().is_err());
        assert!(Config::debian("owner", "app", "1.0.0", "../escape").is_err());
        assert!(Config::debian("owner", "app", "1.0.0", "BadName").is_err());
        assert!(Config::debian("owner", "app", "1.0.0", "sample+extra").is_ok());
    }
}
