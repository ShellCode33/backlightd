use std::{
    env,
    fs::remove_file,
    io,
    os::unix::net::UnixListener,
    process::exit,
    sync::mpsc::{channel, Receiver, Sender},
    thread,
};

use auto::auto_adjust;
use backlight_ipc::{BacklightCommand, BacklightMode, DEFAULT_UNIX_SOCKET_PATH};
use monitors::auto_refresh_monitors_list;

mod acpi;
mod auto;
mod ddc;
mod monitors;

fn handle_commands(
    cmd_receiver: Receiver<BacklightCommand>,
    auto_adjust_sender: Sender<BacklightMode>,
) -> ! {
    loop {
        let command = cmd_receiver
            .recv()
            .expect("Failed to receive command from cmd channel");
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
                        eprintln!("Failed to send mode to auto adjust channel: {err}")
                    });
                Ok(())
            }
        };

        if let Err(err) = result {
            eprintln!("Command handling failed: {err}");
        }
    }
}

fn main() {
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

    if let Err(err) = remove_file(unix_socket_path) {
        if !matches!(err.kind(), io::ErrorKind::NotFound) {
            eprintln!("{unix_socket_path}: {err}");
            exit(1);
        }
    }

    let listener = match UnixListener::bind(unix_socket_path) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("{unix_socket_path}: {err}");
            exit(1);
        }
    };

    let (cmd_sender, cmd_receiver) = channel();
    let (auto_sender, auto_receiver) = channel();

    let auto_refresh_monitors_thread = thread::spawn(move || auto_refresh_monitors_list());
    let command_handler_thread = thread::spawn(move || handle_commands(cmd_receiver, auto_sender));
    let auto_adjust_thread = thread::spawn(move || auto_adjust(auto_receiver));

    for stream in listener.incoming() {
        if auto_refresh_monitors_thread.is_finished() {
            panic!("auto refresh monitors thread is gone");
        }

        if command_handler_thread.is_finished() {
            panic!("command handler thread is gone");
        }

        if auto_adjust_thread.is_finished() {
            panic!("auto adjust thread is gone");
        }

        let stream = match stream {
            Ok(stream) => stream,
            Err(err) => {
                eprintln!("A client tried to connect but something wrong happened: {err}");
                continue;
            }
        };

        match BacklightCommand::deserialize_from(&stream) {
            Ok(command) => {
                cmd_sender.send(command).unwrap();
            }
            Err(err) => {
                eprintln!("Unable to deserialize command: {err}");
            }
        }
    }

    unreachable!()
}
