//! CLI translation and presentation for the shared backlight IPC protocol.
//!
//! Mutations are followed by a status request on the same connection. Device
//! enumeration fetches one indexed record per bounded response; normal JSON status
//! retains its two-field shape. See `docs/wiki/CLI.md` for the user reference.

use std::{io::BufReader, os::unix::net::UnixStream, path::PathBuf, process::exit};

use backlight_ipc::{BacklightCommand, BacklightMode, DEFAULT_UNIX_SOCKET_PATH};
use clap::{error::ErrorKind, CommandFactory, Parser};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct BacklightctlCli {
    /// Set the brightness of all monitors (valid values examples: 50% +10% -10%)
    #[clap(short, long)]
    #[structopt(allow_hyphen_values = true)]
    brightness: Option<String>,

    /// Set backlightd mode to auto (the daemon will automatically adjust brightness)
    #[clap(short, long, default_value_t = false)]
    auto: bool,

    /// Turn off monitors
    #[clap(long, default_value_t = false)]
    turn_off: bool,

    /// Turn on monitors
    #[clap(long, default_value_t = false)]
    turn_on: bool,

    /// Refresh the list of known monitors and wait for completion
    #[clap(short, long, default_value_t = false)]
    refresh: bool,

    /// Notify the daemon of a hotplug event without waiting for discovery (for udev)
    #[clap(long, conflicts_with_all = ["brightness", "auto", "turn_off", "turn_on", "refresh", "json"])]
    notify_hotplug: bool,

    /// List monitors, stable configuration IDs, and brightness details
    #[clap(long, conflicts_with_all = ["brightness", "auto", "turn_off", "turn_on", "notify_hotplug"])]
    devices: bool,

    /// UNIX socket path (for test purposes)
    #[clap(short, long, default_value = DEFAULT_UNIX_SOCKET_PATH)]
    unix_socket_path: PathBuf,

    /// Output will be JSON
    #[clap(long, default_value_t = false)]
    json: bool,
}

