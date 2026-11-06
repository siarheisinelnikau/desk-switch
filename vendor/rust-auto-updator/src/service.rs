use crate::{Config, Prepared, Release, check_latest};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

pub enum Event {
    Checked(Result<Option<Box<Release>>, String>),
    Progress { received: u64, total: u64 },
    Ready(Result<Prepared, String>),
}
enum Request {
    Check,
    Download(Box<Release>, Arc<AtomicBool>),
    Stop,
}

pub struct Service {
    sender: mpsc::Sender<Request>,
    pub events: mpsc::Receiver<Event>,
    cancel: Mutex<Arc<AtomicBool>>,
}

impl Service {
    pub fn new(config: Config, wake: impl Fn() + Send + 'static) -> Result<Self, String> {
        config.validate()?;
        let (sender, receiver) = mpsc::channel();
        let (out, events) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        thread::Builder::new()
            .name("github-updates".into())
            .spawn(move || {
                let send = |event| {
                    let _ = out.send(event);
                    wake();
                };
                while let Ok(request) = receiver.recv() {
                    match request {
                        Request::Check => send(Event::Checked(
                            check_latest(&config).map(|release| release.map(Box::new)),
                        )),
                        Request::Download(release, cancel) => {
                            send(Event::Ready(crate::install::prepare_cancellable(
                                &release,
                                &config,
                                &cancel,
                                |received, total| send(Event::Progress { received, total }),
                            )))
                        }
                        Request::Stop => break,
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            sender,
            events,
            cancel: Mutex::new(cancel),
        })
    }
    #[cfg(test)]
    pub fn test_channel() -> (Self, impl Fn() -> usize) {
        let (sender, requests) = mpsc::channel();
        let (_, events) = mpsc::channel();
        (
            Self {
                sender,
                events,
                cancel: Mutex::new(Arc::new(AtomicBool::new(false))),
            },
            move || requests.try_iter().count(),
        )
    }

    pub fn check(&self) -> Result<(), String> {
        self.sender
            .send(Request::Check)
            .map_err(|_| "The update worker stopped. Restart the application.".into())
    }
    pub fn download(&self, release: Box<Release>) -> Result<(), String> {
        let cancel = Arc::new(AtomicBool::new(false));
        let mut previous = self
            .cancel
            .lock()
            .map_err(|_| "The update cancellation state is unavailable.")?;
        previous.store(true, Ordering::Release);
        *previous = Arc::clone(&cancel);
        self.sender
            .send(Request::Download(release, cancel))
            .map_err(|_| "The update worker stopped. Restart the application.".into())
    }

    pub fn cancel_download(&self) {
        if let Ok(cancel) = self.cancel.lock() {
            cancel.store(true, Ordering::Release);
        }
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.cancel_download();
        let _ = self.sender.send(Request::Stop);
    }
}
