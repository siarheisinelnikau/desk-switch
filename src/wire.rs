use crate::video::Inputs;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Mode {
    Input,
    Video,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum Input {
    Key { code: u16, down: bool },
    Button { code: u16, down: bool },
    Move { dx: i32, dy: i32 },
    Scroll { dx: i16, dy: i16 },
}

impl Input {
    #[cfg(target_os = "linux")]
    pub fn valid(self) -> bool {
        match self {
            Self::Key { code, .. } => (1..=248).contains(&code),
            Self::Button { code, .. } => (272..=276).contains(&code),
            Self::Move { dx, dy } => dx.unsigned_abs() <= 16384 && dy.unsigned_abs() <= 16384,
            Self::Scroll { dx, dy } => dx.unsigned_abs() <= 4096 && dy.unsigned_abs() <= 4096,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum Message {
    Ping(u64),
    Pong(u64),
    Prepare {
        id: u64,
        inputs: Option<Inputs>,
        edge: Option<u16>,
    },
    Ready(u64),
    Verify(u64),
    Observed {
        id: u64,
        input: u16,
    },
    Activate(u64),
    Active(u64),
    Event(Input),
    ExitEdge {
        id: u64,
        position: u16,
    },
    Return(u64),
    Returning(u64),
    Returned(u64),
    Rejected(u64),
}
