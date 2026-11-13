# backlightd

Control the brightness of your laptop screen and external monitors together, with
one command. backlightd runs in the background, keeps monitor connections ready,
and can gradually adjust brightness around sunrise and sunset.

Use it for desktop keybindings, status bars, or automatic brightness without
managing each screen separately. Laptop screens use Linux's backlight interface;
external monitors use DDC/CI. Hotplugged monitors inherit the current manual
target or automatic brightness.

## Install

You need Linux with systemd and udev, Rust 1.86 or newer, a C build toolchain,
`pkg-config`, and the libudev development package (`libudev-dev` on Debian/Ubuntu).
External monitors must support DDC/CI, have it enabled in their on-screen settings,
and expose an accessible `/dev/i2c-*` device; you may need the `i2c-dev` kernel module.

From the source directory:

```sh
cargo build --workspace --release --locked
sudo make install
```

Installation installs both commands, the system service, and the hotplug rule,
then enables and starts the daemon. Binaries default to `/usr/local/bin`; override
this with `sudo make install INSTALL_PREFIX=/your/bin`.

## Use

```sh
backlightctl                  # Show brightness and mode
backlightctl -b 60%           # Set all screens to 60%
backlightctl -b +5%           # Increase brightness by 5 percentage points
backlightctl -b -5%           # Decrease brightness by 5 percentage points
backlightctl --auto           # Resume automatic brightness
backlightctl --json           # Machine-readable brightness and mode
backlightctl --refresh        # Rediscover monitors and wait for completion
```

A manual adjustment overrides auto mode for 12 hours, including time suspended.
Automatic brightness transitions between 1% and 100% over four-hour ramps.
Without a usable solar schedule, it uses morning and evening ramps starting at
06:00 and 18:00 local time. There is no ambient-light sensor integration.

## Configure automatic brightness

By default, backlightd estimates your location through the external **ip-api.com**
service over HTTP and caches it. To supply coordinates and disable IP lookup,
run `sudo systemctl edit backlightd` and add:

```ini
[Service]
Environment=BACKLIGHTD_LOCATION=48.8566,2.3522
Environment=BACKLIGHTD_ENABLE_LOCATION_API=0
```

Then run `sudo systemctl restart backlightd`. To use only the clock-based schedule,
set `BACKLIGHTD_ENABLE_LOCATION_API=0` without specifying coordinates.

For troubleshooting, use `journalctl -u backlightd -b`. Enable debug logs by adding
`Environment=BACKLIGHTD_LOG_LEVEL=backlightd=debug` to the same service override
and restarting the daemon. Monitor and adapter support varies; check the logs
if a screen is missing or does not respond.
