use std::{
    env,
    fs::{self, remove_file},
    io,
    os::unix::net::UnixListener,
    process::exit,
    sync::{
        mpsc::{channel, Receiver, RecvTimeoutError},
        Mutex,
    },
    thread,
    time::Duration,
};

use acpi::{BacklightAcpiDevice, ACPI_DEVICES_PATH};
use backlight_ipc::{BacklightCommand, DEFAULT_UNIX_SOCKET_PATH};
use ddc::BacklightDdcDevice;

mod acpi;
mod ddc;

/// Holds the current backlight state of all monitors.
///
/// Querying/writing to backlight devices is expensive, we try to cache as many things as possible
/// in order to avoid unnecessary I/O.
static MONITORS: Mutex<Vec<Box<dyn BacklightDevice + Send>>> = Mutex::new(Vec::new());
const MONITORS_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

trait BacklightDevice {
    fn name(&self) -> String;
    fn set_brightness(&mut self, percent: u8) -> anyhow::Result<()>;
    fn get_brightness(&self) -> u8;
}

fn refresh_monitors_list() {
    // Don't modify the global MONITORS variable directly to avoid locking the mutex for too long.
    let mut new_monitors: Vec<Box<dyn BacklightDevice + Send>> = Vec::new();

    for ddc_device in ddc_hi::Display::enumerate() {
        match BacklightDdcDevice::new(ddc_device) {
            Ok(monitor) => new_monitors.push(Box::new(monitor)),
            Err(err) => eprintln!("Failed to retrieve DDC backlight monitor: {err}"),
        }
    }

    match fs::read_dir(ACPI_DEVICES_PATH) {
        Ok(dir) => {
            for entry in dir {
                match entry {
                    Ok(file) => match BacklightAcpiDevice::new(file.path()) {
                        Ok(monitor) => new_monitors.push(Box::new(monitor)),
                        Err(err) => println!("Failed to retrieve ACPI backlight monitor: {err}"),
                    },
                    Err(err) => {
                        eprintln!("Unable to read entry from {ACPI_DEVICES_PATH}: {err}");
                    }
                }
            }
        }
        Err(err) => {
            eprintln!("{ACPI_DEVICES_PATH}: {err}");
            // fallthrough
        }
    }

    let mut monitors = MONITORS.lock().unwrap();
    monitors.clear();
    monitors.extend(new_monitors);
}

fn set_brightness_percent(percent: u8) -> anyhow::Result<()> {
    for monitor in MONITORS.lock().unwrap().iter_mut() {
        monitor.set_brightness(percent)?;
    }
    Ok(())
}

fn increase_brightness_percent(percent: u8) -> anyhow::Result<()> {
    for monitor in MONITORS.lock().unwrap().iter_mut() {
        let mut new_brightness = monitor.get_brightness() + percent;

        if new_brightness > 100 {
            new_brightness = 100;
        }

        monitor.set_brightness(new_brightness)?;
    }
    Ok(())
}

fn decrease_brightness_percent(percent: u8) -> anyhow::Result<()> {
    for monitor in MONITORS.lock().unwrap().iter_mut() {
        let mut new_brightness = monitor.get_brightness() as isize - percent as isize;

        // Don't allow setting the brightness to 0 to prevent the monitor from being completely black.
        if new_brightness < 1 {
            new_brightness = 1;
        }

        monitor.set_brightness(new_brightness as u8)?;
    }
    Ok(())
}

fn handle_commands(cmd_receiver: Receiver<BacklightCommand>) {
    loop {
        refresh_monitors_list();

        loop {
            match cmd_receiver.recv_timeout(MONITORS_REFRESH_INTERVAL) {
                Ok(command) => {
                    let result = match command {
                        BacklightCommand::SetBrightness(percent) => set_brightness_percent(percent),
                        BacklightCommand::IncreaseBrightness(percent) => {
                            increase_brightness_percent(percent)
                        }
                        BacklightCommand::DecreaseBrightness(percent) => {
                            decrease_brightness_percent(percent)
                        }
                        BacklightCommand::Refresh => {
                            refresh_monitors_list();
                            Ok(())
                        }
                    };

                    if let Err(err) = result {
                        eprintln!("Command handling failed: {err}");
                    }
                }
                Err(err) => match err {
                    RecvTimeoutError::Timeout => break,
                    RecvTimeoutError::Disconnected => panic!("channel disconnected"),
                },
            }
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

    let command_handler_thread = thread::spawn(move || handle_commands(cmd_receiver));

    for stream in listener.incoming() {
        if command_handler_thread.is_finished() {
            panic!("command handler thread is gone");
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
