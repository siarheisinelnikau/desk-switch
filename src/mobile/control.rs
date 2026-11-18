//! Complete, authenticated control records survive cancelled consumer waits.
use crate::workspace::net::Candidate;
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{mpsc, oneshot},
    task::{AbortHandle, JoinHandle},
    time::{Duration, timeout},
};

pub(super) const LIMIT: usize = 60 * 1024;
const TEXT_REJECTED: &str = "Text: The computer could not insert text. Try again when it is ready.";
pub(super) struct Control;
pub(super) struct Reader {
    incoming: mpsc::Receiver<Result<Value>>,
    task: JoinHandle<()>,
    writing: AbortHandle,
}
#[derive(Clone)]
pub(super) struct Writer {
    outgoing: mpsc::Sender<Write>,
}
struct Write {
    value: Value,
    result: oneshot::Sender<Result<()>>,
}

impl Control {
    pub(super) fn from_candidate(peer: Candidate) -> (Reader, Writer) {
        let (mut input, mut output) = peer.stream.into_split();
        let noise = Arc::new(Mutex::new(peer.noise));
        let reader_noise = noise.clone();
        let (outgoing, mut writes) = mpsc::channel::<Write>(8);
        let writing = tokio::spawn(async move {
            while let Some(write) = writes.recv().await {
                let result = timeout(Duration::from_secs(5), async {
                    let plain = serde_json::to_vec(&write.value)?;
                    ensure!(plain.len() + 16 <= LIMIT, "Control record is too large");
                    let mut encrypted = vec![0; plain.len() + 16];
                    let length = noise
                        .lock()
                        .unwrap()
                        .write_message(&plain, &mut encrypted)?;
                    output.write_u16(length as u16).await?;
                    output.write_all(&encrypted[..length]).await?;
                    output.flush().await?;
                    Ok::<_, anyhow::Error>(())
                })
                .await
                .context("The phone control write timed out")
                .and_then(|result| result);
                let failed = result.is_err();
                let _ = write.result.send(result);
                if failed {
                    break;
                }
            }
        })
        .abort_handle();
        let failed_writer = writing.clone();
        let (send, incoming) = mpsc::channel(8);
        let task = tokio::spawn(async move {
            loop {
                let result = async {
                    let length = usize::from(input.read_u16().await?);
                    ensure!(
                        (16..=LIMIT).contains(&length),
                        "Invalid encrypted control record size"
                    );
                    let mut encrypted = vec![0; length];
                    input.read_exact(&mut encrypted).await?;
                    let mut plain = vec![0; length];
                    let size = reader_noise
                        .lock()
                        .unwrap()
                        .read_message(&encrypted, &mut plain)?;
                    Ok::<_, anyhow::Error>(serde_json::from_slice(&plain[..size])?)
                }
                .await;
                let failed = result.is_err();
                if failed {
                    failed_writer.abort();
                }
                if send.send(result).await.is_err() || failed {
                    break;
                }
            }
        });
        (
            Reader {
                incoming,
                task,
                writing,
            },
            Writer { outgoing },
        )
    }
}
impl Reader {
    pub(super) async fn recv(&mut self) -> Result<Value> {
        self.incoming
            .recv()
            .await
            .context("The phone control connection ended")?
    }
}
impl Drop for Reader {
    fn drop(&mut self) {
        self.task.abort();
        self.writing.abort();
    }
}
impl Writer {
    pub(super) async fn send(&self, value: &Value) -> Result<()> {
        ensure!(
            serde_json::to_vec(value)?.len() + 16 <= LIMIT,
            "Control record is too large"
        );
        let (result, receive) = oneshot::channel();
        self.outgoing
            .send(Write {
                value: value.clone(),
                result,
            })
            .await
            .context("The phone control writer stopped")?;
        // Cancelling this wait never cancels a partly written encrypted frame.
        receive.await.context("The phone control writer stopped")?
    }
}
pub(super) type PreviewRoute = (
    tokio::sync::watch::Sender<super::screen::Options>,
    tokio::sync::watch::Receiver<Option<(u64, super::screen::Frame)>>,
);

