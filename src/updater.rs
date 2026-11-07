use anyhow::{Context, Result, bail, ensure};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};
#[cfg(target_os = "macos")]
#[path = "updater_macos.rs"]
mod macos;

pub const CHANNEL: &str =
    "https://api.github.com/repos/siarheisinelnikau/desk-switch/releases/latest";
const SIGNED_START: &str = "<!-- desk-switch-update:start -->";
const SIGNED_END: &str = "<!-- desk-switch-update:end -->";
const PUBLIC_KEY: &str = include_str!("../release/update-public-key.hex");
const MAX_MANIFEST: u64 = 64 * 1024;
const MAX_BINARY: u64 = 128 * 1024 * 1024;
const WINDOWS: &str = "windows-x86_64";
const LINUX: &str = "linux-x86_64";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub payload: String,
    pub signature: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub version: String,
    pub protocol: u16,
    pub targets: BTreeMap<String, Artifact>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    envelope: Envelope,
    executable: PathBuf,
    payload: PathBuf,
}

pub enum Outcome {
    Current(String),
    Unpublished,
    Available(Candidate),
    Ready(String),
    Prepared(String, Prepared),
}

/// Signed release metadata retained between a check and an explicit download.
#[derive(Clone)]
pub struct Candidate {
    #[cfg(not(target_os = "macos"))]
    bytes: Vec<u8>,
    envelope: Envelope,
    release: Release,
}
impl Candidate {
    pub fn version(&self) -> &str {
        &self.release.version
    }
}

pub struct Prepared {
    #[cfg(not(target_os = "macos"))]
    inner: rust_auto_updator::Prepared,
    #[cfg(target_os = "macos")]
    macos: macos::Prepared,
    #[cfg(target_os = "linux")]
    envelope: Envelope,
}

fn hex<const N: usize>(value: &str) -> Result<[u8; N]> {
    let value = value.trim();
    ensure!(
        value.len() == N * 2 && value.is_ascii(),
        "invalid signature/hash encoding"
    );
    let mut bytes = [0; N];
    for (index, item) in bytes.iter_mut().enumerate() {
        *item = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)?;
    }
    Ok(bytes)
}

fn verified(envelope: &Envelope, public_key: &str) -> Result<Release> {
    ensure!(
        envelope.payload.len() <= MAX_MANIFEST as usize,
        "update manifest is too large"
    );
    let key = VerifyingKey::from_bytes(&hex::<32>(public_key)?)?;
    key.verify_strict(
        envelope.payload.as_bytes(),
        &Signature::from_bytes(&hex::<64>(&envelope.signature)?),
    )
    .context("подпись обновления недействительна; установка отменена")?;
    let release: Release = serde_json::from_str(&envelope.payload)?;
    let version = semver::Version::parse(&release.version)?;
    ensure!(
        version.pre.is_empty() && version.build.is_empty(),
        "only stable numbered releases can be installed automatically"
    );
    ensure!(
        release.targets.contains_key(WINDOWS)
            && (release.targets.contains_key(LINUX)
                || release.targets.contains_key("linux-x86_64-deb")),
        "релиз должен содержать обе сборки, Windows и Ubuntu"
    );
    for artifact in release.targets.values() {
        ensure!(
            (1..=MAX_BINARY).contains(&artifact.size),
            "invalid update binary size"
        );
        hex::<32>(&artifact.sha256)?;
        https(&artifact.url)?;
    }
    Ok(release)
}

fn target() -> Result<&'static str> {
    #[cfg(target_os = "macos")]
    return Ok(if cfg!(target_arch = "aarch64") {
        "macos-arm64-app"
    } else {
        "macos-x86_64-app"
    });
    #[cfg(not(target_os = "macos"))]
    {
        ensure!(
            cfg!(target_arch = "x86_64"),
            "automatic updates currently require x86-64"
        );
        Ok(if cfg!(windows) { WINDOWS } else { LINUX })
    }
}

fn https(url: &str) -> Result<reqwest::Url> {
    let parsed = reqwest::Url::parse(url)?;
    ensure!(
        parsed.scheme() == "https"
            && parsed.host_str().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none(),
        "updates require an HTTPS URL without credentials"
    );
    Ok(parsed)
}

fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(120))
        .user_agent(format!("Desk-Switch/{}", crate::compatibility::VERSION))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()?)
}

