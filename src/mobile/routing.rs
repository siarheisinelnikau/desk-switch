//! Local routing hints from the authenticated workspace, outside the wire format.
use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Destination {
    #[default]
    Local,
    Remote {
        key: String,
        name: String,
        address: SocketAddr,
        control_port: u16,
        input_port: u16,
    },
    Paused {
        reason: String,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Routing {
    pub generation: u64,
    pub destination: Destination,
    pub members: BTreeSet<String>,
    pub workspace: Option<WorkspaceAccess>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceAccess {
    pub id: String,
    pub peers: BTreeSet<String>,
    pub grants: BTreeMap<String, Option<String>>,
    pub can_enroll: bool,
}

impl WorkspaceAccess {
    pub fn grants(&self, key: &str) -> bool {
        self.grants.get(key).is_some_and(Option::is_some)
    }

    pub fn may_migrate(&self, key: &str) -> bool {
        !self.grants.contains_key(key)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Device {
    pub name: String,
    pub connected: bool,
    pub shared: bool,
}
