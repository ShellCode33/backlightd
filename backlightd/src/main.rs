use std::{
    env,
    io::{self, BufReader},
    net::Shutdown,
    os::{fd::AsRawFd, unix::net::UnixStream},
    process::exit,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::channel,
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::anyhow;
use backlight_ipc::{BacklightCommand, DEFAULT_UNIX_SOCKET_PATH};
use controller::{run_controller, Request};
use wake::Sender;

mod auto;
mod calibration;
mod config;
mod controller;
mod ddc;
mod location;
mod monitors;
mod shutdown;
mod socket;
mod sysfs;
mod wake;

fn main() {
    let result = (|| -> anyhow::Result<()> {
        let mut socket_path = std::path::PathBuf::from(DEFAULT_UNIX_SOCKET_PATH);
        let mut config_path = std::path::PathBuf::from(config::DEFAULT_PATH);
        let mut required = false;
        let mut check = false;
        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--config" => {
                    config_path = args
                        .next()
                        .ok_or_else(|| anyhow!("--config requires a path"))?
                        .into();
                    required = true;
                }
                "--unix-socket-path" => {
                    socket_path = args
                        .next()
                        .ok_or_else(|| anyhow!("--unix-socket-path requires a path"))?
                        .into()
                }
                "--check-config" => check = true,
                "--help" | "-h" => {
                    println!("Usage: backlightd [--config PATH] [--check-config] [--unix-socket-path PATH]");
                    return Ok(());
                }
                _ => return Err(anyhow!("Unknown argument {arg:?}")),
            }
        }
        let settings = config::load(&config_path, required || check)?;
        if check {
            println!("Configuration valid: {}", config_path.display());
            return Ok(());
        }
        pretty_env_logger::formatted_builder()
            .filter_level(log::LevelFilter::Trace)
            .parse_env("BACKLIGHTD_LOG_LEVEL")
            .filter_module("ureq", log::LevelFilter::Warn)
            .init();
        config::publish(settings);
        run_server(&socket_path, &config_path, run_controller)
    })();
    if let Err(err) = result {
        eprintln!("backlightd: {err:#}");
        exit(1);
    }
}

fn run_server(
    path: &std::path::Path,
    config_path: &std::path::Path,
    controller: impl FnOnce(wake::Receiver<Request>, Arc<AtomicBool>) -> anyhow::Result<()>
        + Send
        + 'static,
) -> anyhow::Result<()> {
    let signals = shutdown::Signals::new()?;
    let listener = socket::bind(path)?;
    listener.set_nonblocking()?;
    let (sender, receiver) = wake::channel()?;
    let stopping = Arc::new(AtomicBool::new(false));
    let controller_stopping = stopping.clone();
    let (done, finished) = UnixStream::pair()?;
    let controller = thread::Builder::new()
        .name("backlight-controller".into())
        .spawn(move || {
            // Closing this endpoint wakes the main thread on normal exit or panic.
            let _done = done;
            controller(receiver, controller_stopping)
        })?;
    let mut workers = Vec::new();
    let result = serve(
        &listener,
        &signals,
        &finished,
        &sender,
        &mut workers,
        config_path,
    );
    stopping.store(true, Ordering::Release);
    // Wake a controller blocked on its deadline. This command is internal only;
    // the client handler still interprets NotifyShutdown as closing a connection.
    let (reply, _) = channel();
    let _ = sender.send(Request {
        command: BacklightCommand::NotifyShutdown,
        reload: None,
        received_at: boot_time::Instant::now(),
        reply,
    });
    drop(sender);
    for worker in &workers {
        let _ = worker.socket.shutdown(Shutdown::Both);
    }
    let controller_result = controller
        .join()
        .map_err(|_| anyhow!("Controller thread panicked"))
        .and_then(|result| result);
    for worker in workers {
        if worker.join().is_err() {
            log::error!("Client worker panicked during shutdown");
        }
    }
    // listener and its ownership lock stay alive until all device work has stopped.
    controller_result.and(result)
}

