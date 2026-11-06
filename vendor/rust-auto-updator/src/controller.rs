use crate::{Config, Event, Prepared, Release, Service};
use std::time::{Duration, Instant};

/// Presentation-neutral workflow state, usable from egui, native UI, or a CLI.
pub enum State {
    Idle,
    Checking,
    Current,
    Available(Box<Release>),
    Downloading { received: u64, total: u64 },
    Installing,
    Error(String),
}

impl State {
    pub fn busy(&self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Downloading { .. } | Self::Installing
        )
    }
}

#[derive(Default)]
pub struct Poll {
    /// The host can persist its own last-check timestamp after this signal.
    pub checked: bool,
    /// Save user data and finish pending work, then take the prepared update,
    /// hand it off, and exit. The library never terminates the host process.
    pub ready_to_install: bool,
}

pub struct Controller {
    service: Option<Service>,
    state: State,
    prepared: Option<Prepared>,
    version: String,
    disabled_reason: Option<String>,
    started: Instant,
    startup_delay: Duration,
    startup_considered: bool,
}

impl Controller {
    pub fn new(config: Config, wake: impl Fn() + Send + 'static) -> Result<Self, String> {
        let version = config.current_version.to_string();
        let mut controller = Self::disabled(&version, "");
        controller.service = Some(Service::new(config, wake)?);
        controller.disabled_reason = None;
        Ok(controller)
    }

    /// Preview mode creates no worker and performs no network requests.
    pub fn disabled(version: &str, reason: &str) -> Self {
        Self {
            service: None,
            state: State::Idle,
            prepared: None,
            version: version.into(),
            disabled_reason: Some(reason.into()),
            started: Instant::now(),
            startup_delay: Duration::from_secs(5),
            startup_considered: false,
        }
    }

    pub fn state(&self) -> &State {
        &self.state
    }
    pub fn installed_version(&self) -> &str {
        &self.version
    }
    pub fn disabled_reason(&self) -> Option<&str> {
        self.disabled_reason.as_deref()
    }
    pub fn enabled(&self) -> bool {
        self.service.is_some()
    }
    pub fn update_available(&self) -> bool {
        matches!(self.state, State::Available(_))
    }

    pub fn check(&mut self) {
        if !self.state.busy()
            && let Some(service) = &self.service
        {
            self.startup_considered = true;
            self.state = match service.check() {
                Ok(()) => State::Checking,
                Err(error) => State::Error(error),
            };
        }
    }

    /// Only call in response to explicit user intent; polling never downloads.
    pub fn download_available(&mut self) {
        if let State::Available(release) = &self.state
            && let Some(service) = &self.service
        {
            self.state = match service.download(release.clone()) {
                Ok(()) => State::Downloading {
                    received: 0,
                    total: 0,
                },
                Err(error) => State::Error(error),
            };
        }
    }

    /// Call from the host's existing logic tick or worker wake callback.
    /// The automatic-check preference belongs to the host's settings store.
    /// This method has no timers, repaint loop, or blocking network calls.
    pub fn poll(&mut self, check_on_startup: bool) -> Poll {
        let mut result = Poll::default();
        if let Some(service) = &self.service {
            for event in service.events.try_iter() {
                match event {
                    Event::Checked(value) => {
                        result.checked = true;
                        self.state = match value {
                            Ok(Some(release)) => State::Available(release),
                            Ok(None) => State::Current,
                            Err(error) => State::Error(error),
                        };
                    }
                    Event::Progress { received, total } => {
                        self.state = State::Downloading { received, total };
                    }
                    Event::Ready(value) => match value {
                        Ok(prepared) => {
                            self.prepared = Some(prepared);
                            self.state = State::Installing;
                            result.ready_to_install = true;
                        }
                        Err(error) => self.state = State::Error(error),
                    },
                }
            }
        }
        if !self.startup_considered
            && check_on_startup
            && self.started.elapsed() >= self.startup_delay
            && matches!(self.state, State::Idle)
        {
            self.check();
        }
        result
    }

    pub fn take_prepared(&mut self) -> Option<Prepared> {
        self.prepared.take()
    }

    pub fn installation_failed(&mut self, error: String) {
        self.prepared = None;
        self.state = State::Error(error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_opt_out_manual_checks_and_duplicate_suppression() {
        let (service, requests) = Service::test_channel();
        let mut controller = Controller::disabled("1.2.3", "test");
        controller.service = Some(service);
        controller.disabled_reason = None;
        controller.started = Instant::now() - Duration::from_secs(6);
        controller.poll(false);
        assert_eq!(requests(), 0, "opt-out must make no startup request");
        controller.check();
        assert_eq!(requests(), 1, "manual checks remain available");
        controller.check();
        controller.poll(true);
        assert_eq!(requests(), 0, "pending checks are not duplicated");
    }

    #[test]
    fn startup_check_is_once_per_launch_and_never_downloads() {
        let (service, requests) = Service::test_channel();
        let mut controller = Controller::disabled("1.2.3", "test");
        controller.service = Some(service);
        controller.started = Instant::now() - Duration::from_secs(6);
        controller.poll(true);
        assert_eq!(requests(), 1);
        controller.state = State::Current;
        controller.poll(true);
        assert_eq!(requests(), 0);
    }

    #[test]
    fn preview_creates_no_worker_and_cannot_start_requests() {
        let mut controller = Controller::disabled("1.2.3", "Preview mode");
        controller.check();
        controller.download_available();
        controller.poll(true);
        assert!(!controller.enabled());
        assert!(matches!(controller.state(), State::Idle));
    }
}
