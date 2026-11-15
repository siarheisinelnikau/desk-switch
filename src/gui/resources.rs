//! Folder pairs and USB permissions share the existing files entry point.
use super::{Desktop, Page};
use crate::group::{
    model::Workspace,
    resources::{
        self, Command, Snapshot,
        sync::{Change, Node, Resolution, Side},
    },
};
use eframe::egui::{self, RichText};
use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};

fn tr(text: &str) -> String {
    crate::i18n::tr(text)
}
type Picker = Pin<Box<dyn Future<Output = Option<rfd::FileHandle>> + Send>>;
#[derive(Default)]
pub(super) struct Panel {
    open: bool,
    usb: bool,
    peer: String,
    folder_name: String,
    path: String,
    local: String,
    remote: String,
    pair_name: String,
    picker: Option<Picker>,
    error: Option<String>,
    service_error: Option<String>,
    retry: bool,
}
struct PickerWake(egui::Context);
impl Wake for PickerWake {
    fn wake(self: Arc<Self>) {
        self.0.request_repaint();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.request_repaint();
    }
}
impl Panel {
    pub(super) fn close(&mut self) {
        self.open = false;
    }
    pub(super) fn tick(&mut self, ctx: &egui::Context) {
        if let Some(picker) = &mut self.picker {
            let waker = Waker::from(Arc::new(PickerWake(ctx.clone())));
            if let Poll::Ready(value) = picker.as_mut().poll(&mut Context::from_waker(&waker)) {
                self.picker = None;
                if let Some(value) = value {
                    self.path = value.path().to_string_lossy().into_owned();
                }
            }
        }
    }
    fn show(
        &mut self,
        ctx: &egui::Context,
        state: &Workspace,
        own: &str,
        snapshot: &Snapshot,
    ) -> (Vec<Command>, Option<String>, bool, bool) {
        let mut commands = Vec::new();
        let mut detach = None;
        let mut transfers = false;
        let mut pause = false;
        if !self.open {
            return (commands, detach, transfers, pause);
        }
        if !state.members.contains_key(&self.peer) || self.peer == own {
            self.peer = state
                .members
                .keys()
                .find(|key| key.as_str() != own)
                .cloned()
                .unwrap_or_default();
        }
        let max =
            (ctx.content_rect().size() - egui::vec2(48.0, 132.0)).max(egui::vec2(320.0, 240.0));
        let mut open = self.open;
        egui::Window::new(tr("Files & USB")).id(egui::Id::new("folder-usb-workspace"))
            .open(&mut open).collapsible(false).default_pos(ctx.content_rect().min + egui::vec2(70.0, 100.0))
            .default_size(egui::vec2(780.0, 640.0).min(max)).min_size(egui::vec2(440.0, 300.0).min(max)).max_size(max)
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button(tr("Send files")).clicked() { transfers = true; }
                    ui.selectable_value(&mut self.usb, false, tr("Folder sync"));
                    ui.selectable_value(&mut self.usb, true, tr("USB devices"));
                });
                ui.separator();
                if self.service_error.is_some() && ui.button(tr("Retry sharing")).clicked() { self.retry = true; }
                for key in &snapshot.imports {
                    ui.horizontal_wrapped(|ui| { ui.label(format!("USB · {}", key.rsplit('/').next().unwrap_or(key))); if ui.button(tr("Detach")).clicked() { detach = Some(key.clone()); } });
                }
                for (id, owner) in &snapshot.owners {
                    ui.horizontal_wrapped(|ui| { ui.label(format!("USB · {id} · {}", peer_name(state, owner))); if ui.button(tr("Detach")).clicked() { detach = Some(id.clone()); } });
                }
                if snapshot.busy {
                    ui.horizontal_wrapped(|ui| { ui.strong(tr(&snapshot.status)); if ui.button(tr("Pause")).clicked() { pause = true; } });
                    if snapshot.total > 0 { ui.add(egui::ProgressBar::new((snapshot.bytes as f64 / snapshot.total as f64).clamp(0.0, 1.0) as f32).show_percentage()); }
                    if !snapshot.current.is_empty() { ui.add(egui::Label::new(&snapshot.current).truncate()); }
                } else if !snapshot.status.is_empty() { ui.strong(tr(&snapshot.status)); }
                if let Some(error) = self.error.as_ref().or(snapshot.error.as_ref()).or(self.service_error.as_ref()) {
                    ui.colored_label(ui.visuals().error_fg_color, tr(error_summary(error)));
                    egui::CollapsingHeader::new(tr("Details")).id_salt("resource-error").show(ui, |ui| { ui.add(egui::Label::new(error).wrap()); });
                }
                egui::ScrollArea::vertical().id_salt("resources-scroll").auto_shrink([false, false]).show(ui, |ui| {
                    ui.add_enabled_ui(!snapshot.busy, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(tr("Computer"));
                            egui::ComboBox::from_id_salt("resource-peer").width(220.0)
                                .selected_text(state.members.get(&self.peer).map(|m| m.node.name.clone()).unwrap_or_else(|| tr("Choose a computer")))
                                .show_ui(ui, |ui| { for (key, member) in &state.members { if key != own { ui.selectable_value(&mut self.peer, key.clone(), &member.node.name); } } });
                            if ui.add_enabled(!self.peer.is_empty(), egui::Button::new(tr("Refresh shared items"))).clicked() { commands.push(Command::RefreshPeer(self.peer.clone())); }
                        });
                        if self.peer.is_empty() { ui.weak(tr("Add a computer before sharing folders or USB devices.")); }
                        else if !snapshot.catalogs.contains_key(&self.peer) { ui.weak(tr("Refresh to see folders and USB devices explicitly shared by this computer.")); }
                        ui.add_space(8.0);
                        if self.usb { self.usb_ui(ui, state, snapshot, &mut commands, &mut detach); }
                        else { self.folders_ui(ui, state, snapshot, &mut commands); }
                    });
                });
            });
        self.open = open;
        (commands, detach, transfers, pause)
    }
    fn folders_ui(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        view: &Snapshot,
        commands: &mut Vec<Command>,
    ) {
        ui.label(RichText::new(tr("Folder pairs")).size(19.0).strong());
        ui.weak(tr(
            "Compare first. Review changes and conflicts, then synchronize.",
        ));
        if view.pairs.is_empty() {
            ui.label(tr(
                "No folder pairs yet. Share one folder on each computer, then link them below.",
            ));
        }
        for pair in &view.pairs {
            ui.push_id(&pair.id, |ui| { section(ui, |ui| {
                ui.strong(&pair.name);
                if let Some(folder) = view.folders.iter().find(|f| f.id == pair.local) { ui.weak(format!("↔ {}", peer_name(workspace, &folder.peer))); ui.add(egui::Label::new(folder.root.display().to_string()).wrap()); }
                ui.horizontal_wrapped(|ui| {
                    if ui.button(tr(if pair.pending.is_some() { "Compare again" } else { "Compare folders" })).clicked() { commands.push(Command::Compare(pair.id.clone())); }
                    let ready = pair.pending.as_ref().is_some_and(|p| p.conflicts.is_empty() && !p.operations.is_empty());
                    ui.add_enabled_ui(ready, |ui| { if super::view::primary(ui, if pair.attempted { "Resume sync" } else { "Synchronize" }).clicked() { commands.push(Command::Run(pair.id.clone())); } });
                    ui.menu_button(tr("More"), |ui| {
                        if ui.button(tr("Remove pair")).clicked() { commands.push(Command::RemovePair(pair.id.clone())); ui.close(); }
                        ui.weak(tr("Removing a pair keeps files and previous versions."));
                    });
                });
                if pair.baseline.is_none() { ui.weak(tr("First sync keeps files missing from one side.")); }
                if let Some(plan) = &pair.pending {
                    if plan.operations.is_empty() && plan.conflicts.is_empty() { ui.label(tr("No changes to apply")); }
                    else { ui.label(crate::i18n::tr(format!("Planned changes: {} · Conflicts: {}", plan.operations.len(), plan.conflicts.len()))); }
                    let conflicts: Vec<_> = plan.conflicts.iter().chain(pair.choices.keys()).collect();
                    if !conflicts.is_empty() {
                        ui.strong(tr("Resolve different versions"));
                        ui.weak(tr("Both computers changed this item. Choosing a missing version deletes the other copy; its previous version is kept."));
                        egui::ScrollArea::vertical().id_salt("sync-conflicts").max_height(250.0).show_rows(ui, 76.0, conflicts.len(), |ui, range| {
                            for index in range {
                                let path = conflicts[index]; ui.add(egui::Label::new(path).truncate());
                                ui.horizontal_wrapped(|ui| {
                                    ui.weak(format!("{}: {} · {}: {}", tr("This PC"), node_label(plan.local.get(path)), tr("Other computer"), node_label(plan.remote.get(path))));
                                    let mut choice = pair.choices.get(path).copied();
                                    ui.add_enabled_ui(!pair.attempted, |ui| {
                                        egui::ComboBox::from_id_salt(("sync-choice", path)).selected_text(tr(match choice { None => "Choose a version", Some(Resolution::Local) => "Use this PC", Some(Resolution::Remote) => "Use other computer", Some(Resolution::Skip) => "Skip for now" }))
                                            .show_ui(ui, |ui| { for (value, label) in [(Resolution::Local, "Use this PC"), (Resolution::Remote, "Use other computer"), (Resolution::Skip, "Skip for now")] { ui.selectable_value(&mut choice, Some(value), tr(label)); } });
                                    });
                                    if choice != pair.choices.get(path).copied() && let Some(choice) = choice { commands.push(Command::Resolve { pair: pair.id.clone(), choices: BTreeMap::from([(path.clone(), choice)]) }); }
                                });
                            }
                        });
                    }
                    egui::CollapsingHeader::new(tr("Planned changes")).id_salt("sync-plan").show(ui, |ui| {
                        egui::ScrollArea::vertical().max_height(230.0).id_salt("sync-operations").show_rows(ui, 50.0, plan.operations.len(), |ui, range| {
                            for op in &plan.operations[range] {
                                ui.add(egui::Label::new(&op.path).truncate());
                                ui.weak(tr(match (&op.change, op.side) {
                                    (Change::Delete, Side::Local) => "Delete on this PC", (Change::Delete, Side::Remote) => "Delete on other computer",
                                    (Change::Rename { .. }, Side::Local) => "Rename on this PC", (Change::Rename { .. }, Side::Remote) => "Rename on other computer",
                                    (_, Side::Local) => "Copy to this PC", (_, Side::Remote) => "Copy to other computer",
                                }));
                                if let Change::Rename { from, .. } = &op.change { ui.add(egui::Label::new(format!("← {from}")).truncate()); }
                            }
                        });
                    });
                }
            }); });
        }
        ui.add_space(8.0);
        egui::CollapsingHeader::new(tr("Share a local folder")).default_open(view.folders.is_empty()).show(ui, |ui| {
            ui.weak(tr("Allow the selected computer to read and change this folder during an explicit synchronization."));
            ui.add(egui::TextEdit::singleline(&mut self.folder_name).hint_text(tr("Folder name")).desired_width(f32::INFINITY));
            ui.horizontal(|ui| {
                ui.add_sized([ui.available_width().max(200.0) - 125.0, 34.0], egui::TextEdit::singleline(&mut self.path).hint_text(tr("Local folder path")));
                if ui.add_enabled(self.picker.is_none(), egui::Button::new(tr("Choose folder…"))).clicked() { self.picker = Some(Box::pin(rfd::AsyncFileDialog::new().set_title(tr("Choose folder…")).pick_folder())); }
            });
            if ui.add_enabled(!self.peer.is_empty() && !self.path.is_empty() && !self.folder_name.trim().is_empty(), egui::Button::new(tr("Share folder"))).clicked() {
                commands.push(Command::AddFolder { name: self.folder_name.clone(), peer: self.peer.clone(), root: self.path.clone().into() });
            }
        });
        for folder in &view.folders {
            ui.push_id(&folder.id, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(&folder.name);
                    if ui.small_button(tr("Previous versions")).clicked() {
                        commands.push(Command::Versions(folder.id.clone()));
                    }
                    if ui.small_button(tr("Stop sharing")).clicked() {
                        commands.push(Command::RemoveFolder(folder.id.clone()));
                    }
                });
            });
        }
        egui::CollapsingHeader::new(tr("Link two folders"))
            .default_open(view.pairs.is_empty() && !view.folders.is_empty())
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.pair_name)
                        .hint_text(tr("Pair name"))
                        .desired_width(f32::INFINITY),
                );
                ui.label(tr("On this PC"));
                egui::ComboBox::from_id_salt("sync-local")
                    .width(ui.available_width().min(400.0))
                    .selected_text(
                        view.folders
                            .iter()
                            .find(|f| f.id == self.local)
                            .map(|f| f.name.clone())
                            .unwrap_or_else(|| tr("Choose a folder")),
                    )
                    .show_ui(ui, |ui| {
                        for folder in view.folders.iter().filter(|f| f.peer == self.peer) {
                            ui.selectable_value(&mut self.local, folder.id.clone(), &folder.name);
                        }
                    });
                ui.label(tr("On other computer"));
                let folders = view
                    .catalogs
                    .get(&self.peer)
                    .map(|c| c.folders.as_slice())
                    .unwrap_or_default();
                egui::ComboBox::from_id_salt("sync-remote")
                    .width(ui.available_width().min(400.0))
                    .selected_text(
                        folders
                            .iter()
                            .find(|f| f.id == self.remote)
                            .map(|f| f.name.clone())
                            .unwrap_or_else(|| tr("Choose a folder")),
                    )
                    .show_ui(ui, |ui| {
                        for folder in folders {
                            ui.selectable_value(&mut self.remote, folder.id.clone(), &folder.name);
                        }
                    });
                if ui
                    .add_enabled(
                        !self.local.is_empty()
                            && !self.remote.is_empty()
                            && !self.pair_name.trim().is_empty(),
                        egui::Button::new(tr("Save folder pair")),
                    )
                    .clicked()
                {
                    commands.push(Command::AddPair {
                        name: self.pair_name.clone(),
                        local: self.local.clone(),
                        remote: self.remote.clone(),
                    });
                }
            });
        if !view.recovery.is_empty() {
            ui.separator();
            ui.strong(tr("Previous versions"));
            ui.weak(tr("Restore keeps the current file as another recoverable version. Compare again after restoring."));
            for recovery in &view.recovery {
                ui.push_id(&recovery.version.id, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(&recovery.version.path);
                        ui.weak(format!(
                            "{} · {}",
                            saved_age(recovery.version.saved_at),
                            node_label(Some(&recovery.version.value))
                        ));
                        if ui
                            .add_enabled(
                                matches!(recovery.version.value, Node::File { .. }),
                                egui::Button::new(tr("Restore version")),
                            )
                            .clicked()
                        {
                            commands.push(Command::Restore {
                                folder: recovery.folder.clone(),
                                version: recovery.version.id.clone(),
                            });
                        }
                    });
                });
            }
        }
    }
    fn usb_ui(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        view: &Snapshot,
        commands: &mut Vec<Command>,
        detach: &mut Option<String>,
    ) {
        ui.label(RichText::new(tr("USB devices")).size(19.0).strong());
        ui.weak(tr(
            "One computer owns a device at a time. Reconnecting never attaches it automatically.",
        ));
        egui::CollapsingHeader::new(tr("Supported devices and setup")).show(ui, |ui| {
            ui.label(tr("Experimental USB/IP support for printers, USB serial adapters and vendor-specific devices. Storage, hubs, keyboards, audio and cameras are excluded."));
            ui.label(tr("Export requires an installed USB/IP backend with explicitly exported devices on localhost:3240. Import currently requires Ubuntu, usbip and vhci_hcd with existing permissions."));
            ui.label(tr("The raw backend must be restricted to localhost by your existing system configuration. Desk Switch does not install drivers, request administrator access or change firewall rules."));
            ui.label(tr("Detaching is a physical disconnect from the receiving computer. Finish device operations first."));
        });
        section(ui, |ui| {
            ui.strong(tr("On this PC"));
            if ui.button(tr("Refresh USB devices")).clicked() {
                commands.push(Command::RefreshUsb);
            }
            if view.devices.is_empty() {
                ui.weak(tr(
                    "Refresh to list devices already exported by your USB/IP backend.",
                ));
            }
            for device in &view.devices {
                ui.push_id(&device.bus_id, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("{} · {}", device.name, device.bus_id));
                        if !device.supported {
                            ui.weak(tr("Unsupported device class"));
                            return;
                        }
                        if let Some(owner) = view.owners.get(&device.bus_id) {
                            ui.label(crate::i18n::tr(format!(
                                "In use by {}",
                                peer_name(workspace, owner)
                            )));
                            if ui.button(tr("Detach")).clicked() {
                                *detach = Some(device.bus_id.clone());
                            }
                        } else if ui
                            .add_enabled(
                                !self.peer.is_empty(),
                                egui::Button::new(tr("Share with selected computer")),
                            )
                            .clicked()
                        {
                            commands.push(Command::ShareUsb {
                                device: device.clone(),
                                peer: self.peer.clone(),
                            });
                        }
                        if view
                            .usb_grants
                            .iter()
                            .any(|g| g.grant.device.bus_id == device.bus_id)
                            && ui.button(tr("Stop sharing")).clicked()
                        {
                            commands.push(Command::UnshareUsb(device.bus_id.clone()));
                        }
                    });
                    usb_error(ui, view, &device.bus_id);
                });
            }
        });
        section(ui, |ui| {
            ui.strong(tr("On other computer"));
            if let Some(catalog) = view.catalogs.get(&self.peer) {
                if catalog.usb.is_empty() {
                    ui.weak(tr("This computer has not shared any USB devices with you."));
                }
                for device in &catalog.usb {
                    let key = format!("{}/{}", self.peer, device.bus_id);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("{} · {}", device.name, device.bus_id));
                        if view.imports.contains(&key) { ui.strong(tr("Attached")); if ui.button(tr("Detach")).clicked() { *detach = Some(key.clone()); } }
                        else {
                            if let Some(owner) = catalog.owners.get(&device.bus_id) { ui.label(crate::i18n::tr(format!("In use by {}", peer_name(workspace, owner)))); }
                            if ui.add_enabled(cfg!(target_os = "linux") && !catalog.owners.contains_key(&device.bus_id), egui::Button::new(tr("Attach"))).on_disabled_hover_text(tr("USB import is currently available on Ubuntu only. Refresh after the current owner detaches.")).clicked() { commands.push(Command::AttachUsb { device: device.clone(), peer: self.peer.clone() }); }
                        }
                    });
                    usb_error(ui, view, &key);
                }
            }
        });
    }
}
fn usb_error(ui: &mut egui::Ui, view: &Snapshot, key: &str) {
    if let Some(error) = view.usb_errors.get(key) {
        ui.colored_label(ui.visuals().error_fg_color, tr(error_summary(error)));
        egui::CollapsingHeader::new(tr("Details"))
            .id_salt(("usb-error", key))
            .show(ui, |ui| {
                ui.add(egui::Label::new(error).wrap());
            });
    }
}
fn section(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style())
        .inner_margin(16)
        .corner_radius(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            content(ui);
        });
}
fn peer_name(workspace: &Workspace, key: &str) -> String {
    workspace
        .members
        .get(key)
        .map(|m| m.node.name.clone())
        .unwrap_or_else(|| tr("Other computer"))
}
fn node_label(node: Option<&Node>) -> String {
    match node {
        None => tr("Missing"),
        Some(Node::Directory) => tr("Folder"),
        Some(Node::File { size, .. }) => super::transfer::bytes(*size),
    }
}
fn saved_age(saved_at: u64) -> String {
    if saved_at == 0 {
        return tr("Previous versions");
    }
    let age = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().saturating_sub(saved_at))
        .unwrap_or(0);
    if age < 3600 {
        crate::i18n::tr(format!("Saved {} min ago", age / 60))
    } else {
        crate::i18n::tr(format!("Saved {} h ago", age / 3600))
    }
}
fn error_summary(error: &str) -> &'static str {
    if error.contains("backend") || error.contains("usbip") || error.contains("vhci") {
        "USB backend unavailable. Check Supported devices and setup."
    } else if error.contains("changed") || error.contains("colli") || error.contains("verification")
    {
        "A file changed or failed verification. Compare again; previous versions are kept."
    } else if error.contains("unavailable") || error.contains("connect") || error.contains("10013")
    {
        "Could not reach the other computer. Check the connection and update both computers."
    } else {
        "Action failed. Open Details for the reason."
    }
}
impl Desktop {
    pub(super) fn start_resources(&mut self, ctx: &egui::Context) {
        if self.resources.is_none() {
            match resources::Service::start(self.dir.clone(), ctx.clone()) {
                Ok(service) => {
                    self.resources = Some(service);
                    self.resource_panel.service_error = None;
                }
                Err(error) => self.resource_panel.service_error = Some(format!("{error:#}")),
            }
        }
    }
    pub(super) fn open_resources(&mut self, usb: bool) {
        self.transfers.close();
        self.group.add_open = false;
        self.group.layout_editor.close();
        self.page = Page::Overview;
        self.resource_panel.open = true;
        self.resource_panel.usb = usb;
    }
    pub(super) fn resource_window(&mut self, ctx: &egui::Context) {
        let Some(workspace) = &self.group.workspace else {
            return;
        };
        let snapshot = self
            .resources
            .as_ref()
            .map(|s| s.snapshot())
            .unwrap_or_default();
        let (commands, detach, transfers, pause) =
            self.resource_panel
                .show(ctx, workspace, &self.public_key, &snapshot);
        if std::mem::take(&mut self.resource_panel.retry) {
            self.start_resources(ctx);
        }
        if pause && let Some(service) = &self.resources {
            service.pause();
        }
        if let Some(key) = detach
            && let Some(service) = &self.resources
        {
            service.detach(&key);
        }
        for command in commands {
            let result = self
                .resources
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Folder/USB sharing is unavailable"))
                .and_then(|service| service.submit(command));
            self.resource_panel.error = result.err().map(|error| format!("{error:#}"));
        }
        if transfers {
            self.open_transfers(None);
        }
    }
}
