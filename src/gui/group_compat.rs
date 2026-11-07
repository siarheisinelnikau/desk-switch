//! Metadata-only bridge to the saved two-PC partner. This keeps the stable
//! management/update protocol available while a workspace is upgraded to 0.5+.
use super::Desktop;
use crate::{
    compatibility::Version,
    config,
    group::{model::Workspace, net::Action},
    workspace::{
        self, Node, Shared, management,
        net::{Event, Service},
        update,
    },
};
use anyhow::{Context, Result, ensure};
use eframe::egui;
use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};

// The old transport tries at most eight addresses for four seconds each, then
// allows an eight-second reverse connection. Do not queue overlapping attempts.
const DIAL_TIMEOUT: Duration = Duration::from_secs(48);
const LISTENER_RETRY: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(super) struct Compat {
    pub remote: Option<Node>,
    pinned: Option<String>,
    connected: bool,
    suspended: bool,
    pending: bool,
    failures: u32,
    retry_at: Option<Instant>,
    listener_retry_at: Option<Instant>,
    last_published: Option<Node>,
    address: Option<SocketAddr>,
}

fn current_group_peer(state: Option<&Workspace>, target: &str) -> bool {
    state
        .and_then(|state| state.members.get(target))
        .is_some_and(|member| member.compatible())
}

fn outgoing_node(mut node: Node) -> Node {
    // Old Node validators accept exactly the two known identities. Group-wide
    // selections and per-PC maps must never leak into the old data-plan schema.
    node.shared = Shared::default();
    node.ack.clear();
    node
}

impl Compat {
    pub(super) fn legacy_target(&self, target: &str, state: Option<&Workspace>) -> bool {
        !self.suspended
            && self.pinned.as_deref() == Some(target)
            && !current_group_peer(state, target)
            && self.remote.as_ref().is_none_or(|node| {
                node.key == target && Version::current().require(node.version.as_ref()).is_err()
            })
    }

    fn disconnected(&mut self, now: Instant) {
        self.connected = false;
        self.pending = false;
        self.failures = self.failures.saturating_add(1).min(5);
        let delay = Duration::from_secs((3u64 << self.failures.saturating_sub(1)).min(60));
        self.retry_at = Some(now + delay);
    }

    fn accept_node(&mut self, own: &str, node: Node) -> Result<()> {
        let pinned = self
            .pinned
            .as_deref()
            .context("No saved management partner")?;
        node.validate(own, pinned)?;
        ensure!(
            node.key == pinned,
            "The management partner identity changed"
        );
        self.remote = Some(node);
        self.connected = true;
        self.pending = false;
        self.failures = 0;
        self.retry_at = None;
        Ok(())
    }

    fn mirror(&self, state: &mut Workspace) {
        let Some(remote) = &self.remote else { return };
        if self.pinned.as_deref() != Some(remote.key.as_str())
            || current_group_peer(Some(state), &remote.key)
        {
            return;
        }
        if let Some(member) = state.members.get_mut(&remote.key) {
            // Only observations come from the old protocol. The controller's
            // group settings and each member's preferences remain authoritative.
            member.node = remote.clone();
            // A new binary seen on the old channel is not yet a group member.
            // Wait for its group handshake before allowing native data routes.
            member.online =
                self.connected && Version::current().require(remote.version.as_ref()).is_err();
        }
    }
}

fn legacy_management_message(message: &management::Message) -> bool {
    matches!(
        message,
        management::Message::CheckUpdates { .. }
            | management::Message::SetApplication { .. }
            | management::Message::ExtractCable { .. }
            | management::Message::Reply { .. }
    )
}

impl Desktop {
    pub(super) fn legacy_management_target(&self, target: &str) -> bool {
        self.group
            .compat
            .legacy_target(target, self.group.workspace.as_ref())
    }

    pub(super) fn send_legacy_management(
        &self,
        target: &str,
        message: management::Message,
    ) -> Result<()> {
        ensure!(
            self.legacy_management_target(target),
            "Unknown saved management partner"
        );
        ensure!(
            self.group.compat.connected,
            "The saved partner's management connection is offline"
        );
        ensure!(
            legacy_management_message(&message),
            "Update Desk Switch on the saved partner before using this action"
        );
        self.pair
            .service
            .as_ref()
            .context("The saved partner's management connection is unavailable")?
            .manage(message)
    }

