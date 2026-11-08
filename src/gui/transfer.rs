//! File selection and transfer controls use the existing root window. The OS
//! picker and transfer service wake the dispatcher only when their state changes.
use super::{Desktop, Page};
use crate::group::{
    file_transfer::{Direction, Origin, Phase, Transfer},
    model::Workspace,
};
use anyhow::{Context, Result, ensure};
use eframe::egui::{self, RichText};
use std::{
    collections::BTreeSet,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    task::{Context as TaskContext, Poll, Wake, Waker},
};

fn tr(text: &str) -> String {
    crate::i18n::tr(text)
}

const MAX_SELECTION: usize = 256;
type PickerFuture = Pin<Box<dyn Future<Output = Option<Vec<rfd::FileHandle>>> + Send>>;

#[derive(Default)]
pub(super) struct Transfers {
    open: bool,
    target: String,
    selection: Vec<PathBuf>,
    picker: Option<PickerFuture>,
    error: Option<String>,
    service_error: Option<String>,
    focus_composer: bool,
    #[cfg(debug_assertions)]
    pub(super) preview: Vec<Transfer>,
}

enum Command {
    Send { target: String, paths: Vec<PathBuf> },
    Cancel(String),
    Resume(String),
    OpenFolder(PathBuf),
    RetryService,
    ChooseAgain(String),
}

struct Capabilities {
    available: bool,
    retryable: bool,
    resumable: BTreeSet<String>,
}

struct PickerWake(egui::Context);
impl Wake for PickerWake {
    fn wake(self: Arc<Self>) {
        self.0.request_repaint_of(egui::ViewportId::ROOT);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.request_repaint_of(egui::ViewportId::ROOT);
    }
}

impl Transfers {
    pub(super) fn is_open(&self) -> bool {
        self.open
    }

    pub(super) fn close(&mut self) {
        self.open = false;
    }

    fn open(&mut self, target: Option<&str>) {
        self.open = true;
        self.error = None;
        if let Some(target) = target {
            self.target = target.into();
            self.focus_composer = true;
        }
    }

    pub(super) fn tick(&mut self, ctx: &egui::Context) {
        let Some(picker) = self.picker.as_mut() else {
            return;
        };
        let waker = Waker::from(Arc::new(PickerWake(ctx.clone())));
        let result = match picker.as_mut().poll(&mut TaskContext::from_waker(&waker)) {
            Poll::Ready(paths) => paths,
            Poll::Pending => return,
        };
        self.picker = None;
        if let Some(paths) = result {
            for path in paths {
                let path = path.path().to_path_buf();
                if self.selection.contains(&path) {
                    continue;
                }
                if self.selection.len() == MAX_SELECTION {
                    self.error = Some(
                        "Choose up to 256 files or folders at a time. A selected folder includes its contents."
                            .into(),
                    );
                    break;
                }
                self.selection.push(path);
            }
        }
    }

    fn pick(&mut self, folders: bool, ctx: &egui::Context) -> Result<()> {
        ensure!(self.picker.is_none(), "The file picker is already open");
        let title = tr(if folders {
            "Add folders…"
        } else {
            "Add files…"
        });
        // AppKit constructs the native sheet on the UI/main thread. RFD owns
        // platform I/O; its future wakes this context once the chooser finishes.
        let dialog = rfd::AsyncFileDialog::new().set_title(title);
        self.picker = Some(if folders {
            Box::pin(dialog.pick_folders())
        } else {
            Box::pin(dialog.pick_files())
        });
        ctx.request_repaint_of(egui::ViewportId::ROOT);
        self.error = None;
        Ok(())
    }

