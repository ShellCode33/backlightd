# Installation

## Requirements

- Linux; the supplied installer uses systemd and udev.
- Rust 1.86 or newer, Cargo, a C build toolchain, pkg-config, and libudev development
  headers. Debian/Ubuntu call the development package `libudev-dev`.
- Laptop backlights exposed under `/sys/class/backlight`.
- For external monitors: DDC/CI support enabled in the monitor's on-screen settings
  and accessible `/dev/i2c-*` devices. The `i2c-dev` kernel module may be needed.

Hardware, adapters, docks, and drivers vary. A working video connection does not
guarantee DDC/CI control. The project uses `ddc-hi`, not libddcutil.

## Build and install from source

From the checked-out repository root:

```sh
cargo build --workspace --release --locked
sudo make install
```

The installer writes both binaries to `/usr/local/bin`, the service to
`/etc/systemd/system/backlightd.service`, the rule to
`/etc/udev/rules.d/95-backlightd.rules`, and a commented configuration to
`/etc/backlightd/config.toml` if that file does not already exist. It reloads udev
and systemd, enables the service, and restarts it. Installation affects live
brightness because the daemon starts in auto mode.

A custom binary directory is supported:

```sh
sudo make install INSTALL_PREFIX=/usr/local/bin
```

`CONFIG_DIR` changes the installation destination only; it does not change the
daemon's default lookup path. A custom runtime path requires `--config PATH` in
the daemon invocation. Packaging recipes and distribution repositories are not
yet part of this installation workflow.

## Service lifecycle

```sh
systemctl status backlightd
sudo systemctl stop backlightd
sudo systemctl start backlightd
sudo systemctl restart backlightd
journalctl -u backlightd -b
```

For configuration changes, prefer validation and SIGHUP reload over restart:

```sh
backlightd --check-config
sudo systemctl kill --signal=HUP backlightd
```

There is currently no `ExecReload` command in the supplied service, so use the
explicit signal command above. Restarting starts auto mode again; manual mode
and its remaining override duration are not persisted across daemon restarts.

SIGTERM/SIGINT wait for the current device operation and clean up the owned socket.
A permanently stuck hardware call may still require systemd's stop timeout.
A second daemon using the same socket is refused. The `.lock` sidecar is persistent;
its presence alone does not mean the daemon is running, and it should not be deleted.

See [[Troubleshooting]] if installation or startup fails.
