//! Background EDID enumeration; DDC/CI probes and all writes remain controller-owned.
use std::{
    io,
    os::{fd::AsRawFd, unix::net::UnixStream},
    thread::{self, JoinHandle},
};

pub(crate) struct Enumeration<T> {
    completion: UnixStream,
    worker: Option<JoinHandle<T>>,
}

impl<T: Send + 'static> Enumeration<T> {
    pub(crate) fn spawn(enumerate: impl FnOnce() -> T + Send + 'static) -> io::Result<Self> {
        let (completion, finished) = UnixStream::pair()?;
        let worker = thread::Builder::new()
            .name("backlight-discovery".into())
            .spawn(move || {
                // Closing this endpoint wakes poll on both success and panic.
                let _finished = finished;
                enumerate()
            })?;
        Ok(Self {
            completion,
            worker: Some(worker),
        })
    }

    pub(crate) fn fd(&self) -> std::os::fd::RawFd {
        self.completion.as_raw_fd()
    }

    pub(crate) fn ready(&self) -> bool {
        let mut fd = libc::pollfd {
            fd: self.fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: fd references our live socket; poll does not retain the pointer.
        unsafe { libc::poll(&mut fd, 1, 0) > 0 }
    }

    pub(crate) fn finish(mut self) -> anyhow::Result<T> {
        self.worker
            .take()
            .unwrap()
            .join()
            .map_err(|_| anyhow::anyhow!("Discovery worker panicked"))
    }
}

impl<T> Drop for Enumeration<T> {
    fn drop(&mut self) {
        // Never leave a hardware enumeration thread running after controller shutdown.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn worker_panic_notifies_waiter_and_is_reported() {
        let worker = Enumeration::spawn(|| panic!("fake enumeration failure")).unwrap();
        let (_sender, receiver) = crate::wake::channel::<()>().unwrap();
        let deadline = boot_time::Instant::now() + Duration::from_secs(2);
        assert!(receiver
            .recv_until_notified(deadline, Some(worker.fd()))
            .unwrap()
            .is_none());
        assert!(worker
            .finish()
            .unwrap_err()
            .to_string()
            .contains("panicked"));
    }

    #[test]
    fn dropping_enumeration_joins_the_worker() {
        let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = finished.clone();
        let worker = Enumeration::spawn(move || {
            flag.store(true, std::sync::atomic::Ordering::Release);
        })
        .unwrap();
        drop(worker);
        assert!(finished.load(std::sync::atomic::Ordering::Acquire));
    }
}