    fn show(
        &mut self,
        ctx: &egui::Context,
        workspace: &Workspace,
        own: &str,
        capabilities: &Capabilities,
        transfers: &[Transfer],
    ) -> Vec<Command> {
        let available = capabilities.available;
        let retryable = capabilities.retryable;
        let mut commands = Vec::new();
        if !self.open {
            return commands;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) && self.picker.is_none() {
            self.open = false;
            return commands;
        }
        if !workspace.members.contains_key(&self.target)
            && let Some(member) = workspace
                .members
                .values()
                .find(|member| member.node.key != own && member.compatible())
        {
            self.target = member.node.key.clone();
        }
        let bounds = ctx.content_rect();
        let size = (bounds.size() - egui::vec2(48.0, 132.0)).max(egui::vec2(320.0, 220.0));
        let mut open = self.open;
        egui::Window::new(tr("Transfers"))
            .id(egui::Id::new("workspace-transfers"))
            .open(&mut open)
            .collapsible(false)
            .default_size(egui::vec2(660.0, 520.0).min(size))
            .min_size(egui::vec2(380.0, 240.0).min(size))
            .max_size(size)
            .default_pos(egui::pos2(
                bounds.center().x - 330.0_f32.min(size.x * 0.5),
                bounds.top() + 100.0,
            ))
            .show(ctx, |ui| {
                let mut scroll = egui::ScrollArea::vertical()
                    .id_salt("transfers-content")
                    .auto_shrink([false, false]);
                if std::mem::take(&mut self.focus_composer) {
                    scroll = scroll.vertical_scroll_offset(0.0);
                }
                scroll.show(ui, |ui| {
                        ui.strong(tr("Send files and folders"));
                        ui.horizontal_wrapped(|ui| {
                            ui.label(tr("To:"));
                            egui::ComboBox::from_id_salt("transfer-recipient")
                                .width(ui.available_width().min(360.0))
                                .selected_text(
                                    workspace
                                        .members
                                        .get(&self.target)
                                        .map(|member| member.node.name.clone())
                                        .unwrap_or_else(|| tr("Choose a computer")),
                                )
                                .show_ui(ui, |ui| {
                                    for member in workspace
                                        .members
                                        .values()
                                        .filter(|member| member.node.key != own)
                                    {
                                        ui.add_enabled_ui(member.compatible(), |ui| {
                                            ui.selectable_value(
                                                &mut self.target,
                                                member.node.key.clone(),
                                                &member.node.name,
                                            );
                                        });
                                    }
                                });
                        });
                        ui.weak(tr(
                            "Saved computers receive files in Downloads / Desk Switch. Existing files are kept.",
                        ));
                        ui.add_space(8.0);
                        ui.horizontal_wrapped(|ui| {
                            for (label, folders) in [("Add files…", false), ("Add folders…", true)] {
                                if ui
                                    .add_enabled(self.picker.is_none(), egui::Button::new(tr(label)))
                                    .clicked()
                                    && let Err(error) = self.pick(folders, ctx)
                                {
                                    self.error = Some(format!("Could not open the file picker: {error:#}"));
                                }
                            }
                            if !self.selection.is_empty() && ui.button(tr("Clear selection")).clicked() {
                                self.selection.clear();
                            }
                        });
                        if self.picker.is_some() {
                            ui.weak(tr("Choose files in the system dialog…"));
                        }
                        if !self.selection.is_empty() {
                            ui.add_space(6.0);
                            egui::ScrollArea::vertical()
                                .id_salt("transfer-selection")
                                .max_height(100.0)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    let mut remove = None;
                                    for (index, path) in self.selection.iter().enumerate() {
                                        ui.push_id(index, |ui| {
                                            ui.horizontal(|ui| {
                                                if ui.small_button(tr("Remove")).clicked() {
                                                    remove = Some(index);
                                                }
                                                ui.add(
                                                    egui::Label::new(file_name(path)).truncate(),
                                                )
                                                .on_hover_text(path.display().to_string());
                                            });
                                        });
                                    }
                                    if let Some(index) = remove {
                                        self.selection.remove(index);
                                    }
                                });
                        }
                        let recipient_ready = workspace
                            .members
                            .get(&self.target)
                            .is_some_and(|member| member.node.key != own && member.compatible());
                        ui.horizontal_wrapped(|ui| {
                            if ui
                                .add_enabled(
                                    available && recipient_ready && !self.selection.is_empty() && self.picker.is_none(),
                                    egui::Button::new(tr("Send")),
                                )
                                .clicked()
                            {
                                commands.push(Command::Send {
                                    target: self.target.clone(),
                                    paths: self.selection.clone(),
                                });
                            }
                            if !available {
                                ui.weak(tr(if !retryable && self.service_error.is_some() {
                                    "File sharing stopped. Restart Desk Switch to try again."
                                } else {
                                    "File sharing is starting. Try again shortly."
                                }));
                                if retryable && self.service_error.is_some() && ui.small_button(tr("Retry")).clicked() {
                                    commands.push(Command::RetryService);
                                }
                            } else if !recipient_ready {
                                ui.weak(tr("Connect the receiving computer to send files."));
                            }
                        });
                        if let Some(error) = self.error.as_ref().or(self.service_error.as_ref()) {
                            error_feedback(ui, error);
                        }
                        ui.add_space(12.0);
                        ui.separator();
                        ui.add_space(8.0);
                        ui.strong(tr("Recent transfers"));
                        if transfers.is_empty() {
                            ui.weak(tr("Files sent and received will appear here."));
                        }
                        for transfer in ordered_transfers(transfers) {
                            ui.push_id(&transfer.id, |ui| {
                                transfer_row(
                                    ui,
                                    transfer,
                                    workspace,
                                    capabilities.resumable.contains(&transfer.id),
                                    &mut commands,
                                );
                            });
                        }
                    });
            });
        self.open = open;
        commands
    }
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn progress(transfer: &Transfer) -> f32 {
    if transfer.phase == Phase::Completed {
        1.0
    } else if transfer.total == 0 {
        0.0
    } else {
        (transfer.bytes as f64 / transfer.total as f64).clamp(0.0, 1.0) as f32
    }
}

