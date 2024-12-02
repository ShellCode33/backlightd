use std::{
    env,
    fs::{self, remove_file},
    io,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    process::exit,
    sync::mpsc::{channel, Sender},
    thread::{self, sleep},
    time::Duration,
};

use anyhow::{anyhow, bail};
use auto::auto_adjust;
use backlight_ipc::{BacklightCommand, BacklightInfo, BacklightMode, DEFAULT_UNIX_SOCKET_PATH};
use monitors::auto_refresh_monitors_list;

mod acpi;
mod auto;
mod ddc;
mod location;
mod monitors;

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

    let (auto_sender, auto_receiver) = channel();

    let auto_refresh_monitors_thread = thread::spawn(move || auto_refresh_monitors_list());
    let auto_adjust_thread = thread::spawn(move || auto_adjust(auto_receiver));
    let handle_clients_thread = thread::spawn(move || handle_clients_thread(listener, auto_sender));

    loop {
        if auto_refresh_monitors_thread.is_finished() {
            panic!("auto refresh monitors thread is gone");
        }

        if auto_adjust_thread.is_finished() {
            panic!("auto adjust thread is gone");
        }

        if handle_clients_thread.is_finished() {
            panic!("handle_clients_thread thread is gone");
        }

        sleep(Duration::from_secs(5));
    }
}

fn handle_clients_thread(listener: UnixListener, auto_adjust_sender: Sender<BacklightMode>) {
    for stream in listener.incoming() {
        let client = match stream {
            Ok(client) => client,
            Err(err) => {
                log::error!("Failed to accept incoming client: {err}");
                continue;
            }
        };

        if let Err(err) = handle_client(client, auto_adjust_sender.clone()) {
            log::error!("{err}");
            continue;
        }
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

fn handle_client(
    client: UnixStream,
    auto_adjust_sender: Sender<BacklightMode>,
) -> anyhow::Result<()> {
    loop {
        let command = match BacklightCommand::deserialize_from(&client) {
            Ok(cmd) => cmd,
            Err(err) => bail!("Unable to deserialize command: {err}"),
        };

        let result = match command {
            BacklightCommand::SetBrightness(percent) => {
                auto_adjust_sender
                    .send(BacklightMode::Manual)
                    .expect("Failed to send BacklightMode through auto adjust channel");
                monitors::set_brightness_percent(percent)
            }
            BacklightCommand::IncreaseBrightness(percent) => {
                auto_adjust_sender
                    .send(BacklightMode::Manual)
                    .expect("Failed to send BacklightMode through auto adjust channel");
                monitors::increase_brightness_percent(percent)
            }
            BacklightCommand::DecreaseBrightness(percent) => {
                auto_adjust_sender
                    .send(BacklightMode::Manual)
                    .expect("Failed to send BacklightMode through auto adjust channel");
                monitors::decrease_brightness_percent(percent)
            }
            BacklightCommand::Refresh => {
                monitors::refresh_monitors_list();
                Ok(())
            }
            BacklightCommand::SetMode(backlight_mode) => {
                auto_adjust_sender
                    .send(backlight_mode)
                    .unwrap_or_else(|err| {
                        log::error!("Failed to send mode to auto adjust channel: {err}")
                    });
                Ok(())
            }
            BacklightCommand::GetInfo => {
                BacklightCommand::GetInfoResponse(BacklightInfo {
                    brightness_percent: monitors::get_average_brightness(),
                })
                .serialize_into(&client)
                .unwrap_or_else(|err| log::error!("Unable to serialize GetInfoResponse: {err}"));
                Ok(())
            }
            BacklightCommand::GetInfoResponse(_) => {
                log::warn!("Got GetInfoResponse from client, API misuse ?");
                Ok(())
            }
            BacklightCommand::NotifyShutdown => break,
        };

        if let Err(err) = result {
            log::error!("Command handling failed: {err}");
        }
    }

    Ok(())
}