fn release_envelope(bytes: &[u8]) -> Result<Envelope> {
    let release: serde_json::Value = serde_json::from_slice(bytes)?;
    ensure!(
        release["draft"] == false && release["prerelease"] == false,
        "only published stable releases are accepted"
    );
    let body = release["body"]
        .as_str()
        .context("release has no signed update information")?;
    let (_, signed) = body
        .split_once(SIGNED_START)
        .context("signed update information is missing")?;
    let (signed, rest) = signed
        .split_once(SIGNED_END)
        .context("signed update information is incomplete")?;
    ensure!(
        !rest.contains(SIGNED_START) && signed.len() <= MAX_MANIFEST as usize,
        "invalid signed update information"
    );
    let envelope: Envelope = serde_json::from_str(signed.trim())?;
    let payload: Release = serde_json::from_str(&envelope.payload)?;
    ensure!(
        release["tag_name"].as_str() == Some(format!("v{}", payload.version).as_str()),
        "release tag does not match signed version"
    );
    Ok(envelope)
}

fn fetch_release() -> Result<Option<Vec<u8>>> {
    let response = client()?
        .get(https(CHANNEL)?)
        .timeout(Duration::from_secs(15))
        .send()?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    response
        .error_for_status()?
        .take(MAX_MANIFEST * 4 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_MANIFEST as usize * 4,
        "release information is too large"
    );
    Ok(Some(bytes))
}

fn sibling(executable: &Path, suffix: &str) -> Result<PathBuf> {
    let mut name = executable
        .file_name()
        .context("executable has no filename")?
        .to_os_string();
    name.push(suffix);
    Ok(executable.with_file_name(name))
}

fn ticket(dir: &Path) -> PathBuf {
    dir.join("updates/pending.json")
}

fn binary_lock(executable: &Path) -> Result<fs::File> {
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(sibling(executable, ".desk-switch-update.lock")?)?;
    lock.try_lock()
        .context("другая копия приложения уже готовит обновление")?;
    Ok(lock)
}

fn checksum(path: &Path, artifact: &Artifact) -> Result<()> {
    ensure!(
        fs::metadata(path)?.len() == artifact.size,
        "размер обновления не совпадает с подписанным манифестом"
    );
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut data = [0; 64 * 1024];
    loop {
        let count = file.read(&mut data)?;
        if count == 0 {
            break;
        }
        digest.update(&data[..count]);
    }
    ensure!(
        digest.finalize().as_slice() == hex::<32>(&artifact.sha256)?,
        "контрольная сумма обновления не совпадает; установка отменена"
    );
    Ok(())
}

fn binary_version(path: &Path, release: &Release) -> Result<()> {
    let data = fs::read(path)?;
    let prefix = b"DESK-SWITCH-BUILD:";
    let mut found = false;
    for (position, _) in data
        .windows(prefix.len())
        .enumerate()
        .filter(|(_, part)| *part == prefix)
    {
        let start = position + prefix.len();
        let tail = &data[start..data.len().min(start + 128)];
        let Some(end) = tail.iter().position(|byte| *byte == b'}') else {
            continue;
        };
        let Ok(version) = serde_json::from_slice::<crate::compatibility::Version>(&tail[..=end])
        else {
            continue;
        };
        ensure!(
            version.app == release.version && version.protocol == release.protocol,
            "скачанный файл имеет другую версию или протокол, чем подписанный релиз"
        );
        found = true;
    }
    ensure!(found, "скачанный файл не содержит информации о сборке");
    Ok(())
}

fn stage_candidate(dir: &Path, executable: &Path, candidate: Candidate) -> Result<Outcome> {
    let http = client()?;
    let Candidate {
        envelope, release, ..
    } = candidate;
    let artifact = release
        .targets
        .get(target()?)
        .context("Install the latest .deb from GitHub once to enable package updates")?;
    let _lock = binary_lock(executable)?;
    let payload = sibling(executable, ".desk-switch-next")?;
    // An existing ticket is reused only after signature and payload verification.
    if let Ok(data) = fs::read(ticket(dir))
        && let Ok(pending) = serde_json::from_slice::<Pending>(&data)
        && pending.envelope.payload == envelope.payload
        && pending.executable == executable
        && pending.payload == payload
        && checksum(&payload, artifact).is_ok()
        && binary_version(&payload, &release).is_ok()
    {
        return Ok(Outcome::Ready(release.version.clone()));
    }
    let download = sibling(executable, ".desk-switch-download")?;
    // The per-binary lock makes this an abandoned partial download, not another job.
    if download.exists() {
        fs::remove_file(&download)?;
    }
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o700);
        }
        let mut file = options.open(&download).context("не удалось создать файл обновления рядом с приложением; нужен доступ на запись к его папке")?;
        let mut response = http
            .get(https(&artifact.url)?)
            .send()?
            .error_for_status()?
            .take(artifact.size + 1);
        let written = std::io::copy(&mut response, &mut file)?;
        ensure!(
            written == artifact.size,
            "update download size does not match the signed manifest"
        );
        file.flush()?;
        file.sync_all()?;
        drop(file);
        checksum(&download, artifact)?;
        binary_version(&download, &release)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&download, fs::Permissions::from_mode(0o700))?;
        }
        fs::rename(&download, &payload)?;
        let pending = Pending {
            envelope,
            executable: executable.to_path_buf(),
            payload: payload.clone(),
        };
        crate::config::write_atomic(&ticket(dir), &serde_json::to_string(&pending)?)?;
        Ok(Outcome::Ready(release.version.clone()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&download);
    }
    result
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Progress {
    pub downloaded: u64,
    pub total: u64,
}

