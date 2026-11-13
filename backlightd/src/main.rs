use std::{
    env,
    fs::{self, remove_file},
    io::{self, BufReader},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    process::exit,
    sync::mpsc::channel,
    thread::{self, sleep, JoinHandle},
    time::Duration,
};

use anyhow::anyhow;
use backlight_ipc::{BacklightCommand, DEFAULT_UNIX_SOCKET_PATH};
use controller::{run_controller, Request};
use wake::Sender;

mod acpi;
mod auto;
mod controller;
mod ddc;
mod location;
mod monitors;
mod wake;

fn main() {
    pretty_env_logger::formatted_builder()
        .filter_level(log::LevelFilter::Info)
        .parse_env("BACKLIGHTD_LOG_LEVEL")
        .filter_module("ureq", log::LevelFilter::Warn)
        .init();

    let args: Vec<String> = env::args().collect();

    if args.len() != 1 && args.len() != 3 {
        eprintln!(
            "Usage: {} (optional: --unix-socket-path {DEFAULT_UNIX_SOCKET_PATH})",
            args[0]
        );
        exit(1);
    }

    let unix_socket_path = if args.len() == 3 {
        args[2].as_str()
    } else {
        DEFAULT_UNIX_SOCKET_PATH
    };

    let listener = match create_unix_socket(unix_socket_path) {
        Ok(listener) => listener,
        Err(err) => {
            log::error!("{unix_socket_path}: {err}");
            exit(1);
        }
    };

    let (auto_sender, auto_receiver) =
        wake::channel().expect("Unable to create controller wait descriptors");

    let auto_adjust_thread = thread::spawn(move || {
        if let Err(err) = run_controller(auto_receiver) {
            log::error!("Controller failed: {err}");
        }
    });
    let handle_clients_thread = thread::spawn(move || handle_clients_thread(listener, auto_sender));

    loop {
        if auto_adjust_thread.is_finished() {
            panic!("auto adjust thread is gone");
        }

        if handle_clients_thread.is_finished() {
            panic!("handle_clients_thread thread is gone");
        }

        sleep(Duration::from_secs(5));
    }
}

fn create_unix_socket(unix_socket_path: &str) -> anyhow::Result<UnixListener> {
    if let Err(err) = remove_file(unix_socket_path) {
        if !matches!(err.kind(), io::ErrorKind::NotFound) {
            return Err(anyhow!(err));
        }
    }

    let listener = UnixListener::bind(unix_socket_path)?;
    fs::set_permissions(unix_socket_path, fs::Permissions::from_mode(0o777))?;

    Ok(listener)
}

const MAX_CLIENTS: usize = 16;
const CLIENT_IO_TIMEOUT: Duration = Duration::from_secs(5);

fn handle_clients_thread(listener: UnixListener, sender: Sender<Request>) {
    let mut workers = Vec::new();
    for stream in listener.incoming() {
        match stream {
            Ok(client) => {
                if let Err(err) = spawn_client(client, sender.clone(), &mut workers) {
                    log::error!("Unable to serve client: {err}");
                }
            }
            Err(err) => log::error!("Failed to accept incoming client: {err}"),
        }
    }
}

fn spawn_client(
    client: UnixStream,
    sender: Sender<Request>,
    workers: &mut Vec<JoinHandle<()>>,
) -> io::Result<()> {
    workers.retain(|worker| !worker.is_finished());
    if workers.len() >= MAX_CLIENTS {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Too many client connections",
        ));
    }
    workers.push(
        thread::Builder::new()
            .name("backlight-client".into())
            .spawn(move || {
                if let Err(err) = handle_client(client, &sender, CLIENT_IO_TIMEOUT) {
                    log::error!("{err}");
                }
            })?,
    );
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
        let get_info = matches!(command, BacklightCommand::GetInfo);
        let (reply, response) = channel();
        sender
            .send(Request {
                command,
                received_at: boot_time::Instant::now(),
                reply,
            })
            .map_err(|_| anyhow!("Brightness controller disconnected"))?;
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
        };
        request.reply.send(Ok(info)).unwrap();
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
