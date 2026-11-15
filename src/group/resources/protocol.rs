use super::{
    storage::Version,
    sync::{Manifest, Node, Operation},
    usb::Device,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FolderInfo {
    pub id: String,
    pub name: String,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Catalog {
    pub folders: Vec<FolderInfo>,
    pub usb: Vec<Device>,
    pub owners: BTreeMap<String, String>,
}
#[derive(Debug, Deserialize, Serialize)]
pub enum Request {
    Catalog,
    Folder {
        id: String,
        job: String,
    },
    Manifest,
    Begin {
        index: u32,
        operation: Operation,
    },
    Chunk {
        offset: u64,
        bytes: Vec<u8>,
    },
    Finish,
    Verify {
        path: String,
        value: Node,
    },
    Read {
        path: String,
        value: Node,
        offset: u64,
    },
    Prefix {
        path: String,
        offset: u64,
    },
    Close,
    UsbAttach {
        device: Device,
    },
}
#[derive(Debug, Deserialize, Serialize)]
pub enum Response {
    Catalog(Catalog),
    Manifest(Manifest),
    Ready {
        done: bool,
        offset: u64,
        prefix: String,
    },
    Ack(u64),
    Bytes(Vec<u8>),
    Prefix(String),
    Ok,
    Error(String),
}
// Keep versions local: their storage IDs are never exposed as remote file reads.
#[derive(Clone, Debug)]
pub struct Recovery {
    pub folder: String,
    pub version: Version,
}
