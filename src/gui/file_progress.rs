//! Locally requested file transfers share one compact native progress window.
//! Clipboard offers stay quiet; the hidden workspace dispatches changed state.
use crate::group::{
    file_transfer::{Direction, Operation, PasteStage, PauseReason, Phase, Transfer},
    model::Workspace,
};
use eframe::egui::{self, RichText};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const SHOW_DELAY: Duration = Duration::from_millis(200);
const SNAPSHOT_GRACE: Duration = Duration::from_secs(2);
pub(super) const TITLE: &str = "Desk Switch file transfer";
#[cfg(debug_assertions)]
pub(super) static FRAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(super) fn id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("desk-switch-file-progress")
}

fn tr(text: &str) -> String {
    crate::i18n::tr(text)
}

#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    id: String,
    operation: Operation,
    direction: Direction,
    moving: bool,
    name: String,
    source: String,
    destination: String,
    current_name: Option<String>,
    files: u32,
    bytes: u64,
    total: u64,
    status: &'static str,
    pause: bool,
    resume: bool,
    cancel: bool,
    finishing: bool,
    terminal: bool,
    attention: bool,
    error: Option<String>,
    recovery_paths: Vec<std::path::PathBuf>,
}

impl Snapshot {
    fn from_transfer(transfer: &Transfer, workspace: &Workspace, resumable: bool) -> Self {
        let peer = workspace
            .members
            .get(&transfer.peer)
            .map(|member| member.node.name.clone())
            .unwrap_or_else(|| tr("Other computer"));
        let (source, destination) = match transfer.direction {
            Direction::Sending => (tr("This PC"), peer),
            Direction::Receiving => (
                peer,
                transfer
                    .paste
                    .as_ref()
                    .and_then(|paste| paste.destination.clone())
                    .filter(|destination| !destination.is_empty())
                    .unwrap_or_else(|| tr("This PC")),
            ),
        };
        Self {
            id: transfer.id.clone(),
            operation: transfer
                .paste
                .as_ref()
                .map_or(Operation::Copy, |paste| paste.operation),
            direction: transfer.direction,
            moving: transfer.paste.as_ref().is_some_and(|paste| {
                paste.operation == Operation::Move
                    && matches!(
                        paste.stage,
                        PasteStage::RemovingOriginals | PasteStage::Moved
                    )
            }),
            name: transfer.name.clone(),
            source,
            destination,
            current_name: transfer.current_name.clone(),
            files: transfer.files,
            bytes: transfer.bytes,
            total: transfer.total,
            status: status(transfer),
            pause: can_pause(transfer),
            resume: resumable
                && (matches!(transfer.phase, Phase::Paused | Phase::Failed)
                    || transfer.paste.as_ref().is_some_and(|paste| {
                        matches!(paste.stage, PasteStage::Paused | PasteStage::Failed)
                    })),
            cancel: can_cancel(transfer),
            finishing: transfer
                .paste
                .as_ref()
                .is_some_and(|paste| paste.stage == PasteStage::RemovingOriginals),
            terminal: terminal(transfer),
            attention: transfer
                .paste
                .as_ref()
                .is_some_and(|paste| paste.stage == PasteStage::OriginalsRetained),
            error: transfer.error.clone(),
            recovery_paths: transfer.recovery_paths.clone(),
        }
    }
}

pub(super) fn status(transfer: &Transfer) -> &'static str {
    if let Some(paste) = &transfer.paste {
        return match paste.stage {
            PasteStage::Preparing => "Preparing files…",
            PasteStage::Receiving => "Receiving files…",
            PasteStage::Paused => paused(transfer.pause_reason),
            PasteStage::Verifying => "Verifying files…",
            PasteStage::Delivered if paste.operation == Operation::Move => {
                "Waiting for move confirmation…"
            }
            PasteStage::Delivered if paste.reports_completion => "Finishing copy…",
            PasteStage::Delivered => "Files transferred",
            PasteStage::RemovingOriginals => "Removing originals…",
            PasteStage::OriginalsRetained => "Files copied; originals retained",
            PasteStage::Moved => "Moved",
            PasteStage::Cancelled => "Cancelled",
            PasteStage::Failed => "Transfer failed",
        };
    }
    match transfer.phase {
        Phase::Preparing => "Preparing files…",
        Phase::Connecting => "Connecting…",
        Phase::Transferring => match transfer.direction {
            Direction::Sending => "Sending files…",
            Direction::Receiving => "Receiving files…",
        },
        Phase::Paused => paused(transfer.pause_reason),
        Phase::Verifying => "Verifying files…",
        Phase::Completed => "Files transferred",
        Phase::Cancelled => "Cancelled",
        Phase::Failed => "Transfer failed",
    }
}

