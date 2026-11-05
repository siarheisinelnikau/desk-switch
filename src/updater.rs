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

pub const CHANNEL: &str =
    "https://github.com/siarheisinelnikau/desk-switch/releases/latest/download/update.json";
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

#[derive(Debug)]
pub enum Outcome {
    Current,
    Unpublished,
    Ready(String),
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
        release.targets.contains_key(WINDOWS) && release.targets.contains_key(LINUX),
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
    ensure!(
        cfg!(target_arch = "x86_64"),
        "automatic updates currently require x86-64"
    );
    Ok(if cfg!(windows) { WINDOWS } else { LINUX })
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

pub fn check_and_stage(dir: &Path, executable: &Path) -> Result<Outcome> {
    let http = client()?;
    let response = http
        .get(https(CHANNEL)?)
        .timeout(Duration::from_secs(12))
        .send()?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(Outcome::Unpublished);
    }
    let mut bytes = Vec::new();
    response
        .error_for_status()?
        .take(MAX_MANIFEST + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_MANIFEST as usize,
        "update manifest is too large"
    );
    let envelope: Envelope = serde_json::from_slice(&bytes)?;
    let release = verified(&envelope, PUBLIC_KEY)?;
    if semver::Version::parse(&release.version)?
        <= semver::Version::parse(crate::compatibility::VERSION)?
    {
        return Ok(Outcome::Current);
    }
    let artifact = &release.targets[target()?];
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

pub struct Job {
    result: Receiver<Result<Outcome>>,
}
impl Job {
    pub fn start(dir: PathBuf, executable: PathBuf, ctx: eframe::egui::Context) -> Result<Self> {
        let (send, result) = mpsc::channel();
        std::thread::Builder::new()
            .name("Desk Switch updates".into())
            .spawn(move || {
                let result = check_and_stage(&dir, &executable);
                let _ = send.send(result);
                ctx.request_repaint();
            })?;
        Ok(Self { result })
    }
    pub fn poll(&self) -> Option<Result<Outcome>> {
        self.result.try_recv().ok()
    }
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