fn main() {
    let cli = BacklightctlCli::parse();

    if cli.auto && cli.brightness.is_some() {
        BacklightctlCli::command()
            .error(
                ErrorKind::ArgumentConflict,
                "You cannot use both --brightness and --auto",
            )
            .exit();
    }

    if cli.turn_on && cli.turn_off {
        BacklightctlCli::command()
            .error(
                ErrorKind::ArgumentConflict,
                "You cannot use both --turn-on and --turn-off",
            )
            .exit();
    }

    let brightness_cmd = if let Some(brightness) = cli.brightness {
        if brightness.chars().last().is_some_and(|c| c != '%') {
            BacklightctlCli::command()
                .error(
                    ErrorKind::InvalidValue,
                    "Brightness value is missing a % sign at the end",
                )
                .exit();
        }

        let potential_brightness_modifier = brightness.chars().next();

        if potential_brightness_modifier.is_some_and(|c| c == '+') {
            let brightness = brightness
                .chars()
                .skip(1)
                .take_while(|&c| c != '%')
                .collect::<String>();

            let brightness = match brightness.parse::<u8>() {
                Ok(percent) => percent,
                Err(err) => {
                    BacklightctlCli::command()
                        .error(
                            ErrorKind::InvalidValue,
                            format!("Unable to parse brightness value {brightness}: {err}"),
                        )
                        .exit();
                }
            };

            if brightness > 100 {
                BacklightctlCli::command()
                    .error(
                        ErrorKind::InvalidValue,
                        "Brightness value must be a percentage between -100% and 100%",
                    )
                    .exit();
            }

            Some(BacklightCommand::IncreaseBrightness(brightness))
        } else if potential_brightness_modifier.is_some_and(|c| c == '-') {
            let brightness = brightness
                .chars()
                .skip(1)
                .take_while(|&c| c != '%')
                .collect::<String>();

            let brightness = match brightness.parse::<usize>() {
                Ok(percent) => percent,
                Err(err) => {
                    BacklightctlCli::command()
                        .error(
                            ErrorKind::InvalidValue,
                            format!("Unable to parse brightness value {brightness}: {err}"),
                        )
                        .exit();
                }
            };

            if brightness > 100 {
                BacklightctlCli::command()
                    .error(
                        ErrorKind::InvalidValue,
                        "Brightness value must be a percentage between -100% and 100%",
                    )
                    .exit();
            }

            Some(BacklightCommand::DecreaseBrightness(brightness as u8))
        } else {
            let brightness = brightness
                .chars()
                .take_while(|&c| c != '%')
                .collect::<String>();

            let brightness = match brightness.parse::<usize>() {
                Ok(percent) => percent,
                Err(err) => {
                    BacklightctlCli::command()
                        .error(
                            ErrorKind::InvalidValue,
                            format!("Unable to parse brightness value {brightness}: {err}"),
                        )
                        .exit();
                }
            };

            if brightness > 100 {
                BacklightctlCli::command()
                    .error(
                        ErrorKind::InvalidValue,
                        "Brightness value must be a percentage between -100% and 100%",
                    )
                    .exit();
            }

            Some(BacklightCommand::SetBrightness(brightness as u8))
        }
    } else {
        None
    };

    let stream = match UnixStream::connect(&cli.unix_socket_path) {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("{}: {err}", cli.unix_socket_path.display());
            exit(1);
        }
    };

    if cli.notify_hotplug {
        if let Err(err) = BacklightCommand::NotifyHotplug.serialize_into(&stream) {
            eprintln!("{err}");
            exit(1);
        }
        return;
    }

    if cli.refresh {
        if let Err(err) = BacklightCommand::Refresh.serialize_into(&stream) {
            eprintln!("{err}");
            exit(1);
        }
    }

    if cli.devices {
        let mut reader = BufReader::new(&stream);
        let mut devices = Vec::new();
        for index in 0.. {
            if let Err(err) = BacklightCommand::GetDeviceInfo(index).serialize_into(&stream) {
                eprintln!("{err}");
                exit(1);
            }
            match BacklightCommand::deserialize_from(&mut reader) {
                Ok(BacklightCommand::GetInfoResponse(info)) => match info.device {
                    Some(device) => devices.push(device),
                    None => break,
                },
                Ok(BacklightCommand::ErrorResponse(err)) => {
                    eprintln!("{err}");
                    exit(1);
                }
                other => {
                    eprintln!("Unexpected device response: {other:?}");
                    exit(1);
                }
            }
        }
        if cli.json {
            println!(
                "{}",
                serde_json::to_string(&devices).expect("serializable device info")
            );
        } else {
            for device in devices {
                println!("{} ({})", device.id, device.name);
                let requested = device
                    .requested_percent
                    .map(|p| format!("{p}%"))
                    .unwrap_or_else(|| "not recorded".into());
                let readback = device
                    .readback_raw
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unverified".into());
                let actual = device
                    .actual_raw
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unavailable".into());
                println!(
                    "  Enabled: {}; brightness: {}%; requested: {}; hardware: {}/{} ({}%)",
                    device.enabled,
                    device.brightness_percent,
                    requested,
                    device.hardware_raw,
                    device.maximum_raw,
                    device.hardware_percent
                );
                println!("  Control readback: {readback}; actual backlight reading: {actual}");
            }
        }
        let _ = BacklightCommand::NotifyShutdown.serialize_into(&stream);
        return;
    }

    if cli.auto {
        if let Err(err) = BacklightCommand::SetMode(BacklightMode::Auto).serialize_into(&stream) {
            eprintln!("{err}");
            exit(1);
        }
    } else if let Some(brightness_cmd) = brightness_cmd {
        if let Err(err) = brightness_cmd.serialize_into(&stream) {
            eprintln!("{err}");
            exit(1);
        }
    }

    if cli.turn_off {
        if let Err(err) = BacklightCommand::TurnOffMonitors.serialize_into(&stream) {
            eprintln!("{err}");
            exit(1);
        }
    } else if cli.turn_on {
        if let Err(err) = BacklightCommand::TurnOnMonitors.serialize_into(&stream) {
            eprintln!("{err}");
            exit(1);
        }
    }

    if let Err(err) = BacklightCommand::GetInfo.serialize_into(&stream) {
        eprintln!("{err}");
        exit(1);
    }

    let backlight_info = match BacklightCommand::deserialize_from(BufReader::new(&stream)) {
        Ok(BacklightCommand::GetInfoResponse(info)) => info,
        Ok(BacklightCommand::ErrorResponse(err)) => {
            eprintln!("{err}");
            exit(1);
        }
        Ok(cmd) => {
            eprintln!("Unexpected response: {cmd:?}");
            exit(1);
        }
        Err(err) => {
            eprintln!("{err}");
            exit(1);
        }
    };

    if cli.json {
        match serde_json::to_string(&backlight_info) {
            Ok(backlight_info_json) => {
                println!("{backlight_info_json}");
            }
            Err(err) => {
                eprintln!("Cannot serialize GetInfoResponse to json: {err}");
                exit(1);
            }
        }
    } else {
        println!("Current brightness: {}%", backlight_info.brightness_percent);
        println!("Current mode: {}", backlight_info.mode);
    }

    if let Err(err) = BacklightCommand::NotifyShutdown.serialize_into(&stream) {
        eprintln!("{err}");
        exit(1);
    }
}
