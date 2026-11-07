use anyhow::{Context, Result, ensure};
use std::{
    io::Read,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

// No shell, bounded output and a bounded server wait. Sink-input indices are
// monotonic server stream IDs, rather than reusable PipeWire global node IDs.
pub(super) fn pactl(args: &[String]) -> Result<Vec<u8>> {
    let mut child = Command::new("pactl")
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("starting pactl; install pulseaudio-utils")?;
    let output = child.stdout.take().unwrap();
    let error = child.stderr.take().unwrap();
    let read = |stream: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut data = Vec::new();
            stream
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut data)
                .map(|_| data)
        })
    };
    let output = read(Box::new(output));
    let error = read(Box::new(error));
    let until = Instant::now() + Duration::from_secs(2);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let output = output
        .join()
        .map_err(|_| anyhow::anyhow!("pactl output reader stopped"))??;
    let error = error
        .join()
        .map_err(|_| anyhow::anyhow!("pactl error reader stopped"))??;
    ensure!(
        output.len() <= 4 * 1024 * 1024,
        "pactl response is too large"
    );
    ensure!(
        status.is_some_and(|s| s.success()),
        "pactl failed or timed out: {}",
        String::from_utf8_lossy(&error)
            .chars()
            .take(512)
            .collect::<String>()
    );
    Ok(output)
}