fn paused(reason: Option<PauseReason>) -> &'static str {
    match reason {
        Some(PauseReason::Connection) => "Waiting for the other computer…",
        Some(PauseReason::Peer) => "Paused on the other computer",
        _ => "Transfer paused",
    }
}

pub(super) fn terminal(transfer: &Transfer) -> bool {
    if let Some(paste) = &transfer.paste {
        return matches!(
            paste.stage,
            PasteStage::Cancelled | PasteStage::OriginalsRetained | PasteStage::Moved
        ) || (paste.stage == PasteStage::Delivered
            && !paste.reports_completion
            && paste.operation == Operation::Copy);
    }
    matches!(transfer.phase, Phase::Completed | Phase::Cancelled)
}

pub(super) fn can_pause(transfer: &Transfer) -> bool {
    if let Some(paste) = &transfer.paste {
        return matches!(
            paste.stage,
            PasteStage::Preparing | PasteStage::Receiving | PasteStage::Verifying
        );
    }
    matches!(
        transfer.phase,
        Phase::Preparing | Phase::Connecting | Phase::Transferring | Phase::Verifying
    )
}

pub(super) fn can_cancel(transfer: &Transfer) -> bool {
    !terminal(transfer)
        && transfer
            .paste
            .as_ref()
            .is_none_or(|paste| paste.stage != PasteStage::RemovingOriginals)
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Command {
    Pause(String),
    Resume(String),
    Cancel(String),
    Close(Vec<String>),
    OpenRecovery(std::path::PathBuf),
}

#[derive(Default)]
struct State {
    snapshots: Vec<Snapshot>,
    open: bool,
    local_error: Option<String>,
}

#[derive(Default)]
pub(super) struct Progress {
    state: Arc<Mutex<State>>,
    commands: Arc<Mutex<VecDeque<Command>>>,
    pending: BTreeMap<String, Instant>,
    displayed: BTreeSet<String>,
    announced: BTreeSet<String>,
    #[cfg(debug_assertions)]
    preview: Option<Vec<Snapshot>>,
    #[cfg(debug_assertions)]
    offscreen_probe: bool,
}

impl Progress {
    pub(super) fn started(&mut self, operation: String, ctx: &egui::Context) {
        if self.announced.insert(operation.clone()) {
            self.pending.insert(operation, Instant::now());
            ctx.request_repaint_after_for(SHOW_DELAY, egui::ViewportId::ROOT);
        }
    }

    pub(super) fn take_commands(&self) -> Vec<Command> {
        self.commands.lock().unwrap().drain(..).collect()
    }

    pub(super) fn reopen(&mut self, operation: String, ctx: &egui::Context) {
        self.announced.insert(operation.clone());
        let now = Instant::now();
        self.pending
            .insert(operation, now.checked_sub(SHOW_DELAY).unwrap_or(now));
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    pub(super) fn error(&self, error: String, ctx: &egui::Context) {
        let mut state = self.state.lock().unwrap();
        state.local_error = Some(error);
        state.open = true;
        ctx.request_repaint_of(id());
    }

    pub(super) fn close(&mut self, ctx: &egui::Context) {
        self.pending.clear();
        self.displayed.clear();
        let mut state = self.state.lock().unwrap();
        if state.open {
            state.open = false;
            state.snapshots.clear();
            ctx.send_viewport_cmd_to(id(), egui::ViewportCommand::Close);
        }
    }

    pub(super) fn update(
        &mut self,
        ctx: &egui::Context,
        workspace: &Workspace,
        transfers: &[Transfer],
        resumable: &BTreeSet<String>,
    ) {
        let snapshots = transfers
            .iter()
            .map(|transfer| {
                (
                    transfer.id.clone(),
                    Snapshot::from_transfer(transfer, workspace, resumable.contains(&transfer.id)),
                )
            })
            .collect::<BTreeMap<_, _>>();
        self.synchronize(ctx, snapshots, Instant::now());
    }

    fn synchronize(
        &mut self,
        ctx: &egui::Context,
        snapshots: BTreeMap<String, Snapshot>,
        now: Instant,
    ) {
        let mut foreground = false;
        self.pending.retain(|operation, started| {
            let age = now.saturating_duration_since(*started);
            let Some(snapshot) = snapshots.get(operation) else {
                if age < SNAPSHOT_GRACE {
                    ctx.request_repaint_after_for(SNAPSHOT_GRACE - age, egui::ViewportId::ROOT);
                    return true;
                }
                return false;
            };
            if snapshot.terminal && !snapshot.attention {
                return false;
            }
            if age < SHOW_DELAY {
                ctx.request_repaint_after_for(SHOW_DELAY - age, egui::ViewportId::ROOT);
                return true;
            }
            self.displayed.insert(operation.clone());
            foreground = true;
            false
        });
        self.displayed.retain(|operation| {
            snapshots
                .get(operation)
                .is_some_and(|snapshot| !snapshot.terminal || snapshot.attention)
        });
        self.announced.retain(|operation| {
            snapshots.contains_key(operation)
                || self.pending.contains_key(operation)
                || self.displayed.contains(operation)
        });
        let next = self
            .displayed
            .iter()
            .filter_map(|operation| snapshots.get(operation).cloned())
            .collect::<Vec<_>>();
        let mut state = self.state.lock().unwrap();
        if next.is_empty() {
            if state.open {
                state.open = false;
                ctx.send_viewport_cmd_to(id(), egui::ViewportCommand::Close);
            }
            state.snapshots.clear();
            return;
        }
        if state.snapshots != next {
            state.snapshots = next;
            ctx.request_repaint_of(id());
        }
        if foreground {
            state.open = true;
            state.local_error = None;
            ctx.send_viewport_cmd_to(id(), egui::ViewportCommand::Minimized(false));
            #[cfg(debug_assertions)]
            let activate = !self.offscreen_probe;
            #[cfg(not(debug_assertions))]
            let activate = true;
            if activate {
                ctx.send_viewport_cmd_to(id(), egui::ViewportCommand::Focus);
                #[cfg(target_os = "linux")]
                super::linux_activation::request(super::linux_activation::Role::Transfer);
            }
        }
    }

    pub(super) fn show(&self, ctx: &egui::Context) {
        #[cfg(debug_assertions)]
        if self.preview.is_some() {
            return;
        }
        if !self.state.lock().unwrap().open {
            return;
        }
        let state = self.state.clone();
        let commands = self.commands.clone();
        let root = ctx.clone();
        let viewport = egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size([480.0, 330.0])
            .with_min_inner_size([400.0, 240.0])
            .with_max_inner_size([680.0, 760.0])
            .with_taskbar(true)
            .with_active(false)
            .with_icon(egui::IconData {
                rgba: super::tray::icon_rgba(32),
                width: 32,
                height: 32,
            });
        #[cfg(debug_assertions)]
        let viewport = if self.offscreen_probe {
            viewport.with_position([-15_000.0, -15_000.0])
        } else {
            viewport
        };
        ctx.show_viewport_deferred(id(), viewport, move |ui, _| {
            let (snapshots, error) = {
                let state = state.lock().unwrap();
                (state.snapshots.clone(), state.local_error.clone())
            };
            if ui.ctx().input(|input| {
                input.viewport().close_requested() || input.key_pressed(egui::Key::Escape)
            }) {
                enqueue(
                    &commands,
                    &root,
                    Command::Close(
                        snapshots
                            .iter()
                            .filter(|snapshot| snapshot.cancel)
                            .map(|snapshot| snapshot.id.clone())
                            .collect(),
                    ),
                );
                state.lock().unwrap().open = false;
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            content(ui, &snapshots, error.as_deref(), |command| {
                state.lock().unwrap().local_error = None;
                enqueue(&commands, &root, command);
            });
        });
    }
}

fn enqueue(commands: &Mutex<VecDeque<Command>>, root: &egui::Context, command: Command) {
    let mut commands = commands.lock().unwrap();
    if !commands.contains(&command) {
        commands.push_back(command);
    }
    root.request_repaint_of(egui::ViewportId::ROOT);
}

fn bytes(value: u64) -> String {
    super::transfer::bytes(value)
}

fn content(
    ui: &mut egui::Ui,
    snapshots: &[Snapshot],
    error: Option<&str>,
    mut command: impl FnMut(Command),
) {
    #[cfg(debug_assertions)]
    FRAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(ui.visuals().window_fill())
                .inner_margin(18),
        )
        .show(ui, |ui| {
            let moving = snapshots.iter().any(|snapshot| snapshot.moving);
            let receiving = snapshots
                .iter()
                .any(|snapshot| snapshot.direction == Direction::Receiving && !snapshot.moving);
            let sending = snapshots
                .iter()
                .any(|snapshot| snapshot.direction == Direction::Sending && !snapshot.moving);
            let heading = if snapshots.iter().all(|snapshot| snapshot.terminal)
                || usize::from(moving) + usize::from(receiving) + usize::from(sending) > 1
            {
                tr("File transfers")
            } else if moving {
                tr("Moving files")
            } else if receiving && snapshots.len() == 1 {
                crate::i18n::tr(format!("Receiving files from {}", snapshots[0].source))
            } else if receiving {
                tr("Receiving files")
            } else {
                tr("Sending files")
            };
            ui.add(egui::Label::new(RichText::new(&heading).size(20.0).strong()).truncate())
                .on_hover_text(heading);
            if snapshots.iter().any(|snapshot| {
                snapshot.operation == Operation::Move && !snapshot.terminal && !snapshot.moving
            }) {
                ui.weak(tr("Originals are kept until the move is confirmed."));
            }
            ui.add_space(6.0);
            if let Some(error) = error {
                super::transfer::error_feedback(ui, error);
            }
            let active = snapshots
                .iter()
                .filter(|snapshot| snapshot.cancel)
                .collect::<Vec<_>>();
            let completed = snapshots.iter().all(|snapshot| snapshot.terminal);
            let footer = active.len() > 1 || completed;
            let content_height = (ui.available_height() - if footer { 50.0 } else { 0.0 }).max(0.0);
            egui::ScrollArea::vertical()
                .id_salt("file-progress-operations")
                .max_height(content_height)
                .min_scrolled_height(0.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for snapshot in snapshots {
                        ui.push_id(&snapshot.id, |ui| {
                            operation_card(ui, snapshot, &mut command);
                        });
                    }
                });
            if footer {
                ui.add_space(8.0);
                if active.len() > 1
                    && ui
                        .button(tr("Cancel all transfers"))
                        .on_hover_text(tr("Cancel transfers and close"))
                        .clicked()
                {
                    command(Command::Close(
                        active
                            .into_iter()
                            .map(|snapshot| snapshot.id.clone())
                            .collect(),
                    ));
                } else if completed && ui.button(tr("Close")).clicked() {
                    command(Command::Close(Vec::new()));
                }
            }
        });
}

#[cfg(debug_assertions)]
impl Progress {
    pub(super) fn native_probe_offscreen(&mut self) {
        self.offscreen_probe = true;
    }

    pub(super) fn set_preview(&mut self, case: &str) {
        let mut snapshot = Snapshot {
            id: "clipboard-progress-fixture".into(),
            operation: Operation::Copy,
            direction: Direction::Receiving,
            moving: false,
            name: "Autumn photographs and project documents".into(),
            source: "Ubuntu laptop".into(),
            destination: tr("This PC"),
            current_name: Some("Photographs / Autumn trip / IMG_2948.JPG".into()),
            files: 42,
            bytes: 2_648_000_000,
            total: 4_096_000_000,
            status: "Receiving files…",
            pause: true,
            resume: false,
            cancel: true,
            finishing: false,
            terminal: false,
            attention: false,
            error: None,
            recovery_paths: Vec::new(),
        };
        if case.ends_with("preparing") {
            snapshot.bytes = 0;
            snapshot.total = 0;
            snapshot.files = 0;
            snapshot.current_name = None;
            snapshot.status = "Preparing files…";
        } else if case.ends_with("paused") {
            snapshot.status = "Transfer paused";
            snapshot.pause = false;
            snapshot.resume = true;
        } else if case.ends_with("waiting") {
            snapshot.status = "Waiting for the other computer…";
            snapshot.pause = false;
            snapshot.resume = true;
        } else if case.ends_with("error") {
            snapshot.status = "Transfer failed";
            snapshot.pause = false;
            snapshot.resume = true;
            snapshot.error = Some("The receiving computer has no space for the remaining files. Free space and resume the transfer.".into());
        } else if case.ends_with("move") {
            snapshot.operation = Operation::Move;
            snapshot.moving = true;
            snapshot.bytes = snapshot.total;
            snapshot.status = "Removing originals…";
            snapshot.pause = false;
            snapshot.cancel = false;
            snapshot.finishing = true;
        } else if case.ends_with("retained") {
            snapshot.operation = Operation::Move;
            snapshot.bytes = snapshot.total;
            snapshot.status = "Files copied; originals retained";
            snapshot.pause = false;
            snapshot.terminal = true;
            snapshot.attention = true;
            snapshot.cancel = false;
            snapshot.error =
                Some("A source file changed after the copy. The changed original was kept.".into());
        } else if case.ends_with("recovery") {
            snapshot.operation = Operation::Move;
            snapshot.direction = Direction::Sending;
            snapshot.source = tr("This PC");
            snapshot.destination = "Ubuntu laptop".into();
            snapshot.bytes = snapshot.total;
            snapshot.status = "Files copied; originals retained";
            snapshot.pause = false;
            snapshot.cancel = false;
            snapshot.terminal = true;
            snapshot.attention = true;
            snapshot.error =
                Some("The originals are available in the recovery folder on this computer.".into());
            snapshot.recovery_paths = vec![std::path::PathBuf::from(
                "/home/demo/Documents/.desk-switch-recovery-73ba",
            )];
        }
        let mut snapshots = vec![snapshot];
        if case.ends_with("multiple") {
            let mut second = snapshots[0].clone();
            second.id = "second-progress-fixture".into();
            second.name =
                "Client presentation — reviewed source files and reference material".into();
            second.source = "Office PC".into();
            second.bytes = 148_000_000;
            second.total = 934_000_000;
            second.status = "Transfer paused";
            second.pause = false;
            second.resume = true;
            snapshots.push(second);
        }
        self.preview = Some(snapshots);
    }

    pub(super) fn preview(&self, ui: &mut egui::Ui) {
        if let Some(snapshots) = &self.preview {
            content(ui, snapshots, None, |_| {});
        }
    }
}

fn operation_card(ui: &mut egui::Ui, snapshot: &Snapshot, command: &mut impl FnMut(Command)) {
    ui.add(egui::Label::new(RichText::new(&snapshot.name).strong()).truncate())
        .on_hover_text(&snapshot.name);
    for (label, value) in [("From", &snapshot.source), ("To", &snapshot.destination)] {
        ui.add(egui::Label::new(RichText::new(format!("{} {value}", tr(label))).weak()).truncate())
            .on_hover_text(value);
    }
    ui.label(tr(snapshot.status));
    if snapshot.attention {
        ui.weak(tr(
            "The received files are kept. The originals remain on the source computer.",
        ));
    }
    if snapshot.finishing {
        ui.weak(tr("Finishing this move cannot be cancelled."));
    }
    if let Some(name) = &snapshot.current_name {
        ui.add(egui::Label::new(name).truncate())
            .on_hover_text(name);
    }
    if snapshot.total > 0 {
        let progress = (snapshot.bytes as f64 / snapshot.total as f64).clamp(0.0, 1.0) as f32;
        ui.add(
            egui::ProgressBar::new(progress)
                .text(format!("{:.0}%", progress * 100.0))
                .animate(false),
        );
        ui.weak(format!(
            "{} / {}",
            bytes(snapshot.bytes),
            bytes(snapshot.total)
        ));
    }
    if snapshot.files > 0 {
        ui.weak(crate::i18n::tr(format!("Items: {}", snapshot.files)));
    }
    if let Some(error) = &snapshot.error {
        if snapshot.attention {
            super::transfer::error_details(ui, error);
        } else {
            super::transfer::error_feedback(ui, error);
        }
    }
    ui.horizontal_wrapped(|ui| {
        if snapshot.pause && ui.button(tr("Pause transfer")).clicked() {
            command(Command::Pause(snapshot.id.clone()));
        }
        if snapshot.resume && ui.button(tr("Resume transfer")).clicked() {
            command(Command::Resume(snapshot.id.clone()));
        }
        if snapshot.cancel
            && ui
                .button(tr("Cancel transfer"))
                .on_hover_text(tr(
                    "Stops this transfer, including other pastes using the same files.",
                ))
                .clicked()
        {
            command(Command::Cancel(snapshot.id.clone()));
        }
        if let Some(directory) = super::transfer::recovery_button(ui, &snapshot.recovery_paths) {
            command(Command::OpenRecovery(directory));
        }
    });
    ui.add_space(10.0);
    ui.separator();
    ui.add_space(8.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::group::file_transfer::{Origin, PasteView};

    fn transfer() -> Transfer {
        Transfer {
            id: "paste-session".into(),
            peer: "11".repeat(32),
            direction: Direction::Receiving,
            origin: Origin::Clipboard,
            name: "Photographs".into(),
            phase: Phase::Transferring,
            bytes: 128,
            total: 256,
            files: 2,
            error: None,
            paths: Vec::new(),
            clipboard_ready: false,
            clipboard_pending: false,
            clipboard_skipped: false,
            paste: Some(PasteView {
                operation: Operation::Copy,
                stage: PasteStage::Receiving,
                destination: None,
                reports_completion: false,
            }),
            pause_reason: None,
            current_name: Some("Photographs / IMG_2948.JPG".into()),
            recovery_paths: Vec::new(),
        }
    }

    fn snapshot() -> Snapshot {
        let transfer = transfer();
        Snapshot::from_transfer(&transfer, &crate::group::model::tests::fixture(3), true)
    }

    #[test]
    fn paste_and_move_results_require_their_actual_completion_boundary() {
        let mut transfer = transfer();
        transfer.paste.as_mut().unwrap().stage = PasteStage::Delivered;
        assert_eq!(status(&transfer), "Files transferred");
        assert!(terminal(&transfer));
        transfer.paste.as_mut().unwrap().reports_completion = true;
        assert_eq!(status(&transfer), "Finishing copy…");
        assert!(!terminal(&transfer));
        transfer.paste.as_mut().unwrap().operation = Operation::Move;
        assert_eq!(status(&transfer), "Waiting for move confirmation…");
        assert!(!terminal(&transfer));
        transfer.paste.as_mut().unwrap().stage = PasteStage::RemovingOriginals;
        assert_eq!(status(&transfer), "Removing originals…");
        assert!(!terminal(&transfer));
        assert!(!can_pause(&transfer));
        assert!(
            !can_cancel(&transfer),
            "confirmed source cleanup must not advertise rollback"
        );
        transfer.paste.as_mut().unwrap().stage = PasteStage::OriginalsRetained;
        assert_eq!(status(&transfer), "Files copied; originals retained");
        assert!(terminal(&transfer));
        transfer.paste.as_mut().unwrap().stage = PasteStage::Moved;
        assert_eq!(status(&transfer), "Moved");
        assert!(terminal(&transfer));
    }

    #[test]
    fn only_a_started_transfer_opens_progress_and_fast_completions_stay_quiet() {
        let ctx = egui::Context::default();
        let mut progress = Progress::default();
        let mut view = snapshot();
        let now = Instant::now();
        progress.synchronize(&ctx, [(view.id.clone(), view.clone())].into(), now);
        assert!(
            !progress.state.lock().unwrap().open,
            "passive snapshots must not open a window"
        );
        progress.started(view.id.clone(), &ctx);
        let start = progress.pending[&view.id];
        progress.synchronize(
            &ctx,
            [(view.id.clone(), view.clone())].into(),
            start + SHOW_DELAY / 2,
        );
        assert!(!progress.state.lock().unwrap().open);
        view.terminal = true;
        progress.synchronize(&ctx, [(view.id.clone(), view)].into(), start + SHOW_DELAY);
        assert!(!progress.state.lock().unwrap().open);
        assert!(progress.pending.is_empty());
    }

    #[test]
    fn repeated_reads_and_progress_updates_do_not_reactivate_the_window() {
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        let mut progress = Progress::default();
        let mut view = snapshot();
        progress.started(view.id.clone(), &ctx);
        let start = progress.pending[&view.id];
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            progress.synchronize(
                ui.ctx(),
                [(view.id.clone(), view.clone())].into(),
                start + SHOW_DELAY,
            );
            progress.show(ui.ctx());
        });
        assert!(progress.state.lock().unwrap().open);
        assert!(
            output.viewport_output[&id()]
                .commands
                .contains(&egui::ViewportCommand::Focus)
        );
        progress.started(view.id.clone(), &ctx);
        assert!(
            progress.pending.is_empty(),
            "one shared fetch must not reopen on every reader"
        );
        view.bytes += 1;
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            progress.synchronize(
                ui.ctx(),
                [(view.id.clone(), view.clone())].into(),
                start + SHOW_DELAY * 2,
            );
            progress.show(ui.ctx());
        });
        assert!(
            !output
                .viewport_output
                .get(&id())
                .is_some_and(|output| output.commands.contains(&egui::ViewportCommand::Focus))
        );
    }

    #[test]
    fn original_retention_stays_visible_and_child_commands_wake_root() {
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        let mut progress = Progress::default();
        let mut view = snapshot();
        view.terminal = true;
        view.attention = true;
        progress.started(view.id.clone(), &ctx);
        let start = progress.pending[&view.id];
        progress.synchronize(
            &ctx,
            [(view.id.clone(), view.clone())].into(),
            start + SHOW_DELAY,
        );
        assert!(
            progress.state.lock().unwrap().open,
            "a partially completed move needs its visible result"
        );
        for _ in 0..8 {
            let _ = ctx.run_ui(egui::RawInput::default(), |_| {});
        }
        let repaints = Arc::new(Mutex::new(Vec::new()));
        let reported = repaints.clone();
        ctx.set_request_repaint_callback(move |request| {
            reported.lock().unwrap().push(request.viewport_id)
        });
        let mut input = egui::RawInput {
            viewport_id: id(),
            ..Default::default()
        };
        input.viewports.insert(
            id(),
            egui::ViewportInfo {
                parent: Some(egui::ViewportId::ROOT),
                ..Default::default()
            },
        );
        let _ = ctx.run_ui(input, |ui| {
            repaints.lock().unwrap().clear();
            enqueue(
                &progress.commands,
                ui.ctx(),
                Command::Resume("shared-fetch".into()),
            );
        });
        assert!(repaints.lock().unwrap().contains(&egui::ViewportId::ROOT));
        assert_eq!(
            progress.take_commands(),
            [Command::Resume("shared-fetch".into())]
        );
    }
}