    pub(super) fn update_send_to(&self, target: &str, message: update::Message) -> Result<()> {
        message.validate()?;
        if self.legacy_management_target(target) {
            ensure!(
                self.group.compat.connected,
                "The saved partner's management connection is offline"
            );
            self.pair
                .service
                .as_ref()
                .context("The saved partner's management connection is unavailable")?
                .update(message)
        } else if let Some(service) = &self.group.service {
            service.action(Action::Update {
                target: target.into(),
                message,
            })
        } else {
            self.pair
                .service
                .as_ref()
                .context("Waiting to reconnect before continuing the paired update.")?
                .update(message)
        }
    }

    /// Actual transport availability, including the saved partner during upgrade.
    pub(super) fn update_target_connected(&self, target: &str) -> bool {
        if self.legacy_management_target(target) {
            self.group.compat.connected
        } else {
            current_group_peer(self.group.workspace.as_ref(), target)
                || self.group.workspace.is_none() && self.pair.remote.is_some()
        }
    }

    pub(super) fn tick_group_compat(&mut self, ctx: &egui::Context) {
        if self.group.preview {
            return;
        }
        let pinned = config::load_peer(&self.dir)
            .ok()
            .map(|key| config::hex(&key));
        if pinned != self.group.compat.pinned {
            if self.group.compat.pinned.is_some() {
                self.pair.service.take();
                self.group.compat.connected = false;
                if let Some(state) = &mut self.group.workspace {
                    self.group.compat.mirror(state);
                }
            }
            self.group.compat = Compat {
                pinned: pinned.clone(),
                ..Default::default()
            };
        }
        let Some(pinned) = pinned else { return };
        let now = Instant::now();
        if current_group_peer(self.group.workspace.as_ref(), &pinned)
            && self.paired_update.is_none()
        {
            self.group.compat.suspended = true;
            self.group.compat.connected = false;
            self.pair.service.take();
            self.pair.status.clear();
            return;
        }
        if self.group.compat.suspended || self.quitting {
            return;
        }
        if self.pair.service.is_none() && !self.install_pending && self.install_job.is_none() {
            if let Some(deadline) = self.group.compat.listener_retry_at
                && deadline > now
            {
                ctx.request_repaint_after_for(deadline - now, egui::ViewportId::ROOT);
                return;
            }
            let snapshot = outgoing_node(self.local_node());
            match Service::start(
                self.dir.clone(),
                snapshot.clone(),
                workspace::PORT,
                ctx.clone(),
            ) {
                Ok(service) => {
                    self.pair.service = Some(service);
                    self.group.compat.last_published = Some(snapshot);
                    self.group.compat.listener_retry_at = None;
                }
                Err(error) => {
                    self.pair.status = format!(
                        "Legacy management: {}",
                        management::status_text(error.to_string(), 1024)
                    );
                    self.group.compat.listener_retry_at = Some(now + LISTENER_RETRY);
                    ctx.request_repaint_after_for(LISTENER_RETRY, egui::ViewportId::ROOT);
                    return;
                }
            }
        }
        let events = self
            .pair
            .service
            .as_ref()
            .map(|service| service.poll().collect::<Vec<_>>())
            .unwrap_or_default();
        for event in events {
            match event {
                Event::Connected { key, address, .. } if key == pinned => {
                    self.group.compat.connected = true;
                    self.group.compat.pending = false;
                    self.group.compat.failures = 0;
                    self.group.compat.retry_at = None;
                    self.group.compat.address = Some(address);
                    self.pair.status.clear();
                    if self.group.update_target.as_deref() == Some(&pinned)
                        && let Some(message) = self
                            .paired_update
                            .as_ref()
                            .and_then(update::Coordinator::retry)
                    {
                        let _ = self.update_send_to(&pinned, message);
                    }
                }
                Event::Connected { .. } => {
                    // The legacy listener itself pins the root peer. Keep a
                    // second guard here rather than adopting another identity.
                    self.pair.service.take();
                    self.group.compat.disconnected(now);
                }
                Event::State(node) => {
                    if let Err(error) = self.group.compat.accept_node(&self.public_key, *node) {
                        self.pair.status = format!(
                            "Legacy management: {}",
                            management::status_text(error.to_string(), 1024)
                        );
                        self.pair.service.take();
                        self.group.compat.disconnected(now);
                    }
                }
                Event::Management(message) if self.group.compat.connected => {
                    self.group.reply_target = Some(pinned.clone());
                    self.receive_management(message, ctx);
                    self.group.reply_target = None;
                }
                Event::Update(message) if self.group.compat.connected => {
                    if self
                        .group
                        .update_target
                        .as_ref()
                        .is_none_or(|target| target == &pinned)
                    {
                        self.group.update_target = Some(pinned.clone());
                        if let Some(remote) = &self.group.compat.remote {
                            self.pair.remote = Some(remote.clone());
                        }
                        self.receive_update(message, ctx);
                    }
                }
                Event::UpdateSent(message)
                    if self.group.update_target.as_deref() == Some(&pinned) =>
                {
                    self.sent_update(&message, ctx)
                }
                Event::Closed(error) => {
                    self.group.compat.disconnected(now);
                    if !error.is_empty() {
                        self.pair.status = format!(
                            "Legacy management: {}",
                            management::status_text(error, 1024)
                        );
                    }
                }
                Event::Request(request) => {
                    if let Some(service) = &self.pair.service {
                        service.approve(&request.key, false);
                    }
                }
                Event::Pairing(_) => {}
                Event::Forgotten => {
                    self.pair.service.take();
                    self.group.compat.connected = false;
                    if let Some(state) = &mut self.group.workspace {
                        self.group.compat.mirror(state);
                    }
                    self.group.compat = Default::default();
                }
                _ => {}
            }
        }
        if let Some(state) = &mut self.group.workspace {
            self.group.compat.mirror(state);
        }
        let snapshot = outgoing_node(self.local_node());
        if self.group.compat.last_published.as_ref() != Some(&snapshot) {
            if let Some(service) = &self.pair.service {
                service.publish(snapshot.clone());
            }
            self.group.compat.last_published = Some(snapshot);
        }
        if self.group.compat.pending {
            if self
                .group
                .compat
                .retry_at
                .is_some_and(|deadline| deadline <= now)
            {
                self.group.compat.disconnected(now);
            } else if let Some(deadline) = self.group.compat.retry_at {
                ctx.request_repaint_after_for(
                    deadline.saturating_duration_since(now),
                    egui::ViewportId::ROOT,
                );
            }
        }
        if self.group.compat.connected
            || self.group.compat.pending
            || !self.prefs.auto_connect
            || self.pair.service.is_none()
        {
            return;
        }
        if let Some(deadline) = self.group.compat.retry_at
            && deadline > now
        {
            ctx.request_repaint_after_for(deadline - now, egui::ViewportId::ROOT);
            return;
        }
        let mut addresses = Vec::new();
        for peer in self
            .discovered
            .values()
            .filter(|peer| peer.public_key.as_deref() == Some(&pinned) && peer.manageable())
        {
            let port = peer.control_port.unwrap_or(workspace::PORT);
            for address in &peer.addresses {
                let mut address = *address;
                address.set_port(port);
                addresses.push(address);
            }
        }
        if let Some(address) = self.group.compat.address {
            addresses.push(address);
        }
        if let Ok(mut address) = self.prefs.workspace.last_address.parse::<SocketAddr>() {
            address.set_port(workspace::PORT);
            addresses.push(address);
        }
        addresses.retain(|address| {
            address.port() != 0 && !address.ip().is_unspecified() && !address.ip().is_multicast()
        });
        addresses.sort();
        addresses.dedup();
        addresses.truncate(8);
        if addresses.is_empty() {
            return;
        }
        match self.pair.service.as_ref().unwrap().dial(addresses, pinned) {
            Ok(()) => {
                self.group.compat.pending = true;
                self.group.compat.retry_at = Some(now + DIAL_TIMEOUT);
                ctx.request_repaint_after_for(DIAL_TIMEOUT, egui::ViewportId::ROOT);
            }
            Err(error) => {
                self.group.compat.disconnected(now);
                self.pair.status = format!(
                    "Legacy management: {}",
                    management::status_text(error.to_string(), 1024)
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn old_version() -> Version {
        Version {
            app: if crate::compatibility::VERSION == "0.4.11" {
                "0.4.10"
            } else {
                "0.4.11"
            }
            .into(),
            protocol: 7,
        }
    }

    #[test]
    fn legacy_metadata_does_not_replace_group_settings_or_member_preferences() {
        let mut state = crate::group::model::tests::fixture(4);
        state.settings.clipboard_enabled = true;
        let key = state.members.keys().nth(1).unwrap().clone();
        let mut node = state.members[&key].node.clone();
        node.version = Some(old_version());
        node.shared.audio_enabled = false;
        node.shared.keyboard_enabled = false;
        let settings = state.settings.clone();
        let preferences = state.members[&key].preferences.clone();
        state.members.get_mut(&key).unwrap().online = false;
        let mut compat = Compat {
            pinned: Some(key.clone()),
            ..Default::default()
        };
        compat.accept_node(&state.hub, node.clone()).unwrap();
        compat.mirror(&mut state);
        assert_eq!(state.settings, settings);
        assert_eq!(state.members[&key].preferences, preferences);
        assert_eq!(state.members[&key].node, node);
        assert!(state.members[&key].online);
        assert!(!state.members[&key].compatible());
        assert!(compat.legacy_target(&key, Some(&state)));
        assert!(!compat.legacy_target(&state.hub, Some(&state)));
        let other = state
            .members
            .values()
            .find(|member| member.node.key != key && member.node.key != state.hub)
            .unwrap()
            .node
            .clone();
        assert!(compat.accept_node(&state.hub, other).is_err());
    }

    #[test]
    fn group_handshake_takes_over_without_using_stale_legacy_metadata() {
        let mut state = crate::group::model::tests::fixture(3);
        let key = state.members.keys().nth(1).unwrap().clone();
        let current = state.members[&key].node.clone();
        let mut old = current.clone();
        old.version = Some(old_version());
        let compat = Compat {
            pinned: Some(key.clone()),
            remote: Some(old),
            connected: true,
            ..Default::default()
        };
        assert!(!compat.legacy_target(&key, Some(&state)));
        compat.mirror(&mut state);
        assert_eq!(state.members[&key].node, current);
        state.members.get_mut(&key).unwrap().online = false;
        let compat = Compat {
            pinned: Some(key.clone()),
            remote: Some(current),
            connected: true,
            ..Default::default()
        };
        compat.mirror(&mut state);
        assert!(
            !state.members[&key].online,
            "Only a group handshake may enable native group routes"
        );
    }

    #[test]
    fn legacy_snapshot_is_bounded_to_old_schema_and_new_only_actions_are_rejected() {
        let state = crate::group::model::tests::fixture(4);
        let own = &state.hub;
        let peer = state.members.keys().find(|key| *key != own).unwrap();
        let mut node = state.members[own].node.clone();
        for key in state.members.keys() {
            node.shared
                .audio_levels
                .insert(key.clone(), Default::default());
            node.shared.devices.insert(key.clone(), Default::default());
        }
        let shared = node.shared.clone();
        let version = node.version.clone();
        let snapshot = outgoing_node(node);
        snapshot.validate(peer, own).unwrap();
        assert_eq!(snapshot.shared, Shared::default());
        assert_ne!(snapshot.shared, shared);
        assert_eq!(snapshot.version, version);
        assert!(legacy_management_message(
            &management::Message::CheckUpdates { id: 1 }
        ));
        assert!(!legacy_management_message(
            &management::Message::ReadMonitor {
                id: 3,
                monitor: "display-1".into()
            }
        ));
        assert!(!legacy_management_message(
            &management::Message::InstallCable {
                id: 2,
                platform: crate::cable::Platform::Windows
            }
        ));
    }
}