fn serve(
    listener: &socket::Listener,
    signals: &shutdown::Signals,
    finished: &UnixStream,
    sender: &Sender<Request>,
    workers: &mut Vec<ClientWorker>,
    config_path: &std::path::Path,
) -> anyhow::Result<()> {
    loop {
        let mut fds = [
            listener.as_raw_fd(),
            signals.as_raw_fd(),
            finished.as_raw_fd(),
        ]
        .map(|fd| libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        });
        // SAFETY: fds is writable and all descriptors stay owned for this call.
        if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, -1) } < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err.into());
        }
        if fds[1].revents & libc::POLLIN != 0 {
            let signal = signals.receive()?;
            if signal == libc::SIGHUP as u32 {
                match config::load(config_path, true) {
                    Ok(config) => {
                        let (reply, _) = channel();
                        sender.send(Request {
                            command: BacklightCommand::GetInfo,
                            reload: Some(config),
                            received_at: boot_time::Instant::now(),
                            reply,
                        })?;
                    }
                    Err(err) => log::error!(
                        "Configuration reload rejected; retaining current settings: {err:#}"
                    ),
                }
                continue;
            }
            log::info!("Received signal {signal}; shutting down");
            return Ok(());
        }
        if fds[2].revents != 0 {
            return Err(anyhow!("Controller stopped unexpectedly"));
        }
        if fds
            .iter()
            .any(|fd| fd.revents & (libc::POLLERR | libc::POLLNVAL) != 0)
            || fds[0].revents & libc::POLLHUP != 0
        {
            return Err(anyhow!("Daemon wait descriptor failed"));
        }
        if fds[0].revents & libc::POLLIN != 0 {
            match listener.accept() {
                Ok(client) => {
                    if let Err(err) = spawn_client(client, sender.clone(), workers) {
                        log::error!("Unable to serve client: {err}");
                    }
                }
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                    ) => {}
                Err(err) => return Err(err.into()),
            }
        }
    }
}

const MAX_CLIENTS: usize = 16;
const CLIENT_IO_TIMEOUT: Duration = Duration::from_secs(5);

struct ClientWorker {
    thread: JoinHandle<()>,
    socket: Arc<UnixStream>,
}

struct DisconnectOnDrop(Arc<UnixStream>);
impl Drop for DisconnectOnDrop {
    fn drop(&mut self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}

impl ClientWorker {
    fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }
    fn join(self) -> thread::Result<()> {
        self.thread.join()
    }
}

fn spawn_client(
    client: UnixStream,
    sender: Sender<Request>,
    workers: &mut Vec<ClientWorker>,
) -> io::Result<()> {
    workers.retain(|worker| !worker.is_finished());
    if workers.len() >= MAX_CLIENTS {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Too many client connections",
        ));
    }
    let socket = Arc::new(client.try_clone()?);
    let completion = DisconnectOnDrop(socket.clone());
    let thread = thread::Builder::new()
        .name("backlight-client".into())
        .spawn(move || {
            let _completion = completion;
            if let Err(err) = handle_client(client, &sender, CLIENT_IO_TIMEOUT) {
                log::error!("{err:#}");
            }
        })?;
    workers.push(ClientWorker { thread, socket });
    Ok(())
}

