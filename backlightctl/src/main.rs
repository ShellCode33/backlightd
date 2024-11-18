use std::{os::unix::net::UnixStream, path::PathBuf, process::exit};

use backlight_ipc::{BacklightCommand, DEFAULT_UNIX_SOCKET_PATH};
use clap::Parser;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct BacklightctlCli {
    /// Set the brightness of all monitors (valid values examples: 50% +10% -10%)
    #[clap(short, long)]
    #[structopt(allow_hyphen_values = true)]
    brightness: Option<String>,

    /// Refresh the list of known monitors (called by the udev rule)
    #[clap(short, long, default_value_t = false)]
    refresh: bool,

    /// UNIX socket path (for test purposes)
    #[clap(short, long, default_value = DEFAULT_UNIX_SOCKET_PATH)]
    unix_socket_path: PathBuf,
}

fn main() {
    let cli = BacklightctlCli::parse();

    let brightness_cmd = if let Some(brightness) = cli.brightness {
        if brightness.chars().last().is_some_and(|c| c != '%') {
            eprintln!("Brightness value is missing a % sign at the end");
            exit(1);
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
                    eprintln!("Unable to parse brightness value {brightness}: {err}");
                    exit(1);
                }
            };

            if brightness > 100 {
                eprintln!("Brightness value must be a percentage between -100% and 100%");
                exit(1);
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
                    eprintln!("Unable to parse brightness value {brightness}: {err}");
                    exit(1);
                }
            };

            if brightness > 100 {
                eprintln!("Brightness value must be a percentage between -100% and +100%");
                exit(1);
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
                    eprintln!("Unable to parse brightness value {brightness}: {err}");
                    exit(1);
                }
            };

            if brightness > 100 {
                eprintln!("Brightness value must be a percentage between -100% and +100%");
                exit(1);
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

    if let Some(brightness_cmd) = brightness_cmd {
        if let Err(err) = brightness_cmd.serialize_into(&stream) {
            eprintln!("{err}");
            exit(1);
        }
    }
}