fn bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = value as f64;
    let mut index = 0;
    while value >= 1024.0 && index + 1 < UNITS.len() {
        value /= 1024.0;
        index += 1;
    }
    if index == 0 {
        format!("{value:.0} {}", UNITS[index])
    } else {
        format!("{value:.1} {}", UNITS[index])
    }
}

fn active(transfer: &Transfer) -> bool {
    matches!(
        transfer.phase,
        Phase::Preparing | Phase::Connecting | Phase::Transferring | Phase::Verifying
    ) || (transfer.phase == Phase::Completed
        && transfer.origin == Origin::Clipboard
        && transfer.direction == Direction::Receiving
        && transfer.clipboard_pending)
}

fn ordered_transfers(transfers: &[Transfer]) -> Vec<&Transfer> {
    // Service snapshots are newest first. Keep that order within each group
    // while making an older running transfer visible above completed history.
    let mut ordered = transfers.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|transfer| !active(transfer));
    ordered
}

fn phase(transfer: &Transfer) -> &'static str {
    match transfer.phase {
        Phase::Preparing => "Preparing…",
        Phase::Connecting => "Connecting…",
        Phase::Transferring => match transfer.direction {
            Direction::Sending => "Sending…",
            Direction::Receiving => "Receiving…",
        },
        Phase::Paused => "Transfer paused",
        Phase::Verifying => "Verifying files…",
        Phase::Completed
            if transfer.origin == Origin::Clipboard
                && transfer.direction == Direction::Receiving =>
        {
            if transfer.clipboard_ready {
                "Copied to clipboard"
            } else if transfer.clipboard_pending {
                "Preparing clipboard…"
            } else {
                "Files saved"
            }
        }
        Phase::Completed => match transfer.direction {
            Direction::Sending => "Sent",
            Direction::Receiving => "Received",
        },
        Phase::Cancelled => "Cancelled",
        Phase::Failed => "Transfer failed",
    }
}