fn handle_client(
    client: UnixStream,
    sender: &Sender<Request>,
    timeout: Duration,
) -> anyhow::Result<()> {
    client.set_read_timeout(Some(timeout))?;
    client.set_write_timeout(Some(timeout))?;
    let mut reader = BufReader::new(&client);
    loop {
        let command = BacklightCommand::deserialize_from(&mut reader)
            .map_err(|err| anyhow!("Unable to deserialize command: {err}"))?;
        if matches!(command, BacklightCommand::NotifyShutdown) {
            return Ok(());
        }
        let get_info = matches!(
            command,
            BacklightCommand::GetInfo | BacklightCommand::GetDeviceInfo(_)
        );
        let hotplug = matches!(command, BacklightCommand::NotifyHotplug);
        let (reply, response) = channel();
        sender
            .send(Request {
                command,
                reload: None,
                received_at: boot_time::Instant::now(),
                reply,
            })
            .map_err(|_| anyhow!("Brightness controller disconnected"))?;
        // udev must be free to deliver the rest of an event burst while scanning.
        if hotplug {
            return Ok(());
        }
        let result = response
            .recv()
            .map_err(|_| anyhow!("Brightness controller dropped its reply"))?;
        match result {
            Ok(info) if get_info => BacklightCommand::GetInfoResponse(info)
                .serialize_into(&client)
                .map_err(|err| anyhow!("Unable to serialize response: {err}"))?,
            Ok(_) => {}
            Err(err) => {
                BacklightCommand::ErrorResponse(err.to_string())
                    .serialize_into(&client)
                    .map_err(|err| anyhow!("Unable to serialize error: {err}"))?;
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use backlight_ipc::{BacklightInfo, BacklightMode};

    // Run the server with fake devices in a separate process: signal masks and
    // process-directed signals must never affect the cargo test runner.
    #[test]
    fn shutdown_subprocess() {
        let Some(path) = env::var_os("BACKLIGHTD_SHUTDOWN_TEST") else {
            return;
        };
        let path = std::path::PathBuf::from(path);
        let gate_path = path.with_extension("gate");
        let active_path = path.with_extension("active");
        let complete_path = path.with_extension("complete");
        let mode = env::var("BACKLIGHTD_SHUTDOWN_MODE").unwrap();
        let result = run_server(
            &path,
            &path.with_extension("toml"),
            move |receiver, stopping| {
                if mode == "error" {
                    return Err(anyhow!("fake controller failure"));
                }
                if mode == "panic" {
                    panic!("fake controller panic");
                }
                let gate = std::os::unix::net::UnixListener::bind(gate_path)?;
                loop {
                    let request = receiver.recv_timeout(Duration::from_secs(10))?;
                    if stopping.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    if let Some(settings) = request.reload {
                        let duration = settings.manual.override_duration.clone();
                        config::publish(settings);
                        std::fs::write(&active_path, duration)?;
                        continue;
                    }
                    if mode == "active" {
                        use std::io::Read;
                        std::fs::write(&active_path, "active")?;
                        let (mut release, _) = gate.accept()?;
                        release.read_exact(&mut [0])?;
                        std::fs::write(&complete_path, "complete")?;
                    }
                    let _ = request.reply.send(Ok(BacklightInfo {
                        brightness_percent: if mode == "reload"
                            && config::current().manual.override_duration == "forever"
                        {
                            77
                        } else {
                            50
                        },
                        mode: BacklightMode::Manual,
                        device: None,
                    }));
                }
            },
        );
        if env::var("BACKLIGHTD_SHUTDOWN_MODE").unwrap() == "error" {
            assert!(format!("{:#}", result.unwrap_err()).contains("fake controller failure"));
        } else if env::var("BACKLIGHTD_SHUTDOWN_MODE").unwrap() == "panic" {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("Controller thread panicked"));
        } else {
            result.unwrap();
        }
    }

    struct ShutdownChild {
        child: std::process::Child,
        directory: std::path::PathBuf,
    }
    impl Drop for ShutdownChild {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
    fn wait_for(mut condition: impl FnMut() -> bool) {
        let limit = std::time::Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(
                std::time::Instant::now() < limit,
                "subprocess did not reach expected state"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn shutdown_child(mode: &str) -> (ShutdownChild, std::path::PathBuf) {
        use std::os::unix::process::CommandExt;
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = env::temp_dir().join(format!(
            "backlightd-shutdown-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        let socket = directory.join("daemon.sock");
        let mut command = std::process::Command::new(env::current_exe().unwrap());
        command
            .args(["--exact", "tests::shutdown_subprocess", "--test-threads=1"])
            .env("BACKLIGHTD_SHUTDOWN_TEST", &socket)
            .env("BACKLIGHTD_SHUTDOWN_MODE", mode)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // SAFETY: Only async-signal-safe libc operations run before exec. Blocking
        // here ensures even the test harness main thread inherits this signal mask.
        unsafe {
            command.pre_exec(|| {
                let mut set = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGTERM);
                libc::sigaddset(&mut set, libc::SIGINT);
                libc::sigaddset(&mut set, libc::SIGHUP);
                if libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().unwrap();
        (ShutdownChild { child, directory }, socket)
    }

    #[test]
    fn sighup_applies_valid_configuration_and_rejects_invalid_reload() {
        let (mut server, path) = shutdown_child("reload");
        wait_for(|| path.exists());
        let config_path = path.with_extension("toml");
        std::fs::write(&config_path, "[manual]\noverride_duration = 'forever'").unwrap();
        // SAFETY: Both signals target only this isolated test process.
        assert_eq!(
            unsafe { libc::kill(server.child.id() as _, libc::SIGHUP) },
            0
        );
        wait_for(|| {
            std::fs::read_to_string(path.with_extension("active"))
                .ok()
                .as_deref()
                == Some("forever")
        });
        for invalid in [false, true] {
            if invalid {
                std::fs::write(&config_path, "[logging]\nlevel = 'invalid'").unwrap();
                // SAFETY: The child is still owned by this test.
                assert_eq!(
                    unsafe { libc::kill(server.child.id() as _, libc::SIGHUP) },
                    0
                );
            }
            let client = UnixStream::connect(&path).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            BacklightCommand::GetInfo.serialize_into(&client).unwrap();
            let response = BacklightCommand::deserialize_from(BufReader::new(&client)).unwrap();
            assert!(
                matches!(response, BacklightCommand::GetInfoResponse(info) if info.brightness_percent == 77)
            );
            BacklightCommand::NotifyShutdown
                .serialize_into(&client)
                .unwrap();
        }
        // SAFETY: Shut down only the child process spawned above.
        assert_eq!(
            unsafe { libc::kill(server.child.id() as _, libc::SIGTERM) },
            0
        );
        let mut status = None;
        wait_for(|| {
            status = server.child.try_wait().unwrap();
            status.is_some()
        });
        assert!(status.unwrap().success());
        assert!(!path.exists());
    }

    #[test]
    fn termination_signals_clean_up_idle_and_active_servers() {
        for signal in [libc::SIGTERM, libc::SIGINT] {
            for mode in ["idle", "active"] {
                let (mut server, path) = shutdown_child(mode);
                wait_for(|| path.exists());
                let mut client = UnixStream::connect(&path).unwrap();
                if mode == "active" {
                    BacklightCommand::SetBrightness(50)
                        .serialize_into(&client)
                        .unwrap();
                    wait_for(|| path.with_extension("active").exists());
                }
                // SAFETY: Signal only our still-owned child process.
                assert_eq!(unsafe { libc::kill(server.child.id() as _, signal) }, 0);
                if mode == "active" {
                    use std::io::{Read, Write};
                    client
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    assert_eq!(client.read(&mut [0]).unwrap(), 0);
                    assert!(server.child.try_wait().unwrap().is_none());
                    assert!(
                        path.exists(),
                        "ownership released before device work finished"
                    );
                    UnixStream::connect(path.with_extension("gate"))
                        .unwrap()
                        .write_all(&[1])
                        .unwrap();
                }
                let mut status = None;
                wait_for(|| {
                    status = server.child.try_wait().unwrap();
                    status.is_some()
                });
                assert!(status.unwrap().success());
                assert!(!path.exists());
                assert!(
                    socket::bind(&path).is_ok(),
                    "socket ownership was not released"
                );
                if mode == "active" {
                    assert!(path.with_extension("complete").exists());
                }
            }
        }
    }

    #[test]
    fn controller_failure_cleans_up_and_preserves_its_cause() {
        for mode in ["error", "panic"] {
            let (mut server, path) = shutdown_child(mode);
            let mut status = None;
            wait_for(|| {
                status = server.child.try_wait().unwrap();
                status.is_some()
            });
            assert!(status.unwrap().success());
            assert!(!path.exists());
            assert!(socket::bind(&path).is_ok());
        }
    }

    #[test]
    fn hotplug_notification_does_not_wait_for_controller_or_client_shutdown() {
        let (client, server) = UnixStream::pair().unwrap();
        let (sender, receiver) = wake::channel().unwrap();
        let worker = thread::spawn(move || {
            handle_client(server, &sender, Duration::from_millis(20)).unwrap();
        });
        BacklightCommand::NotifyHotplug
            .serialize_into(&client)
            .unwrap();
        let request = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(request.command, BacklightCommand::NotifyHotplug));
        // Keep both the reply and client open: the worker must finish independently.
        worker.join().unwrap();
        assert!(request.reply.send(Err(anyhow!("unused reply"))).is_err());
    }

    #[test]
    fn completed_worker_closes_connection_while_its_handle_is_retained() {
        use std::io::Read;
        let (mut client, server) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let (sender, _receiver) = wake::channel().unwrap();
        let mut workers = Vec::new();
        spawn_client(server, sender, &mut workers).unwrap();
        BacklightCommand::NotifyShutdown
            .serialize_into(&client)
            .unwrap();
        assert_eq!(client.read(&mut [0]).unwrap(), 0);
        workers.pop().unwrap().join().unwrap();
    }

    #[test]
    fn idle_client_does_not_block_another_client() {
        let (idle, idle_server) = UnixStream::pair().unwrap();
        let (client, server) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let (sender, receiver) = wake::channel::<Request>().unwrap();
        let mut workers = Vec::new();
        spawn_client(idle_server, sender.clone(), &mut workers).unwrap();
        spawn_client(server, sender, &mut workers).unwrap();
        BacklightCommand::GetInfo.serialize_into(&client).unwrap();
        let request = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(request.command, BacklightCommand::GetInfo));
        request
            .reply
            .send(Ok(BacklightInfo {
                brightness_percent: 50,
                mode: BacklightMode::Auto,
                device: None,
            }))
            .unwrap();
        assert!(matches!(
            BacklightCommand::deserialize_from(BufReader::new(&client)).unwrap(),
            BacklightCommand::GetInfoResponse(_)
        ));
        BacklightCommand::NotifyShutdown
            .serialize_into(&client)
            .unwrap();
        drop(idle);
        for worker in workers {
            worker.join().unwrap();
        }
    }

    #[test]
    fn idle_and_partial_messages_time_out() {
        for partial in [b"".as_slice(), b"{\"type\":"] {
            let (mut client, server) = UnixStream::pair().unwrap();
            use std::io::Write;
            client.write_all(partial).unwrap();
            let (sender, _receiver) = wake::channel().unwrap();
            // Keep the client connected while checking that the read expires.
            let err = handle_client(server, &sender, Duration::from_millis(20)).unwrap_err();
            assert!(err
                .to_string()
                .starts_with("Unable to deserialize command:"));
        }
    }

    #[test]
    fn connection_limit_rejects_excess_clients_and_releases_slots() {
        let (sender, _receiver) = wake::channel().unwrap();
        let mut workers = Vec::new();
        let mut clients = Vec::new();
        for _ in 0..MAX_CLIENTS {
            let (client, server) = UnixStream::pair().unwrap();
            spawn_client(server, sender.clone(), &mut workers).unwrap();
            clients.push(client);
        }
        let (_excess, server) = UnixStream::pair().unwrap();
        assert_eq!(
            spawn_client(server, sender.clone(), &mut workers)
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        drop(clients.pop().unwrap());
        workers.pop().unwrap().join().unwrap();
        let (client, server) = UnixStream::pair().unwrap();
        spawn_client(server, sender, &mut workers).unwrap();
        drop(client);
        drop(clients);
        for worker in workers {
            worker.join().unwrap();
        }
    }

    #[test]
    fn client_receives_controller_error_without_waiting_for_status() {
        let (client, server) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let (sender, receiver) = wake::channel::<Request>().unwrap();
        let worker = thread::spawn(move || handle_client(server, &sender, CLIENT_IO_TIMEOUT));
        BacklightCommand::SetMode(BacklightMode::Auto)
            .serialize_into(&client)
            .unwrap();
        let request = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        request
            .reply
            .send(Err(anyhow!("device unavailable")))
            .unwrap();
        assert!(
            matches!(BacklightCommand::deserialize_from(BufReader::new(&client)).unwrap(), BacklightCommand::ErrorResponse(message) if message == "device unavailable")
        );
        worker.join().unwrap().unwrap();
    }

    #[test]
    fn client_waits_for_completion_before_requesting_status() {
        let (client, server) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let (sender, receiver) = wake::channel::<Request>().unwrap();
        let worker = thread::spawn(move || handle_client(server, &sender, CLIENT_IO_TIMEOUT));
        BacklightCommand::SetMode(BacklightMode::Auto)
            .serialize_into(&client)
            .unwrap();
        BacklightCommand::GetInfo.serialize_into(&client).unwrap();
        let request = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(receiver.try_recv().is_err());
        let info = BacklightInfo {
            brightness_percent: 50,
            mode: BacklightMode::Auto,
            device: None,
        };
        request.reply.send(Ok(info.clone())).unwrap();
        let status = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(status.command, BacklightCommand::GetInfo));
        status.reply.send(Ok(info)).unwrap();
        assert!(
            matches!(BacklightCommand::deserialize_from(BufReader::new(&client)).unwrap(), BacklightCommand::GetInfoResponse(value) if value.brightness_percent == 50 && value.mode == BacklightMode::Auto)
        );
        BacklightCommand::NotifyShutdown
            .serialize_into(&client)
            .unwrap();
        worker.join().unwrap().unwrap();
    }
}
