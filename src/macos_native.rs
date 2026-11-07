//! One verified native helper is embedded in each macOS architecture build.
use anyhow::{Context, Result, ensure};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/desk-switch-native"));
const DDC_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/desk-switch-ddc"));

fn bytes(name: &str) -> Result<&'static [u8]> {
    match name {
        "desk-switch-native" => Ok(BYTES),
        "desk-switch-ddc" => Ok(DDC_BYTES),
        _ => anyhow::bail!("Unknown macOS helper"),
    }
}
pub fn export(name: &str, destination: &Path) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    ensure!(
        destination.is_absolute(),
        "The helper export path must be absolute"
    );
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o755)
        .open(destination)?;
    file.write_all(bytes(name)?)?;
    file.sync_all()?;
    Ok(())
}
fn executable(name: &str) -> Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    let contents = bytes(name)?;
    let current = std::env::current_exe()?;
    if let Some(macos) = current.parent()
        && macos.file_name().is_some_and(|part| part == "MacOS")
        && let Some(bundle) = macos.parent()
    {
        let helper = bundle.join("Helpers").join(name);
        let metadata = std::fs::symlink_metadata(&helper).context(
            "The application bundle is missing its native helper; reinstall Desk Switch",
        )?;
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "The bundled macOS helper must be a regular file"
        );
        ensure!(
            std::fs::read(&helper)? == contents,
            "The bundled macOS helper failed integrity verification; reinstall Desk Switch"
        );
        return Ok(helper);
    }
    let digest = format!("{:x}", Sha256::digest(contents));
    let directory = dirs::cache_dir()
        .context("The user cache directory is unavailable")?
        .join("desk-switch/native")
        .join(&digest);
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(&directory)?;
    ensure!(
        !std::fs::symlink_metadata(&directory)?
            .file_type()
            .is_symlink(),
        "Native helper directory must not be a link"
    );
    let path = directory.join(name);
    if !path.exists() {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let temporary = directory.join(format!(
            ".{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let result = (|| -> Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o700)
                .open(&temporary)?;
            file.write_all(contents)?;
            file.sync_all()?;
            match std::fs::hard_link(&temporary, &path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
            Ok(())
        })();
        let _ = std::fs::remove_file(&temporary);
        result?;
    }
    ensure!(
        !std::fs::symlink_metadata(&path)?.file_type().is_symlink(),
        "Native helper must not be a link"
    );
    ensure!(
        std::fs::read(&path)? == contents,
        "The embedded macOS helper failed verification"
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
    Ok(path)
}
pub fn command(operation: &str) -> Result<Command> {
    let mut command = Command::new(executable("desk-switch-native")?);
    command.arg(operation);
    Ok(command)
}
pub fn ddc_command(operation: &str) -> Result<Command> {
    let mut command = Command::new(executable("desk-switch-ddc")?);
    command.arg(operation);
    Ok(command)
}
pub fn async_command(operation: &str) -> Result<tokio::process::Command> {
    let mut command = tokio::process::Command::from(command(operation)?);
    command.kill_on_drop(true);
    Ok(command)
}
pub fn query<T: DeserializeOwned>(operation: &str, args: &[String]) -> Result<T> {
    query_command(operation, command(operation)?, args)
}
pub fn query_command<T: DeserializeOwned>(
    operation: &str,
    mut command: Command,
    args: &[String],
) -> Result<T> {
    let mut child = command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let output = child.stdout.take().unwrap();
    let errors = child.stderr.take().unwrap();
    let read = |stream: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut data = Vec::new();
            stream
                .take(1024 * 1024 + 1)
                .read_to_end(&mut data)
                .map(|_| data)
        })
    };
    let output = read(Box::new(output));
    let errors = read(Box::new(errors));
    let since = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if since.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("macOS {operation} query timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let output = output
        .join()
        .map_err(|_| anyhow::anyhow!("macOS query reader stopped"))??;
    let errors = errors
        .join()
        .map_err(|_| anyhow::anyhow!("macOS error reader stopped"))??;
    ensure!(
        status.success(),
        "{}",
        String::from_utf8_lossy(&errors)
            .chars()
            .take(4096)
            .collect::<String>()
    );
    ensure!(
        output.len() <= 1024 * 1024,
        "macOS query output is too large"
    );
    Ok(serde_json::from_slice(&output)?)
}
pub async fn ready(child: &mut tokio::process::Child) -> Result<String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut lines = BufReader::new(
        child
            .stderr
            .take()
            .context("macOS helper stderr is unavailable")?,
    )
    .lines();
    let label = tokio::time::timeout(Duration::from_secs(120), async {
        while let Some(line) = lines.next_line().await? {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line)
                && let Some(name) = value["ready"].as_str()
            {
                return Ok(name.to_owned());
            }
            anyhow::bail!("{line}");
        }
        anyhow::bail!("The macOS native helper stopped before becoming ready")
    })
    .await
    .context("Waiting for macOS permission timed out")??;
    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("macOS: {line}");
        }
    });
    Ok(label)
}