fn transfer_row(
    ui: &mut egui::Ui,
    transfer: &Transfer,
    workspace: &Workspace,
    resumable: bool,
    commands: &mut Vec<Command>,
) {
    ui.add_space(8.0);
    let peer = workspace
        .members
        .get(&transfer.peer)
        .map(|member| member.node.name.as_str())
        .unwrap_or("—");
    ui.add(egui::Label::new(RichText::new(&transfer.name).strong()).truncate())
        .on_hover_text(&transfer.name);
    ui.horizontal_wrapped(|ui| {
        ui.weak(format!(
            "{} {peer}",
            tr(match transfer.direction {
                Direction::Sending => "To",
                Direction::Receiving => "From",
            })
        ));
        ui.label(tr(phase(transfer)));
        if transfer.origin == Origin::Clipboard {
            ui.weak(tr("Shared clipboard"));
        }
    });
    if transfer.total > 0 && !matches!(transfer.phase, Phase::Cancelled | Phase::Failed) {
        ui.add(
            egui::ProgressBar::new(progress(transfer))
                .text(format!(
                    "{} / {}",
                    bytes(transfer.bytes),
                    bytes(transfer.total)
                ))
                .animate(false),
        );
    }
    if let Some(error) = &transfer.error {
        error_feedback(ui, error);
    }
    ui.horizontal_wrapped(|ui| {
        let clipboard = transfer.phase == Phase::Completed
            && transfer.direction == Direction::Receiving
            && transfer.origin == Origin::Clipboard;
        if clipboard
            && transfer.clipboard_pending
            && ui.small_button(tr("Keep current clipboard")).clicked()
        {
            commands.push(Command::Cancel(transfer.id.clone()));
        }
        if clipboard
            && (!transfer.clipboard_pending || transfer.error.is_some())
            && ui
                .small_button(tr(if transfer.clipboard_pending {
                    "Retry clipboard"
                } else {
                    "Copy received files"
                }))
                .clicked()
        {
            commands.push(Command::Resume(transfer.id.clone()));
        }
        if !matches!(transfer.phase, Phase::Completed | Phase::Cancelled)
            && ui.small_button(tr("Cancel transfer")).clicked()
        {
            commands.push(Command::Cancel(transfer.id.clone()));
        }
        if matches!(transfer.phase, Phase::Paused | Phase::Failed)
            && resumable
            && ui.small_button(tr("Resume transfer")).clicked()
        {
            commands.push(Command::Resume(transfer.id.clone()));
        }
        if matches!(transfer.phase, Phase::Paused | Phase::Failed)
            && !resumable
            && transfer.direction == Direction::Sending
            && ui.small_button(tr("Choose files again…")).clicked()
        {
            commands.push(Command::ChooseAgain(transfer.peer.clone()));
        }
        if transfer.phase == Phase::Completed
            && transfer.direction == Direction::Receiving
            && let Some(path) = transfer.paths.first().and_then(|path| path.parent())
            && ui.small_button(tr("Open folder")).clicked()
        {
            commands.push(Command::OpenFolder(path.into()));
        }
    });
    ui.add_space(6.0);
    ui.separator();
}

fn error_summary(error: &str) -> &'static str {
    if error.contains("source file changed") || error.contains("saved export has changed") {
        "A source file changed. Send the files again."
    } else if error.contains("storage budget") {
        "This transfer is larger than the available file sharing space."
    } else if error.contains("failed verification") {
        "File verification failed. Resume the transfer or send the files again."
    } else if error.contains("stopped responding") || error.contains("did not answer") {
        "Connection interrupted. Resume when the other computer is online."
    } else if error.contains("no longer a trusted workspace member") {
        "The other computer is no longer linked. Reconnect it before transferring files."
    } else if error.contains("queue is full") {
        "Too many transfers are in progress. Wait for one to finish."
    } else if error.contains("already complete") || error.contains("Completed transfers") {
        "This transfer has already finished."
    } else if error.contains("regular files")
        || error.contains("Symbolic links")
        || error.contains("Reparse points")
        || error.contains("Network and device paths")
    {
        "Choose regular files or folders on this computer."
    } else if error.starts_with("File sharing is unavailable") {
        "File sharing is unavailable"
    } else if error.starts_with("The received folder is no longer available") {
        "The received folder is no longer available"
    } else if error.starts_with("Could not open the received folder") {
        "Could not open the received folder"
    } else if error.starts_with("Could not open the file picker") {
        "Could not open the file picker"
    } else if error.starts_with("Choose up to 256 files or folders") {
        "Choose up to 256 files or folders at a time. A selected folder includes its contents."
    } else if error.starts_with("Choose the files or folders again to retry preparation") {
        "Choose the files or folders again to retry preparation"
    } else if error.starts_with("The receiving computer has no space") {
        "The receiving computer has no space for the remaining files. Free space and resume the transfer."
    } else if error.starts_with("Connection interrupted.") {
        "Connection interrupted. Resume when the other computer is online."
    } else {
        "The transfer could not finish. Resume or send the files again."
    }
}

fn error_feedback(ui: &mut egui::Ui, error: &str) {
    ui.colored_label(ui.visuals().error_fg_color, tr(error_summary(error)));
    egui::CollapsingHeader::new(tr("Details"))
        .id_salt(("file-error-details", error))
        .show(ui, |ui| {
            ui.add(egui::Label::new(tr(error)).wrap());
        });
}