pub(super) async fn run(
    peer: Candidate,
    key: String,
    own: String,
    texts: mpsc::Sender<super::Text>,
    mut plan: tokio::sync::watch::Receiver<super::hub::Plan>,
    routing: (
        bool,
        bool,
        Option<String>,
        PreviewRoute,
        super::screen::Provider,
    ),
    stop: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    use super::{Destination, screen};
    use serde_json::json;
    let (local, shared, workspace, (remote_request, mut remote_frames), provider) = routing;
    let capable = peer.phone_preview;
    let (mut reader, writer) = Control::from_candidate(peer);
    let authorized = |state: &super::hub::Plan| {
        workspace.as_ref().is_none_or(|id| {
            state
                .routing
                .workspace
                .as_ref()
                .is_some_and(|scope| &scope.id == id && scope.grants(&key))
        })
    };
    let shared = |state: &super::hub::Plan| {
        workspace.is_some() || state.sharing.get(&key).copied().unwrap_or(shared)
    };
    let permit = |state: &super::hub::Plan| {
        authorized(state)
            && provider.available()
            && !matches!(state.routing.destination, Destination::Paused { .. })
            && if local {
                matches!(state.routing.destination, Destination::Local)
            } else {
                !shared(state) || matches!(state.routing.destination, Destination::Local)
            }
    };
    let remote_permit = |state: &super::hub::Plan| {
        authorized(state)
            && !local
            && shared(state)
            && matches!(state.routing.destination, Destination::Remote { .. })
    };
    let remote_options = |options: screen::Options, state: &super::hub::Plan| screen::Options {
        enabled: options.enabled && remote_permit(state),
        ..options
    };
    let mut last_remote = None::<std::time::Instant>;
    let mut destination = plan.borrow().routing.destination.clone();
    let mut options_value = screen::Options::default();
    let (enable, allowed) = tokio::sync::watch::channel(permit(&plan.borrow()));
    let (request, options) = tokio::sync::watch::channel(screen::Options::default());
    let (mut frames, capture) = screen::start(provider, options, allowed, stop);
    struct Cancel(tokio::task::JoinHandle<()>);
    impl Drop for Cancel {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _cancel = Cancel(capture);
    loop {
        tokio::select! {
            biased;
            value = reader.recv() => {
                let value = value?;
                if capable && let Some(options) = screen::parse(&value, &key)? {
                    options_value = options;
                    request.send_replace(options);
                    let next = remote_options(options, &plan.borrow());
                    remote_request.send_if_modified(|old| {
                        if *old == next { false } else { *old = next; true }
                    });
                    writer.send(&json!("Ping")).await?;
                    if options.enabled && !permit(&plan.borrow()) && !remote_permit(&plan.borrow()) {
                        writer.send(&json!({"Error":"Cursor preview: unavailable on the selected computer"})).await?;
                    }
                    continue;
                }
                let text = crate::group::net::remote_text(value, &key)?;
                let (reply, result) = oneshot::channel();
                texts.try_send(super::Text { value: text, reply }).context("Remote text is still pending")?;
                match timeout(Duration::from_secs(4), result).await?? {
                    Ok(()) => writer.send(&crate::group::net::remote_text_ack()).await?,
                    Err(_) => writer.send(&json!({"Error":TEXT_REJECTED})).await?,
                }
            }
            changed = plan.changed() => {
                changed?;
                let (next, remote, changed_target) = {
                    let state = plan.borrow_and_update();
                    let changed_target = destination != state.routing.destination;
                    if changed_target { destination = state.routing.destination.clone(); }
                    (permit(&state), remote_options(options_value, &state), changed_target)
                };
                let previous = *enable.borrow();
                if previous != next {
                    enable.send_replace(next);
                }
                let previously_remote = remote_request.borrow().enabled;
                remote_request.send_if_modified(|old| {
                    if *old == remote { false } else { *old = remote; true }
                });
                if options_value.enabled && (changed_target || previous && !next || previously_remote && !remote.enabled) {
                    last_remote = None;
                    let status = if next || remote.enabled { "waiting for the selected computer" }
                        else { "unavailable on the selected computer" };
                    writer.send(&json!({"Error":format!("Cursor preview: {status}")})).await?;
                }
            }
            changed = remote_frames.changed() => {
                changed?;
                let frame = remote_frames.borrow_and_update().clone();
                if options_value.enabled && let Some((epoch, frame)) = frame && {
                    let state = plan.borrow();
                    epoch == state.epoch && remote_permit(&state)
                } {
                    let period = Duration::from_secs_f64(1.0 / f64::from(options_value.fps));
                    if matches!(frame, screen::Frame::Unavailable(_)) || last_remote.is_none_or(|last| last.elapsed() >= period) {
                        writer.send(&frame.packet(&own)?).await?;
                        last_remote = Some(std::time::Instant::now());
                    }
                }
            }
            changed = frames.changed() => {
                changed?;
                let frame = frames.borrow_and_update().clone();
                if options_value.enabled && permit(&plan.borrow()) && let Some(frame) = frame {
                    writer.send(&frame.packet(&own)?).await?;
                }
            }
        }
    }
}
