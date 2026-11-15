# backlightd

Control your laptop backlight and external monitors together, with one command.
backlightd runs in the background and can gradually adjust brightness around
sunrise and sunset. Use it for desktop keybindings, status bars, or automatic
brightness; hotplugged monitors inherit the current target.

Built for Linux with systemd and udev. Laptop screens use Linux's backlight
interface; external monitors require DDC/CI support.

## Install

Download a package for your distribution and CPU from
[GitHub Releases](https://github.com/ShellCode33/backlightd/releases).
See the [installation guide](https://github.com/ShellCode33/backlightd/wiki/Installation)
for package commands, hardware requirements, and building from source, or the
[NixOS guide](https://github.com/ShellCode33/backlightd/wiki/NixOS) for declarative setup.

## Use

```sh
backlightctl           # Show brightness and mode
backlightctl -b 60%    # Set all screens to 60%
backlightctl -b +5%    # Increase brightness by 5 percentage points
backlightctl --auto    # Resume automatic brightness
backlightctl --json    # Status for scripts and status bars
```

A manual adjustment overrides automatic brightness for 12 hours by default,
including time suspended. See the [CLI reference](https://github.com/ShellCode33/backlightd/wiki/CLI)
for all commands and [integrations](https://github.com/ShellCode33/backlightd/wiki/Integrations)
for shortcuts and Waybar setup.

## Configure

Settings live in `/etc/backlightd/config.toml`, with comments explaining each
option. Configure automatic schedules, location, manual override duration, and
per-monitor brightness ranges or curves. IP location lookup is opt-in.

The [user wiki](https://github.com/ShellCode33/backlightd/wiki) covers
[configuration and reloads](https://github.com/ShellCode33/backlightd/wiki/Configuration),
[automatic brightness](https://github.com/ShellCode33/backlightd/wiki/Automatic-brightness),
[calibration](https://github.com/ShellCode33/backlightd/wiki/Brightness-and-calibration),
and [troubleshooting](https://github.com/ShellCode33/backlightd/wiki/Troubleshooting).

For developer guides and the IPC API, run `cargo doc --workspace --no-deps --open`.
See [AGENTS.md](AGENTS.md) for contributor guidance and
[packaging documentation](packaging/README.md) for the release workflow.