impl Desktop {
    #[cfg(debug_assertions)]
    pub(super) fn preview_transfers(&mut self, case: &str) {
        let transfer = |id: &str,
                        peer: String,
                        direction,
                        origin,
                        name: &str,
                        phase,
                        bytes,
                        total,
                        clipboard_ready| Transfer {
            id: id.into(),
            peer,
            direction,
            origin,
            name: name.into(),
            phase,
            bytes,
            total,
            files: 42,
            error: None,
            paths: Vec::new(),
            clipboard_ready,
            clipboard_pending: false,
            clipboard_skipped: false,
        };
        self.transfers.preview = vec![
            transfer(
                "paused-fixture",
                "44".repeat(32),
                Direction::Sending,
                Origin::Send,
                "Client deliverables — source images and design documents",
                Phase::Paused,
                148_000_000,
                934_000_000,
                false,
            ),
            transfer(
                "clipboard-fixture",
                "11".repeat(32),
                Direction::Receiving,
                Origin::Clipboard,
                "Screenshot and notes for the next release",
                Phase::Completed,
                12_400_000,
                12_400_000,
                true,
            ),
            transfer(
                "active-fixture",
                "33".repeat(32),
                Direction::Sending,
                Origin::Send,
                "Project archive — reviewed presentation, source artwork and reference photographs — final.zip",
                Phase::Transferring,
                2_648_000_000,
                4_096_000_000,
                false,
            ),
        ];
        self.transfers.preview[0].error =
            Some("Connection interrupted. Resume when the other computer is online.".into());
        self.transfers.preview[1].paths = vec![PathBuf::from(
            "/home/demo/Downloads/Desk Switch/Transfer demo/files/Screenshot and notes",
        )];
        if case == "shared-four-transfers-error" {
            let failed = self.transfers.preview.last_mut().unwrap();
            failed.phase = Phase::Failed;
            failed.error = Some("The receiving computer has no space for the remaining files. Free space and resume the transfer.".into());
        }
        self.transfers.preview.reverse();
        self.open_transfers(Some(&"33".repeat(32)));
        if case == "shared-four-transfers-selection" {
            self.transfers.selection = [
                "Project archive — reviewed presentation, source artwork and reference photographs — final.zip",
                "Client deliverables — source images and design documents",
            ].map(|name| PathBuf::from("/home/demo/Documents").join(name)).into();
        }
    }

    pub(super) fn start_file_transfers(&mut self, ctx: &egui::Context) {
        if self.group.file_transfer.is_none() {
            match crate::group::file_transfer::Service::start(self.dir.clone(), ctx.clone()) {
                Ok(service) => {
                    self.group.file_transfer = Some(service);
                    self.transfers.service_error = None;
                }
                Err(error) => {
                    self.transfers.service_error =
                        Some(format!("File sharing is unavailable: {error:#}"));
                }
            }
        }
    }

    pub(super) fn update_file_transfers(&self, workspace: &Workspace) {
        let Some(service) = &self.group.file_transfer else {
            return;
        };
        let trusted = workspace.authority.is_some() && !self.quitting;
        service.update(crate::group::file_transfer::TrustSnapshot {
            workspace_id: workspace
                .authority
                .as_ref()
                .map(|authority| authority.workspace_id.clone())
                .unwrap_or_default(),
            own: self.public_key.clone(),
            peers: workspace
                .members
                .iter()
                .filter(|(key, _)| trusted && *key != &self.public_key)
                .map(|(key, member)| {
                    (
                        key.clone(),
                        crate::group::file_transfer::Peer {
                            address: member.address,
                            online: member.compatible(),
                        },
                    )
                })
                .collect(),
            clipboard_enabled: trusted && workspace.settings.clipboard_enabled,
        });
        if let Some(clipboard) = &self.group.clipboard {
            service.clipboard(clipboard);
        }
    }

    pub(super) fn open_transfers(&mut self, target: Option<&str>) {
        self.transfers.open(target);
        self.group.add_open = false;
        self.group.layout_editor.close();
        self.page = Page::Overview;
    }

    fn transfer_snapshot(&self) -> Vec<Transfer> {
        #[cfg(debug_assertions)]
        if self.group.preview {
            return self.transfers.preview.clone();
        }
        self.group
            .file_transfer
            .as_ref()
            .map(|service| service.snapshot())
            .unwrap_or_default()
    }

