//! Local routing hints from the authenticated workspace, outside the wire format.
use std::{collections::BTreeSet, net::SocketAddr};

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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Device {
    pub name: String,
    pub connected: bool,
    pub shared: bool,
}
