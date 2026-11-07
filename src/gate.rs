use crate::transport::LEASE;
use std::{
    sync::atomic::{AtomicU8, AtomicU64, Ordering::SeqCst},
    time::Instant,
};

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Route {
    Local = 0,
    Hold = 1,
    Remote = 2,
}

pub struct Gate {
    state: AtomicU64,
    deadline: AtomicU64,
    origin: Instant,
    reason: AtomicU8,
}

impl Gate {
    pub fn new() -> Self {
        Self {
            state: AtomicU64::new(0),
            deadline: AtomicU64::new(0),
            origin: Instant::now(),
            reason: AtomicU8::new(0),
        }
    }

    fn now(&self) -> u64 {
        self.origin.elapsed().as_millis() as u64
    }

    pub fn renew(&self, token: u64) {
        if self.current().0 == token {
            self.deadline
                .store(self.now() + LEASE.as_millis() as u64, SeqCst);
        }
    }

    pub fn current(&self) -> (u64, Route) {
        loop {
            let state = self.state.load(SeqCst);
            let token = state >> 2;
            let route = match state & 3 {
                1 => Route::Hold,
                2 => Route::Remote,
                _ => return (token, Route::Local),
            };
            if self.now() < self.deadline.load(SeqCst) {
                return (token, route);
            }
            self.reason.store(1, SeqCst);
            self.local(token);
        }
    }

    pub fn begin(&self) -> Option<u64> {
        let (token, route) = self.current();
        if route != Route::Local {
            return None;
        }
        self.deadline
            .store(self.now() + LEASE.as_millis() as u64, SeqCst);
        if self.set(token, Route::Local, token + 1, Route::Hold) {
            self.reason.store(0, SeqCst);
            Some(token + 1)
        } else {
            None
        }
    }

    fn set(&self, from_token: u64, from: Route, to_token: u64, to: Route) -> bool {
        self.state
            .compare_exchange(
                (from_token << 2) | from as u64,
                (to_token << 2) | to as u64,
                SeqCst,
                SeqCst,
            )
            .is_ok()
    }

    pub fn hold(&self, token: u64) -> bool {
        self.current() == (token, Route::Remote)
            && self.set(token, Route::Remote, token, Route::Hold)
    }

    pub fn forward(&self, token: u64) -> bool {
        self.current() == (token, Route::Hold) && self.set(token, Route::Hold, token, Route::Remote)
    }

    pub fn local(&self, token: u64) {
        while self.state.load(SeqCst) >> 2 == token {
            if self.set(token, Route::Remote, token + 1, Route::Local)
                || self.set(token, Route::Hold, token + 1, Route::Local)
                || self.state.load(SeqCst) == token << 2
            {
                return;
            }
        }
    }

    pub fn emergency(&self) {
        self.reason.store(2, SeqCst);
        self.local(self.state.load(SeqCst) >> 2);
    }
    pub fn input_overflow(&self, token: u64) {
        self.reason.store(3, SeqCst);
        self.local(token);
    }
    pub fn control_overflow(&self, token: u64) {
        self.reason.store(4, SeqCst);
        self.local(token);
    }
    pub fn reason(&self) -> &'static str {
        match self.reason.load(SeqCst) {
            1 => "the input heartbeat lease expired",
            2 => "the emergency key was pressed",
            3 => "the input event queue overflowed",
            4 => "the input control queue overflowed",
            _ => "the input route changed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emergency_revokes_a_late_activation() {
        let gate = Gate::new();
        let token = gate.begin().unwrap();
        gate.emergency();
        gate.renew(token);
        assert!(!gate.forward(token));
        assert!(gate.current().1 == Route::Local);
    }

    #[test]
    fn emergency_restores_local_input_during_return() {
        let gate = Gate::new();
        let token = gate.begin().unwrap();
        assert!(gate.forward(token));
        assert!(gate.hold(token));
        gate.emergency();
        assert!(!gate.forward(token));
        assert!(gate.current().1 == Route::Local);
    }

    #[test]
    fn expired_lease_is_fail_open_without_coordinator() {
        let gate = Gate::new();
        let token = gate.begin().unwrap();
        assert!(gate.forward(token));
        gate.deadline.store(0, SeqCst);
        assert!(gate.current().1 == Route::Local);
        assert!(!gate.forward(token));
    }
}