    pub(super) fn transfer_toolbar(&mut self, ui: &mut egui::Ui) {
        if self.group.workspace.is_none() {
            return;
        }
        let count = self
            .transfer_snapshot()
            .iter()
            .filter(|transfer| active(transfer))
            .count();
        let label = if count == 0 {
            tr("Transfers")
        } else {
            crate::i18n::tr(format!("Transfers · {count}"))
        };
        if ui
            .add_enabled(
                !self.quitting,
                egui::Button::new(label).min_size(egui::vec2(0.0, 40.0)),
            )
            .clicked()
        {
            self.open_transfers(None);
        }
    }

    pub(super) fn transfer_card_action(&mut self, ui: &mut egui::Ui, key: &str, online: bool) {
        if key != self.public_key
            && ui
                .add_enabled(online, egui::Button::new(tr("Send files…")))
                .clicked()
        {
            self.open_transfers(Some(key));
        }
    }

    pub(super) fn transfer_window(&mut self, ctx: &egui::Context) {
        let Some(workspace) = self.group.workspace.as_ref() else {
            self.transfers.close();
            return;
        };
        let snapshot = self.transfer_snapshot();
        let runtime_error = self
            .group
            .file_transfer
            .as_ref()
            .and_then(|service| service.error());
        if let Some(error) = &runtime_error {
            self.transfers.service_error = Some(format!("File sharing is unavailable: {error}"));
        }
        let available = (self.group.file_transfer.is_some()
            && runtime_error.is_none()
            && workspace.authority.is_some())
            || self.group.preview;
        let capabilities = Capabilities {
            available,
            retryable: self.group.file_transfer.is_none(),
            resumable: snapshot
                .iter()
                .filter(|transfer| matches!(transfer.phase, Phase::Paused | Phase::Failed))
                .filter(|transfer| {
                    self.group
                        .file_transfer
                        .as_ref()
                        .map_or(self.group.preview, |service| {
                            service.can_resume(&transfer.id)
                        })
                })
                .map(|transfer| transfer.id.clone())
                .collect(),
        };
        let commands =
            self.transfers
                .show(ctx, workspace, &self.public_key, &capabilities, &snapshot);
        for command in commands {
            let result = match command {
                Command::Send { target, paths } => self
                    .group
                    .file_transfer
                    .as_ref()
                    .context("File sharing is unavailable")
                    .and_then(|service| service.send(target, paths))
                    .map(|_| self.transfers.selection.clear()),
                Command::Cancel(id) => self
                    .group
                    .file_transfer
                    .as_ref()
                    .context("File sharing is unavailable")
                    .and_then(|service| service.cancel(&id)),
                Command::Resume(id) => self
                    .group
                    .file_transfer
                    .as_ref()
                    .context("File sharing is unavailable")
                    .and_then(|service| service.resume(&id)),
                Command::OpenFolder(path) => open_folder(&path),
                Command::RetryService => {
                    self.start_file_transfers(ctx);
                    if let Some(error) = &self.transfers.service_error {
                        Err(anyhow::anyhow!(error.clone()))
                    } else {
                        Ok(())
                    }
                }
                Command::ChooseAgain(target) => {
                    self.open_transfers(Some(&target));
                    Ok(())
                }
            };
            self.transfers.error = result.err().map(|error| format!("{error:#}"));
        }
    }
}

