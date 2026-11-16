//! A command queue whose deadline wait includes time spent suspended.
use std::{
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    sync::{mpsc, Arc},
};

use boot_time::Instant;

pub(crate) struct Sender<T> {
    queue: Option<mpsc::Sender<T>>,
    wake: Arc<UnixStream>,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self {
            queue: self.queue.clone(),
            wake: self.wake.clone(),
        }
    }
}

pub(crate) struct Receiver<T> {
    queue: mpsc::Receiver<T>,
    wake: UnixStream,
    timer: OwnedFd,
}

pub(crate) fn channel<T>() -> io::Result<(Sender<T>, Receiver<T>)> {
    let (write, read) = UnixStream::pair()?;
    write.set_nonblocking(true)?;
    read.set_nonblocking(true)?;
    // SAFETY: No pointers are passed; ownership of the new descriptor is taken below.
    let fd = unsafe {
        libc::timerfd_create(libc::CLOCK_BOOTTIME, libc::TFD_CLOEXEC | libc::TFD_NONBLOCK)
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is a valid, newly created descriptor with no other owner.
    let timer = unsafe { OwnedFd::from_raw_fd(fd) };
    let (tx, rx) = mpsc::channel();
    Ok((
        Sender {
            queue: Some(tx),
            wake: Arc::new(write),
        },
        Receiver {
            queue: rx,
            wake: read,
            timer,
        },
    ))
}

fn notify(mut socket: &UnixStream) -> io::Result<()> {
    loop {
        match socket.write(&[1]) {
            Ok(_) => return Ok(()),
            // A full socket already has a pending notification.
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
}

impl<T> Sender<T> {
    pub(crate) fn send(&self, value: T) -> io::Result<()> {
        self.queue
            .as_ref()
            .unwrap()
            .send(value)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Controller disconnected"))?;
        notify(&self.wake)
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        // Disconnect the queue before notifying, so a receiver cannot miss shutdown.
        self.queue.take();
        let _ = notify(&self.wake);
    }
}

impl<T> Receiver<T> {
    fn arm(&self, deadline: Instant) -> io::Result<()> {
        let mut now = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: now points to a writable timespec for the duration of the call.
        if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut now) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // Sample the raw clock first. If suspend occurs between the samples, we
        // may wake early, but cannot delay an overdue deadline by the sleep time.
        let delay = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_default();
        let nanos = now.tv_nsec as u64 + u64::from(delay.subsec_nanos());
        let seconds = (now.tv_sec as u64)
            .checked_add(delay.as_secs())
            .and_then(|s| s.checked_add(nanos / 1_000_000_000))
            .and_then(|s| libc::time_t::try_from(s).ok())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Timer deadline overflow")
            })?;
        let timer = libc::itimerspec {
            it_interval: libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            },
            it_value: libc::timespec {
                tv_sec: seconds,
                tv_nsec: (nanos % 1_000_000_000) as _,
            },
        };
        // Absolute arming avoids restarting the remaining delay after a suspend
        // between calculating the timeout and entering the kernel.
        // SAFETY: timer is initialized and the descriptor remains owned by self.
        if unsafe {
            libc::timerfd_settime(
                self.timer.as_raw_fd(),
                libc::TFD_TIMER_ABSTIME,
                &timer,
                std::ptr::null_mut(),
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn recv_until(&self, deadline: Instant) -> io::Result<Option<T>> {
        self.recv_until_notified(deadline, None)
    }

    pub(crate) fn recv_until_notified(
        &self,
        deadline: Instant,
        completion: Option<std::os::fd::RawFd>,
    ) -> io::Result<Option<T>> {
        self.arm(deadline)?;
        loop {
            let mut socket = &self.wake;
            let mut bytes = [0; 256];
            loop {
                match socket.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                    Err(err) => return Err(err),
                }
            }
            // Drain notifications before checking the queue: any later send is
            // either visible here or leaves the socket readable for poll.
            match self.queue.try_recv() {
                Ok(value) => return Ok(Some(value)),
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "Controller disconnected",
                    ))
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            let mut fds = [
                libc::pollfd {
                    fd: self.wake.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: self.timer.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: completion.unwrap_or(-1),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            // SAFETY: fds is a writable array, both descriptors stay valid.
            if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, -1) } < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(err);
            }
            if fds[..2]
                .iter()
                .any(|fd| fd.revents & (libc::POLLERR | libc::POLLNVAL) != 0)
            {
                return Err(io::Error::other("Controller wait descriptor failed"));
            }
            // A one-shot timer needs no read here: the boottime deadline is now
            // due. An early expiration from clock sampling is rearmed next time.
            if fds[1].revents & libc::POLLIN != 0 || fds[2].revents != 0 {
                return Ok(None);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn recv_timeout(&self, timeout: std::time::Duration) -> io::Result<T> {
        self.recv_until(Instant::now() + timeout)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "Receive timed out"))
    }

    #[cfg(test)]
    pub(crate) fn try_recv(&self) -> Result<T, mpsc::TryRecvError> {
        self.queue.try_recv()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timer_expires_without_requests_and_can_be_rearmed() {
        let (_sender, receiver) = channel::<u8>().unwrap();
        let deadline = Instant::now() + Duration::from_millis(20);
        assert_eq!(receiver.recv_until(deadline).unwrap(), None);
        assert!(Instant::now() >= deadline);
        let next = Instant::now() + Duration::from_millis(20);
        assert_eq!(receiver.recv_until(next).unwrap(), None);
        assert!(Instant::now() >= next);
    }

    #[test]
    fn queued_and_new_commands_interrupt_long_deadlines() {
        let (sender, receiver) = channel().unwrap();
        let other = sender.clone();
        sender.send(1).unwrap();
        sender.send(2).unwrap();
        assert_eq!(receiver.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
        assert_eq!(receiver.recv_timeout(Duration::from_secs(2)).unwrap(), 2);
        let worker =
            std::thread::spawn(move || receiver.recv_timeout(Duration::from_secs(2)).unwrap());
        other.send(3).unwrap();
        assert_eq!(worker.join().unwrap(), 3);
    }

    #[test]
    fn last_sender_disconnect_wakes_receiver() {
        let (sender, receiver) = channel::<u8>().unwrap();
        let worker = std::thread::spawn(move || {
            receiver.recv_until(Instant::now() + Duration::from_secs(2))
        });
        drop(sender);
        assert_eq!(
            worker.join().unwrap().unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
    }

    #[test]
    fn overdue_deadline_returns_without_waiting() {
        let (_sender, receiver) = channel::<u8>().unwrap();
        assert_eq!(receiver.recv_until(Instant::now()).unwrap(), None);
    }
}