pub struct Job {
    result: Receiver<Result<Outcome>>,
    progress: std::sync::Arc<std::sync::Mutex<Option<Progress>>>,
}
enum Operation {
    Check,
    Prepare {
        expected: String,
        candidate: Option<Candidate>,
    },
}
impl Job {
    pub fn start(dir: PathBuf, executable: PathBuf, ctx: eframe::egui::Context) -> Result<Self> {
        Self::spawn(Operation::Check, dir, executable, ctx)
    }
    pub fn prepare(
        dir: PathBuf,
        executable: PathBuf,
        expected: String,
        candidate: Option<Candidate>,
        ctx: eframe::egui::Context,
    ) -> Result<Self> {
        Self::spawn(
            Operation::Prepare {
                expected,
                candidate,
            },
            dir,
            executable,
            ctx,
        )
    }
    fn spawn(
        operation: Operation,
        dir: PathBuf,
        executable: PathBuf,
        ctx: eframe::egui::Context,
    ) -> Result<Self> {
        let (send, result) = mpsc::channel();
        let progress = std::sync::Arc::new(std::sync::Mutex::new(None));
        let progress_send = progress.clone();
        std::thread::Builder::new()
            .name("Desk Switch updates".into())
            .spawn(move || {
                let result = run_operation(
                    operation,
                    || check_latest(&dir, &executable),
                    |candidate| {
                        if shared_managed(&executable) {
                            shared_download(&dir, &candidate, |downloaded, total| {
                                *progress_send.lock().unwrap() =
                                    Some(Progress { downloaded, total });
                                ctx.request_repaint_of(eframe::egui::ViewportId::ROOT);
                            })
                        } else {
                            stage_candidate(&dir, &executable, candidate)
                        }
                    },
                );
                let _ = send.send(result);
                ctx.request_repaint_of(eframe::egui::ViewportId::ROOT);
            })?;
        Ok(Self { result, progress })
    }
    pub fn poll(&self) -> Option<Result<Outcome>> {
        self.result.try_recv().ok()
    }
    pub fn progress(&self) -> Option<Progress> {
        self.progress.lock().unwrap().clone()
    }
}

fn run_operation(
    operation: Operation,
    check: impl FnOnce() -> Result<Outcome>,
    prepare: impl FnOnce(Candidate) -> Result<Outcome>,
) -> Result<Outcome> {
    let Operation::Prepare {
        expected,
        candidate,
    } = operation
    else {
        return check();
    };
    let candidate = match candidate {
        Some(candidate) => candidate,
        None => match check()? {
            Outcome::Available(candidate) => candidate,
            _ => bail!("The requested signed update is not available on this PC."),
        },
    };
    ensure!(
        candidate.version() == expected,
        "The signed release changed from version {expected} to {}. Check for updates again.",
        candidate.version()
    );
    prepare(candidate)
}

fn check_latest(dir: &Path, executable: &Path) -> Result<Outcome> {
    let Some(bytes) = fetch_release()? else {
        return Ok(Outcome::Unpublished);
    };
    metadata_outcome(bytes, dir, executable, PUBLIC_KEY)
}

fn metadata_outcome(
    bytes: Vec<u8>,
    _dir: &Path,
    executable: &Path,
    public_key: &str,
) -> Result<Outcome> {
    let envelope = release_envelope(&bytes)?;
    let release = verified(&envelope, public_key)?;
    if semver::Version::parse(&release.version)?
        <= semver::Version::parse(crate::compatibility::VERSION)?
    {
        return Ok(Outcome::Current(release.version));
    }
    let candidate = Candidate {
        #[cfg(not(target_os = "macos"))]
        bytes,
        envelope,
        release,
    };
    if shared_managed(executable) {
        #[cfg(not(target_os = "macos"))]
        shared_release(_dir, &candidate)?;
        #[cfg(target_os = "macos")]
        candidate
            .release
            .targets
            .get(target()?)
            .context("This release has no signed application for this Mac architecture")?;
    } else {
        candidate
            .release
            .targets
            .get(target()?)
            .context("Install the latest .deb from GitHub once to enable package updates")?;
    }
    Ok(Outcome::Available(candidate))
}

