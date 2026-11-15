# Installation

## Requirements

- Linux; the supplied installer uses systemd and udev.
- Laptop backlights exposed under `/sys/class/backlight`.
- For external monitors: DDC/CI support enabled in the monitor's on-screen settings
  and accessible `/dev/i2c-*` devices. The `i2c-dev` kernel module may be needed.

Hardware, adapters, docks, and drivers vary. A working video connection does not
guarantee DDC/CI control. The project uses `ddc-hi`, not libddcutil.

## Release packages

Download the file matching your distribution version and CPU from
[GitHub Releases](https://github.com/ShellCode33/backlightd/releases). These are
upstream packages; they are not entries in the distributions' official repositories.
Only the latest stable release of each distribution is targeted. Arch follows its
rolling release; Nix uses the latest stable NixOS channel pinned in `flake.lock`.
Rust and development headers are not needed to install a binary package.

| Distribution | Release asset | CPU architectures |
| --- | --- | --- |
| Debian 13 | `backlightd-VERSION-debianRELEASE-ARCH.deb` | amd64, arm64 |
| Ubuntu 26.10 | `backlightd-VERSION-ubuntuRELEASE-ARCH.deb` | amd64, arm64 |
| Fedora 45 | `backlightd-VERSION-1.fedoraRELEASE.ARCH.rpm` | x86_64, aarch64 |
| openSUSE Leap 16.1 | `backlightd-VERSION-1.opensuseleap161.ARCH.rpm` | x86_64, aarch64 |
| Arch Linux | `backlightd-VERSION-1-x86_64.pkg.tar.zst` | x86_64 |
| Nix / NixOS | Versioned flake; see [[NixOS]] | x86_64-linux, aarch64-linux |

For example, install the downloaded package using your package manager:

```sh
# Debian / Ubuntu
sudo apt install ./backlightd-0.1.0-debian13-amd64.deb
# Fedora
sudo dnf install ./backlightd-0.1.0-1.fedora45.x86_64.rpm
# openSUSE
sudo zypper install ./backlightd-0.1.0-1.opensuseleap161.x86_64.rpm
# Arch Linux
sudo pacman -U ./backlightd-0.1.0-1-x86_64.pkg.tar.zst
```

Use the actual filename of your downloaded asset. RPMs and Arch packages are
currently unsigned; their package managers may ask for confirmation. `SHA256SUMS`
checks download integrity and is distributed with the assets. There is no update
repository yet; install a newer release package with the same command to upgrade.

Packages install binaries to `/usr/bin`, service/rule files under `/usr/lib`, and
configuration to `/etc/backlightd/config.toml`. Local configuration edits are
preserved on upgrade through each package manager's configuration-file mechanism.
A fresh install does not start the daemon. Enable it explicitly:

```sh
sudo systemctl enable --now backlightd
```

Upgrading restarts the service if it was running, which resets it to auto mode.
Removal stops/disables it. Configuration retention on removal follows the package
manager (for example, Debian keeps it until `apt purge backlightd`).

If migrating from `make install`, inspect `systemctl cat backlightd` first.
Old source-installed binaries under `/usr/local/bin` and integration files under
`/etc/systemd/system` and `/etc/udev/rules.d` take precedence over packaged files.
Remove the old source-installed copies after reviewing them, preserving any
intentional local overrides, then reload systemd/udev. Keep your configuration.

## Build and install from source

Install the latest stable Rust toolchain, Cargo, a C toolchain, pkg-config, and libudev development headers
(`libudev-dev` on Debian/Ubuntu).

From the checked-out repository root:

```sh
cargo build --workspace --release --locked
sudo make install
sudo make activate
```

The installer writes both binaries to `/usr/local/bin`, the service to
`/etc/systemd/system/backlightd.service`, the rule to
`/etc/udev/rules.d/95-backlightd.rules`, and a commented configuration to
`/etc/backlightd/config.toml` if that file does not already exist.
`make install` only copies files;
`make activate` reloads udev/systemd and enables/starts the service. Starting it
affects live brightness because the daemon starts in auto mode. To replace an
already running build, use `sudo systemctl restart backlightd` after installation.

A custom binary directory is supported:

```sh
sudo make install INSTALL_PREFIX=/usr/local/bin
```

`CONFIG_DIR` changes the installation destination only; it does not change the
daemon's default lookup path. A custom runtime path requires `--config PATH` in
the daemon invocation. Package builders can stage installation with `DESTDIR` and
override `PREFIX`, `SYSTEMD_UNIT_DIR`, `UDEV_RULES_DIR`, and `DOC_DIR`. Staging never
includes `DESTDIR` in the installed service or hotplug command paths.

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
