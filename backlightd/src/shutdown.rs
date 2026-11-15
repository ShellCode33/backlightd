//! Receive termination signals synchronously, without async signal handlers.
use std::{
    io, mem,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
};

pub(crate) struct Signals {
    fd: OwnedFd,
    previous: libc::sigset_t,
    _same_thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl Signals {
    // Must run before spawning workers so they inherit the blocked signal mask.
    pub(crate) fn new() -> io::Result<Self> {
        // SAFETY: sigsets are initialized before use, all pointers remain valid.
        unsafe {
            let mut signals = mem::zeroed();
            let mut previous = mem::zeroed();
            libc::sigemptyset(&mut signals);
            libc::sigaddset(&mut signals, libc::SIGTERM);
            libc::sigaddset(&mut signals, libc::SIGINT);
            let error = libc::pthread_sigmask(libc::SIG_BLOCK, &signals, &mut previous);
            if error != 0 {
                return Err(io::Error::from_raw_os_error(error));
            }
            let fd = libc::signalfd(-1, &signals, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK);
            if fd < 0 {
                let error = io::Error::last_os_error();
                libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut());
                return Err(error);
            }
            Ok(Self {
                fd: OwnedFd::from_raw_fd(fd),
                previous,
                _same_thread: std::marker::PhantomData,
            })
        }
    }

    pub(crate) fn receive(&self) -> io::Result<()> {
        // SAFETY: info is writable, correctly sized, and not inspected unless initialized.
        let mut info: libc::signalfd_siginfo = unsafe { mem::zeroed() };
        let result = unsafe {
            libc::read(
                self.fd.as_raw_fd(),
                &mut info as *mut _ as *mut _,
                mem::size_of_val(&info),
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        if result as usize != mem::size_of_val(&info) {
            return Err(io::Error::other("Incomplete signal notification"));
        }
        log::info!("Received signal {}; shutting down", info.ssi_signo);
        Ok(())
    }
}

impl AsRawFd for Signals {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl Drop for Signals {
    fn drop(&mut self) {
        // SAFETY: Restore on the same thread that installed this valid signal mask,
        // after workers have joined. No shared state is accessed by signal handlers.
        unsafe {
            libc::pthread_sigmask(libc::SIG_SETMASK, &self.previous, std::ptr::null_mut());
        }
    }
}