pub fn shared_managed(executable: &Path) -> bool {
    cfg!(any(windows, target_os = "macos"))
        || executable == Path::new("/usr/lib/desk-switch/desk-switch")
}

#[cfg(not(target_os = "macos"))]
pub fn shared_config(dir: &Path) -> Result<rust_auto_updator::Config> {
    let config = if cfg!(windows) {
        rust_auto_updator::Config::new(
            "siarheisinelnikau",
            "desk-switch",
            crate::compatibility::VERSION,
            "desk-switch",
        )
    } else {
        rust_auto_updator::Config::debian(
            "siarheisinelnikau",
            "desk-switch",
            crate::compatibility::VERSION,
            "desk-switch",
        )
    };
    let mut config = config.map_err(anyhow::Error::msg)?;
    config.allow_asset_digest = true;
    config.asset_platform = Some(if cfg!(windows) { "windows" } else { "linux" }.into());
    config.legacy_executable_names = vec![
        if cfg!(windows) {
            "desk-switch-v{version}-windows-x86-64.exe"
        } else {
            "desk-switch-v{version}-linux-x86-64.deb"
        }
        .into(),
    ];
    config.max_executable_size = MAX_BINARY;
    config.install.stage_prefix = ".desk-switch-shared-update-".into();
    config.install.helper_name = if cfg!(windows) {
        "desk-switch-update.exe"
    } else {
        "desk-switch-update"
    }
    .into();
    config.install.restart_args = vec![
        "gui".into(),
        "--state-dir".into(),
        dir.to_string_lossy().into_owned(),
    ];
    config.install.rollback_args = config.install.restart_args.clone();
    #[cfg(windows)]
    {
        config.expected_subsystem = Some(2);
        config.install.startup_health_timeout = Some(Duration::from_secs(30));
    }
    config.validate().map_err(anyhow::Error::msg)?;
    Ok(config)
}

#[cfg(target_os = "macos")]
fn shared_download(
    dir: &Path,
    candidate: &Candidate,
    progress: impl FnMut(u64, u64),
) -> Result<Outcome> {
    macos::download(dir, candidate, progress)
}
#[cfg(not(target_os = "macos"))]
fn shared_release(dir: &Path, candidate: &Candidate) -> Result<rust_auto_updator::Release> {
    let config = shared_config(dir)?;
    let latest = rust_auto_updator::parse_release(std::str::from_utf8(&candidate.bytes)?, &config)
        .map_err(anyhow::Error::msg)?
        .context("signed installer is unavailable")?;
    let key = if cfg!(windows) {
        WINDOWS
    } else {
        "linux-x86_64-deb"
    };
    verify_shared_asset(&candidate.release, &latest, key)?;
    Ok(latest)
}
#[cfg(not(target_os = "macos"))]
fn shared_download(
    dir: &Path,
    candidate: &Candidate,
    progress: impl FnMut(u64, u64),
) -> Result<Outcome> {
    let config = shared_config(dir)?;
    let latest = shared_release(dir, candidate)?;
    let prepared = rust_auto_updator::prepare_download(&latest, &config, progress)
        .map_err(anyhow::Error::msg)?;
    Ok(Outcome::Prepared(
        candidate.version().to_owned(),
        Prepared {
            inner: prepared,
            #[cfg(target_os = "linux")]
            envelope: candidate.envelope.clone(),
        },
    ))
}

