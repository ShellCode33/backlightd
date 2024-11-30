use std::{os::unix::net::UnixStream, path::PathBuf, process::exit};

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

    /// Refresh the list of known monitors (called by the udev rule)
    #[clap(short, long, default_value_t = false)]
    refresh: bool,

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

    if cli.refresh {
        if let Err(err) = BacklightCommand::Refresh.serialize_into(&stream) {
            eprintln!("{err}");
            exit(1);
        }
    }

    if cli.auto {
        if let Err(err) = BacklightCommand::SetMode(BacklightMode::Auto).serialize_into(&stream) {
            eprintln!("{err}");
            exit(1);
        }
    }

    if let Some(brightness_cmd) = brightness_cmd {
        if let Err(err) = brightness_cmd.serialize_into(&stream) {
            eprintln!("{err}");
            exit(1);
        }
    }

    if let Err(err) = BacklightCommand::GetInfo.serialize_into(&stream) {
        eprintln!("{err}");
        exit(1);
    }

    let backlight_info = match BacklightCommand::deserialize_from(&stream) {
        Ok(BacklightCommand::GetInfoResponse(info)) => info,
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
    }

    if let Err(err) = BacklightCommand::NotifyShutdown.serialize_into(&stream) {
        eprintln!("{err}");
        exit(1);
    }
}