fn open_folder(path: &std::path::Path) -> Result<()> {
    ensure!(path.is_dir(), "The received folder is no longer available");
    #[cfg(windows)]
    let mut command = {
        let mut command = std::process::Command::new("explorer.exe");
        command.arg(path);
        command
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(path);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(path);
        command
    };
    command
        .spawn()
        .context("Could not open the received folder")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    fn clipboard_transfer() -> Transfer {
        Transfer {
            id: "clipboard-test".into(),
            peer: "11".repeat(32),
            direction: Direction::Receiving,
            origin: Origin::Clipboard,
            name: "Photographs and notes".into(),
            phase: Phase::Completed,
            bytes: 128,
            total: 128,
            files: 2,
            error: None,
            paths: Vec::new(),
            clipboard_ready: false,
            clipboard_pending: true,
            clipboard_skipped: false,
        }
    }

    #[test]
    fn copied_files_are_not_reported_ready_before_native_clipboard_acknowledgement() {
        let mut transfer = clipboard_transfer();
        assert_eq!(phase(&transfer), "Preparing clipboard…");
        assert!(active(&transfer));
        transfer.clipboard_ready = true;
        transfer.clipboard_pending = false;
        assert_eq!(phase(&transfer), "Copied to clipboard");
        assert!(!active(&transfer));
        transfer.clipboard_ready = false;
        assert_eq!(phase(&transfer), "Files saved");
        assert!(!active(&transfer));
        transfer.direction = Direction::Sending;
        assert_eq!(phase(&transfer), "Sent");
        assert!(!active(&transfer));
        transfer.phase = Phase::Verifying;
        assert_eq!(phase(&transfer), "Verifying files…");
        assert!(active(&transfer));
        transfer.phase = Phase::Failed;
        assert_eq!(phase(&transfer), "Transfer failed");
        assert!(!active(&transfer));
    }

    struct CancelledPicker {
        completed: Arc<AtomicBool>,
        waker: Arc<Mutex<Option<Waker>>>,
    }
    impl Future for CancelledPicker {
        type Output = Option<Vec<rfd::FileHandle>>;
        fn poll(self: Pin<&mut Self>, ctx: &mut TaskContext<'_>) -> Poll<Self::Output> {
            if self.completed.load(Ordering::SeqCst) {
                Poll::Ready(None)
            } else {
                *self.waker.lock().unwrap() = Some(ctx.waker().clone());
                Poll::Pending
            }
        }
    }

    #[test]
    fn picker_sleeps_until_woken_then_cancellation_keeps_the_existing_selection() {
        let ctx = egui::Context::default();
        for _ in 0..8 {
            let _ = ctx.run_ui(egui::RawInput::default(), |_| {});
        }
        let repaints = Arc::new(AtomicUsize::new(0));
        let received = repaints.clone();
        ctx.set_request_repaint_callback(move |info| {
            assert_eq!(info.viewport_id, egui::ViewportId::ROOT);
            received.fetch_add(1, Ordering::SeqCst);
        });
        let completed = Arc::new(AtomicBool::new(false));
        let waker = Arc::new(Mutex::new(None));
        let selection = vec![PathBuf::from("existing-selection.zip")];
        let mut ui = Transfers {
            selection: selection.clone(),
            picker: Some(Box::pin(CancelledPicker {
                completed: completed.clone(),
                waker: waker.clone(),
            })),
            ..Default::default()
        };
        ui.tick(&ctx);
        assert!(ui.picker.is_some());
        assert_eq!(repaints.load(Ordering::SeqCst), 0);
        completed.store(true, Ordering::SeqCst);
        waker.lock().unwrap().take().unwrap().wake();
        assert_eq!(repaints.load(Ordering::SeqCst), 1);
        ui.tick(&ctx);
        assert!(ui.picker.is_none());
        assert_eq!(ui.selection, selection);
        assert!(ui.error.is_none());
    }

    #[test]
    fn transfer_progress_is_bounded_for_resumes_and_zero_length_bundles() {
        let mut transfer = clipboard_transfer();
        transfer.phase = Phase::Transferring;
        transfer.total = 0;
        assert_eq!(progress(&transfer), 0.0);
        transfer.total = 64;
        assert_eq!(progress(&transfer), 1.0);
        transfer.bytes = 16;
        assert_eq!(progress(&transfer), 0.25);
        transfer.phase = Phase::Completed;
        transfer.total = 0;
        transfer.bytes = 0;
        assert_eq!(progress(&transfer), 1.0);
    }

    #[test]
    fn running_transfers_precede_long_history_and_history_keeps_service_newest_first_order() {
        let mut transfers = (0..31)
            .map(|index| {
                let mut transfer = clipboard_transfer();
                transfer.id = format!("newest-first-{index}");
                transfer.clipboard_pending = false;
                transfer.clipboard_ready = true;
                transfer
            })
            .collect::<Vec<_>>();
        let mut running = clipboard_transfer();
        running.id = "old-running-transfer".into();
        running.phase = Phase::Transferring;
        transfers.push(running);
        let ordered = ordered_transfers(&transfers);
        assert_eq!(ordered.len(), 32);
        assert_eq!(ordered[0].id, "old-running-transfer");
        for (index, transfer) in ordered[1..].iter().enumerate() {
            assert_eq!(transfer.id, format!("newest-first-{index}"));
        }
    }
}