#[cfg(not(target_os = "macos"))]
fn verify_shared_asset(
    signed: &Release,
    latest: &rust_auto_updator::Release,
    key: &str,
) -> Result<()> {
    let artifact = signed
        .targets
        .get(key)
        .context("signed installer is missing from this release")?;
    ensure!(
        signed.version == latest.version.to_string()
            && artifact.url == latest.executable_url()
            && artifact.size == latest.executable_size()
            && Some(artifact.sha256.as_str()) == latest.sha256_digest(),
        "installer does not match the signed release"
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
static CLEANUP: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

pub fn shared_cli(args: &mut Vec<String>) -> Result<bool> {
    #[cfg(target_os = "macos")]
    return macos::cli(args);
    #[cfg(not(target_os = "macos"))]
    {
        if args.first().is_some_and(|arg| arg == "--apply-update") {
            rust_auto_updator::apply_update(&shared_config(&crate::config::state_dir()?)?)
                .map_err(anyhow::Error::msg)?;
            return Ok(true);
        }
        if args.first().is_some_and(|arg| arg == "gui")
            && args.len() >= 3
            && args
                .get(args.len() - 2)
                .is_some_and(|arg| arg == "--cleanup-update")
        {
            let index = args.len() - 2;
            let path = PathBuf::from(args.get(index + 1).context("missing cleanup directory")?);
            args.drain(index..=index + 1);
            *CLEANUP.lock().unwrap() = Some(path);
        }
        Ok(false)
    }
}

pub fn acknowledge_shared_start(dir: &Path) {
    #[cfg(target_os = "macos")]
    macos::acknowledge(dir);
    #[cfg(not(target_os = "macos"))]
    if let Some(path) = CLEANUP.lock().unwrap().take()
        && let Ok(config) = shared_config(dir)
    {
        rust_auto_updator::cleanup_after_update(path, config.install);
    }
}

pub struct InstallJob {
    result: Receiver<(Prepared, Result<(), String>)>,
}

impl InstallJob {
    pub fn start(mut prepared: Prepared, dir: &Path, ctx: eframe::egui::Context) -> Result<Self> {
        #[cfg(not(target_os = "macos"))]
        prepared
            .inner
            .set_restart_args(shared_config(dir)?.install.restart_args)
            .map_err(anyhow::Error::msg)?;
        let (send, result) = mpsc::channel();
        #[cfg(target_os = "macos")]
        let state = dir.to_path_buf();
        #[cfg(target_os = "linux")]
        let envelope = {
            let path = dir.join("updates/install-envelope.json");
            crate::config::write_atomic(&path, &serde_json::to_string(&prepared.envelope)?)?;
            path
        };
        std::thread::Builder::new()
            .name("Desk Switch install".into())
            .spawn(move || {
                #[cfg(target_os = "linux")]
                let installed = prepared.inner.handoff_with_installer(|archive| {
                    install_signed_package(archive, &envelope).map_err(|error| error.to_string())
                });
                #[cfg(windows)]
                let installed = rust_auto_updator::handoff(&mut prepared.inner);
                #[cfg(target_os = "macos")]
                let installed = prepared
                    .macos
                    .handoff(&state)
                    .map_err(|error| format!("{error:#}"));
                let _ = send.send((prepared, installed));
                ctx.request_repaint();
            })?;
        Ok(Self { result })
    }
    pub fn poll(&self) -> Option<(Prepared, Result<(), String>)> {
        self.result.try_recv().ok()
    }
}

#[cfg(target_os = "linux")]
fn install_signed_package(archive: &Path, envelope: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let helper = Path::new("/usr/lib/desk-switch/update-install");
    ensure!(
        helper.is_file(),
        "Install the current .deb once to enable password-free signed updates"
    );
    require_permission("com.deskswitch.update")?;
    let mut command = Command::new("/usr/bin/pkexec");
    command
        .arg(helper)
        .arg(archive)
        .arg(envelope)
        .stdin(Stdio::null());
    // Older pkexec versions close inherited descriptors one by one. Bound the
    // child-only descriptor limit, as the generic Debian installer does.
    unsafe {
        command.pre_exec(|| {
            let mut limits = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limits) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            limits.rlim_cur = limits.rlim_cur.min(4096);
            limits.rlim_max = limits.rlim_max.min(4096);
            if libc::setrlimit(libc::RLIMIT_NOFILE, &limits) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let result = command
        .output()
        .context("starting the signed package installer")?;
    ensure!(
        result.status.success(),
        "Signed package installation failed. {}",
        String::from_utf8_lossy(&result.stderr)
            .chars()
            .take(4000)
            .collect::<String>()
    );
    Ok(())
}

fn no_console(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
}

fn pending(dir: &Path) -> Result<(Pending, Release)> {
    let data = fs::read(ticket(dir))?;
    ensure!(
        data.len() <= MAX_MANIFEST as usize * 2,
        "invalid pending update size"
    );
    let pending: Pending = serde_json::from_slice(&data)?;
    ensure!(
        pending.executable.is_absolute()
            && pending.payload == sibling(&pending.executable, ".desk-switch-next")?,
        "invalid pending update paths"
    );
    let release = verified(&pending.envelope, PUBLIC_KEY)?;
    ensure!(
        semver::Version::parse(&release.version)?
            > semver::Version::parse(crate::compatibility::VERSION)?,
        "refusing an old update"
    );
    checksum(&pending.payload, &release.targets[target()?])?;
    binary_version(&pending.payload, &release)?;
    Ok((pending, release))
}

#[cfg(target_os = "linux")]
pub fn require_permission(action: &str) -> Result<()> {
    ensure!(
        ["com.deskswitch.update", "com.deskswitch.input-access"].contains(&action),
        "unsupported Desk Switch permission"
    );
    // Without --allow-user-interaction, pkcheck can only grant or fail. It
    // cannot show an authentication prompt or fall back to a password dialog.
    let result = Command::new("/usr/bin/pkcheck")
        .args([
            "--action-id",
            action,
            "--process",
            &std::process::id().to_string(),
        ])
        .stdin(Stdio::null())
        .output()
        .context("checking the installed Desk Switch permission")?;
    ensure!(
        result.status.success(),
        "Desk Switch's password-free permission is unavailable. Install the current .deb once and run it in an active local desktop session."
    );
    Ok(())
}

pub fn launch_apply(dir: &Path) -> Result<()> {
    let (pending, _) = pending(dir)?;
    ensure!(
        pending.executable == std::env::current_exe()?,
        "pending update belongs to another executable"
    );
    let helper = dir.join(if cfg!(windows) {
        "updates/apply-helper.exe"
    } else {
        "updates/apply-helper"
    });
    fs::copy(std::env::current_exe()?, &helper)?;
    let mut command = Command::new(helper);
    command.arg("internal-update-apply").arg(dir);
    no_console(&mut command);
    command.spawn().context("запуск установки обновления")?;
    Ok(())
}

pub fn staged(dir: &Path) -> Result<String> {
    Ok(pending(dir)?.1.version)
}

fn replace(source: &Path, target: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match fs::rename(source, target) {
            Ok(()) => return Ok(()),
            Err(error)
                if Instant::now() < deadline
                    && matches!(
                        error.kind(),
                        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::WouldBlock
                    ) =>
            {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn restart(executable: &Path, dir: &Path) -> Result<std::process::Child> {
    let mut command = Command::new(executable);
    command.args(["gui", "--tray", "--state-dir"]).arg(dir);
    no_console(&mut command);
    Ok(command.spawn()?)
}

pub fn apply(dir: &Path) -> Result<()> {
    let restart_on_error = fs::read(ticket(dir))
        .ok()
        .and_then(|data| serde_json::from_slice::<Pending>(&data).ok())
        .filter(|pending| {
            pending.executable.is_absolute() && verified(&pending.envelope, PUBLIC_KEY).is_ok()
        })
        .map(|pending| pending.executable);
    let result = apply_inner(dir);
    if let Err(error) = &result {
        let _ =
            crate::config::write_atomic(&dir.join("updates/last-error.txt"), &format!("{error:#}"));
        if let Some(executable) = restart_on_error
            && let Ok(lock) = crate::config::lock(dir, "desktop.lock")
        {
            drop(lock);
            let _ = restart(&executable, dir);
        }
    }
    result
}

fn apply_inner(dir: &Path) -> Result<()> {
    let (pending, release) = pending(dir)?;
    let deadline = Instant::now() + Duration::from_secs(45);
    let locks = loop {
        let try_locks = (|| {
            Ok::<_, anyhow::Error>([
                crate::config::lock(dir, "desktop.lock")?,
                crate::config::lock(dir, "run.lock")?,
                crate::config::lock(dir, "audio.lock")?,
            ])
        })();
        match try_locks {
            Ok(locks) => break locks,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Err(error) => return Err(error.context("службы не завершились; обновление отложено")),
        }
    };
    let _update_lock = binary_lock(&pending.executable)?;
    let backup = sibling(&pending.executable, ".desk-switch-previous")?;
    fs::copy(&pending.executable, &backup).context("создание резервной копии приложения")?;
    checksum(&pending.payload, &release.targets[target()?])?;
    replace(&pending.payload, &pending.executable)?;
    drop(locks);
    let mut child = match restart(&pending.executable, dir) {
        Ok(child) => child,
        Err(error) => {
            replace(&backup, &pending.executable)?;
            return Err(error.context(
                "обновлённое приложение не запустилось; предыдущая версия восстановлена",
            ));
        }
    };
    std::thread::sleep(Duration::from_secs(2));
    if let Some(status) = child.try_wait()? {
        replace(&backup, &pending.executable)?;
        bail!("обновлённое приложение завершилось ({status}); предыдущая версия восстановлена");
    }
    let _ = fs::remove_file(ticket(dir));
    let _ = fs::remove_file(dir.join("updates/last-error.txt"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    fn fixture() -> (Envelope, String) {
        let key = SigningKey::from_bytes(&[57; 32]);
        let artifact = Artifact {
            url: "https://example.com/app".into(),
            sha256: "ab".repeat(32),
            size: 16,
        };
        let release = Release {
            version: "0.3.0".into(),
            protocol: 3,
            targets: BTreeMap::from([(WINDOWS.into(), artifact.clone()), (LINUX.into(), artifact)]),
        };
        let payload = serde_json::to_string(&release).unwrap();
        let signature = crate::config::hex(&key.sign(payload.as_bytes()).to_bytes());
        (
            Envelope { payload, signature },
            crate::config::hex(key.verifying_key().as_bytes()),
        )
    }
    fn available_fixture() -> (Vec<u8>, String) {
        let key = SigningKey::from_bytes(&[57; 32]);
        let mut targets = BTreeMap::new();
        let mut assets = Vec::new();
        for (target, filename) in [
            (WINDOWS, "desk-switch-v99.0.0-windows-x86-64.exe"),
            (LINUX, "desk-switch-v99.0.0-linux-x86-64"),
            ("linux-x86_64-deb", "desk-switch-v99.0.0-linux-x86-64.deb"),
            ("macos-arm64-app", "desk-switch-v99.0.0-macos-arm64.zip"),
            ("macos-x86_64-app", "desk-switch-v99.0.0-macos-x86-64.zip"),
        ] {
            let url = format!(
                "https://github.com/siarheisinelnikau/desk-switch/releases/download/v99.0.0/{filename}"
            );
            let hash = "11".repeat(32);
            targets.insert(
                target.into(),
                Artifact {
                    url: url.clone(),
                    sha256: hash.clone(),
                    size: 1024,
                },
            );
            assets.push(serde_json::json!({"name":filename,"browser_download_url":url,"size":1024,"digest":format!("sha256:{hash}")}));
        }
        let payload = serde_json::to_string(&Release {
            version: "99.0.0".into(),
            protocol: crate::compatibility::PROTOCOL,
            targets,
        })
        .unwrap();
        let envelope = Envelope {
            signature: crate::config::hex(&key.sign(payload.as_bytes()).to_bytes()),
            payload,
        };
        let body = format!(
            "Release notes\n{SIGNED_START}\n{}\n{SIGNED_END}",
            serde_json::to_string(&envelope).unwrap()
        );
        let api = serde_json::json!({"tag_name":"v99.0.0","html_url":"https://github.com/siarheisinelnikau/desk-switch/releases/tag/v99.0.0","draft":false,"prerelease":false,"body":body,"assets":assets});
        (
            serde_json::to_vec(&api).unwrap(),
            crate::config::hex(key.verifying_key().as_bytes()),
        )
    }
    fn available_candidate() -> Candidate {
        let (bytes, key) = available_fixture();
        let executable = if cfg!(target_os = "linux") {
            Path::new("/usr/lib/desk-switch/desk-switch")
        } else {
            Path::new("/not-created/Desk Switch")
        };
        let Outcome::Available(candidate) =
            metadata_outcome(bytes, &std::env::temp_dir(), executable, &key).unwrap()
        else {
            panic!("the newer signed fixture should be available");
        };
        candidate
    }
    #[test]
    fn metadata_check_never_prepares_a_payload_or_creates_update_state() {
        let (bytes, key) = available_fixture();
        let state = std::env::temp_dir().join(format!(
            "desk-switch-check-only-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let executable = if cfg!(target_os = "linux") {
            Path::new("/usr/lib/desk-switch/desk-switch")
        } else {
            Path::new("/not-created/Desk Switch")
        };
        let outcome = run_operation(
            Operation::Check,
            || metadata_outcome(bytes, &state, executable, &key),
            |_| panic!("a metadata check must never prepare or download a payload"),
        )
        .unwrap();
        assert!(
            matches!(outcome, Outcome::Available(candidate) if candidate.version() == "99.0.0")
        );
        assert!(!state.exists());
    }
    #[test]
    fn an_explicit_download_retains_the_checked_signed_snapshot() {
        let candidate = available_candidate();
        let signature = candidate.envelope.signature.clone();
        let outcome = run_operation(
            Operation::Prepare {
                expected: "99.0.0".into(),
                candidate: Some(candidate),
            },
            || panic!("an explicit download must use the already checked release snapshot"),
            |candidate| {
                assert_eq!(candidate.envelope.signature, signature);
                assert_eq!(candidate.version(), "99.0.0");
                Ok(Outcome::Ready(candidate.version().to_owned()))
            },
        )
        .unwrap();
        assert!(matches!(outcome, Outcome::Ready(version) if version == "99.0.0"));
    }
    #[test]
    fn a_remote_prepare_rejects_release_drift_before_downloading() {
        let error = run_operation(
            Operation::Prepare {
                expected: "98.0.0".into(),
                candidate: None,
            },
            || Ok(Outcome::Available(available_candidate())),
            |_| panic!("a different version must never be downloaded for this request"),
        )
        .err()
        .unwrap();
        assert!(
            error
                .to_string()
                .contains("changed from version 98.0.0 to 99.0.0")
        );
    }
    #[test]
    fn a_remote_prepare_without_a_newer_signed_release_never_downloads() {
        for outcome in [
            Outcome::Current(crate::compatibility::VERSION.into()),
            Outcome::Unpublished,
        ] {
            assert!(
                run_operation(
                    Operation::Prepare {
                        expected: "99.0.0".into(),
                        candidate: None
                    },
                    || Ok(outcome),
                    |_| panic!("a missing signed update must never start a payload download"),
                )
                .is_err()
            );
        }
    }
    #[test]
    fn verifies_the_exact_manifest_and_rejects_tampering_and_wrong_signer() {
        let (mut envelope, key) = fixture();
        assert_eq!(verified(&envelope, &key).unwrap().version, "0.3.0");
        assert!(
            verified(
                &envelope,
                &crate::config::hex(SigningKey::from_bytes(&[58; 32]).verifying_key().as_bytes())
            )
            .is_err()
        );
        envelope.payload = envelope.payload.replace("0.3.0", "9.0.0");
        assert!(verified(&envelope, &key).is_err());
    }
    #[test]
    fn hash_and_size_must_both_match() {
        let path = std::env::temp_dir().join(format!("desk-switch-hash-{}", std::process::id()));
        fs::write(&path, b"signed app").unwrap();
        let mut artifact = Artifact {
            url: "https://example.com/app".into(),
            sha256: crate::config::hex(&Sha256::digest(b"signed app")),
            size: 10,
        };
        checksum(&path, &artifact).unwrap();
        fs::write(&path, b"changedapp").unwrap();
        assert!(checksum(&path, &artifact).is_err());
        artifact.size = 9;
        assert!(checksum(&path, &artifact).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn signed_release_body_needs_no_metadata_download_and_rejects_wrong_tag_or_prerelease() {
        let (envelope, key) = fixture();
        let body = format!(
            "Release notes\n{SIGNED_START}\n{}\n{SIGNED_END}",
            serde_json::to_string(&envelope).unwrap()
        );
        let mut api =
            serde_json::json!({"tag_name":"v0.3.0","draft":false,"prerelease":false,"body":body});
        let parsed = release_envelope(&serde_json::to_vec(&api).unwrap()).unwrap();
        assert_eq!(verified(&parsed, &key).unwrap().version, "0.3.0");
        api["tag_name"] = "v9.9.9".into();
        assert!(release_envelope(&serde_json::to_vec(&api).unwrap()).is_err());
        api["tag_name"] = "v0.3.0".into();
        api["prerelease"] = true.into();
        assert!(release_envelope(&serde_json::to_vec(&api).unwrap()).is_err());
        api["prerelease"] = false.into();
        api["body"] = "Unsigned description".into();
        assert!(release_envelope(&serde_json::to_vec(&api).unwrap()).is_err());
    }

    #[test]
    fn binary_metadata_must_match_the_signed_release_and_ignore_search_literals() {
        let path = std::env::temp_dir().join(format!("desk-switch-stamp-{}", std::process::id()));
        let (envelope, key) = fixture();
        let mut release = verified(&envelope, &key).unwrap();
        fs::write(&path, b"DESK-SWITCH-BUILD:search literal\0DESK-SWITCH-BUILD:{\"app\":\"0.3.0\",\"protocol\":3}\0").unwrap();
        binary_version(&path, &release).unwrap();
        release.version = "0.3.1".into();
        assert!(binary_version(&path, &release).is_err());
        fs::remove_file(path).unwrap();
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod shared_tests {
    use super::*;
    #[test]
    fn library_download_must_match_the_pinned_signed_artifact() {
        let config = shared_config(Path::new("/tmp/desk-switch-shared-fixture")).unwrap();
        let filename = if cfg!(windows) {
            "desk-switch-v99.0.0-windows-x86-64.exe"
        } else {
            "desk-switch-v99.0.0-linux-x86-64.deb"
        };
        let key = if cfg!(windows) {
            WINDOWS
        } else {
            "linux-x86_64-deb"
        };
        let url = format!(
            "https://github.com/siarheisinelnikau/desk-switch/releases/download/v99.0.0/{filename}"
        );
        let hash = "11".repeat(32);
        let json = serde_json::json!({"tag_name":"v99.0.0","html_url":"https://github.com/siarheisinelnikau/desk-switch/releases/tag/v99.0.0","draft":false,"prerelease":false,"assets":[{"name":filename,"browser_download_url":url,"size":1024,"digest":format!("sha256:{hash}")}]}).to_string();
        let latest = rust_auto_updator::parse_release(&json, &config)
            .unwrap()
            .unwrap();
        let mut signed = Release {
            version: "99.0.0".into(),
            protocol: 2,
            targets: BTreeMap::from([(
                key.into(),
                Artifact {
                    url,
                    sha256: hash,
                    size: 1024,
                },
            )]),
        };
        verify_shared_asset(&signed, &latest, key).unwrap();
        signed.targets.get_mut(key).unwrap().sha256 = "22".repeat(32);
        assert!(verify_shared_asset(&signed, &latest, key).is_err());
        signed.targets.get_mut(key).unwrap().sha256 = "11".repeat(32);
        signed.version = "98.0.0".into();
        assert!(verify_shared_asset(&signed, &latest, key).is_err());
    }
}
