//! Ownership of the daemon's filesystem socket and its persistent lock file.
use std::{
    fs::{self, File, OpenOptions},
    io, mem,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::{
            ffi::OsStrExt,
            fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
};

use anyhow::{bail, Context};

pub(crate) struct Listener {
    listener: UnixListener,
    path: PathBuf,
    identity: (u64, u64),
    // Never unlink the lock file: another process may already have opened it.
    _lock: File,
}

impl Listener {
    pub(crate) fn accept(&self) -> io::Result<UnixStream> {
        self.listener.accept().map(|(stream, _)| stream)
    }

    pub(crate) fn set_nonblocking(&self) -> io::Result<()> {
        self.listener.set_nonblocking(true)
    }
}

impl AsRawFd for Listener {
    fn as_raw_fd(&self) -> RawFd {
        self.listener.as_raw_fd()
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        // Do not remove a path that was replaced after we bound it.
        if fs::symlink_metadata(&self.path).is_ok_and(|metadata| {
            metadata.file_type().is_socket() && (metadata.dev(), metadata.ino()) == self.identity
        }) {
            if let Err(err) = fs::remove_file(&self.path) {
                log::warn!("Unable to remove socket {}: {err}", self.path.display());
            }
        }
    }
}

fn stale_socket(path: &Path) -> io::Result<bool> {
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: An all-zero sockaddr_un is valid to initialize before setting fields.
    let mut address: libc::sockaddr_un = unsafe { mem::zeroed() };
    if bytes.len() >= address.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Socket path too long",
        ));
    }
    address.sun_family = libc::AF_UNIX as _;
    for (dest, source) in address.sun_path.iter_mut().zip(bytes) {
        *dest = *source as _;
    }
    // SAFETY: No pointers; take ownership of the newly created descriptor below.
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is a valid descriptor exclusively owned here.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: address is initialized, correctly sized, and valid throughout connect.
    let result = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            &address as *const _ as *const libc::sockaddr,
            mem::size_of_val(&address) as _,
        )
    };
    if result == 0 {
        return Ok(false);
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::ECONNREFUSED) => Ok(true),
        // A full listen backlog is still an active listener.
        Some(libc::EAGAIN | libc::EINPROGRESS) => Ok(false),
        _ => Err(err),
    }
}

pub(crate) fn bind(path: &Path) -> anyhow::Result<Listener> {
    let mut lock_path = path.as_os_str().to_os_string();
    lock_path.push(".lock");
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&lock_path)
        .context("Unable to open socket ownership lock")?;
    anyhow::ensure!(
        lock.metadata()?.is_file(),
        "Socket lock must be a regular file"
    );
    // SAFETY: lock remains open for the entire ownership period. No pointers.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
        return Err(io::Error::last_os_error())
            .context("Unable to acquire socket ownership lock; another daemon may be running");
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.file_type().is_socket(),
                "Refusing to replace a non-socket path"
            );
            if !stale_socket(path).context("Unable to check existing socket")? {
                bail!("Socket is already in use");
            }
            let current = fs::symlink_metadata(path)?;
            anyhow::ensure!(
                (current.dev(), current.ino()) == (metadata.dev(), metadata.ino()),
                "Socket changed during startup"
            );
            fs::remove_file(path)?;
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    let listener = UnixListener::bind(path)?;
    let metadata = fs::symlink_metadata(path)?;
    let listener = Listener {
        listener,
        path: path.into(),
        identity: (metadata.dev(), metadata.ino()),
        _lock: lock,
    };
    fs::set_permissions(path, fs::Permissions::from_mode(0o777))?;
    Ok(listener)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::symlink,
        sync::{Arc, Barrier},
        time::{SystemTime, UNIX_EPOCH},
    };

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("backlightd-socket-{}-{unique}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn socket(&self) -> PathBuf {
            self.0.join("daemon.sock")
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn refuses_an_existing_listener_with_or_without_our_lock() {
        let directory = Directory::new();
        let path = directory.socket();
        let original = UnixListener::bind(&path).unwrap();
        let inode = fs::symlink_metadata(&path).unwrap().ino();
        assert!(bind(&path).is_err());
        assert_eq!(fs::symlink_metadata(&path).unwrap().ino(), inode);
        // An actual connection still reaches the original listener.
        let _client = std::os::unix::net::UnixStream::connect(&path).unwrap();
        original.accept().unwrap();
        drop(original);
        let owner = bind(&path).unwrap();
        assert!(bind(&path).is_err());
        drop(owner);
        assert!(!path.exists());
        assert!(path.with_extension("sock.lock").exists());
        assert!(bind(&path).is_ok());
    }

    #[test]
    fn a_full_listener_backlog_is_not_treated_as_stale() {
        let directory = Directory::new();
        let path = directory.socket();
        let listener = UnixListener::bind(&path).unwrap();
        // SAFETY: listener owns a valid listening socket; reducing its backlog
        // lets this test exercise nonblocking probes against a full queue.
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 1) }, 0);
        for _ in 0..8 {
            assert!(!stale_socket(&path).unwrap());
        }
        assert!(bind(&path).is_err());
        assert!(fs::symlink_metadata(&path).unwrap().file_type().is_socket());
    }

    #[test]
    fn refuses_regular_files_directories_and_symlinks() {
        let directory = Directory::new();
        let target = directory.0.join("target");
        fs::write(&target, "keep me").unwrap();
        for kind in ["file", "directory", "symlink", "dangling"] {
            let path = directory.0.join(kind);
            match kind {
                "file" => fs::write(&path, "keep me").unwrap(),
                "directory" => fs::create_dir(&path).unwrap(),
                "symlink" => symlink(&target, &path).unwrap(),
                _ => symlink(directory.0.join("missing"), &path).unwrap(),
            }
            assert!(bind(&path).is_err());
            assert!(fs::symlink_metadata(&path).is_ok());
        }
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep me");
        assert_eq!(
            fs::read_to_string(directory.0.join("file")).unwrap(),
            "keep me"
        );
    }

    #[test]
    fn refuses_a_symlink_lock_file() {
        let directory = Directory::new();
        let path = directory.socket();
        let target = directory.0.join("target");
        fs::write(&target, "keep me").unwrap();
        symlink(&target, path.with_extension("sock.lock")).unwrap();
        assert!(bind(&path).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "keep me");
        assert!(!path.exists());
    }

    #[test]
    fn cleanup_does_not_remove_a_replacement_path() {
        let directory = Directory::new();
        let path = directory.socket();
        let owner = bind(&path).unwrap();
        fs::remove_file(&path).unwrap();
        fs::write(&path, "replacement").unwrap();
        drop(owner);
        assert_eq!(fs::read_to_string(&path).unwrap(), "replacement");
    }

    #[test]
    fn simultaneous_starts_cannot_both_own_a_stale_socket() {
        let directory = Directory::new();
        let path = directory.socket();
        drop(UnixListener::bind(&path).unwrap());
        let barrier = Arc::new(Barrier::new(2));
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let result = bind(&path);
                    let success = result.is_ok();
                    barrier.wait();
                    drop(result);
                    success
                })
            })
            .collect();
        let successes = workers
            .into_iter()
            .map(|worker| usize::from(worker.join().unwrap()))
            .sum::<usize>();
        assert_eq!(successes, 1);
    }
}
