# Packaging and releases

One native package contains both `backlightd` and `backlightctl`, a commented
configuration, systemd unit, udev rule, README, and license. The user manual lives
in the separate GitHub wiki. See the
[installation guide](https://github.com/ShellCode33/backlightd/wiki/Installation) for supported distributions,
architectures, and commands. Target only the latest stable release of each
distribution; update the explicit container versions when new releases arrive.
Arch follows its rolling release, and Nix pins the latest stable NixOS channel.
These are upstream downloads, not official distro
repository packages. There is no automatic update repository or signing key yet.

## Release procedure

1. Set the same version in all three workspace member `Cargo.toml` files and
   refresh `Cargo.lock`. Use the latest stable Rust toolchain.
2. Commit and push the reviewed changes. Push a matching tag, e.g. `v0.1.0`.
3. The `Release packages` workflow runs the existing tests/linters, builds native
   packages, and checks the Nix package/module. Every native package is installed,
   checked, reinstalled after a local config edit, and removed in its container.
4. Only after every job passes does CI create a draft, upload all assets and
   `SHA256SUMS`, and publish it. A `-rc.1` style version becomes a prerelease.

The workflow can also be dispatched with an **existing** tag. It validates the
tag against the checked-out Cargo versions and checks/builds that tag's code.
Published releases are never overwritten. A failed upload can be retried against
the remaining draft. Download build artifacts from the workflow while diagnosing
failures; successful package jobs upload their artifacts even if another fails.
GitHub Actions needs permission to create releases; only the final publish job
receives `contents: write`. No registry account, PAT, or signing secret is required.

Assets include distro/CPU-specific `.deb`, `.rpm`, and `.pkg.tar.zst` files, a
versioned source archive, and the rendered Arch `PKGBUILD`/install script. The
source archive excludes untracked files. `SHA256SUMS` detects corruption; it is
not a detached cryptographic signature. Packages are currently unsigned.

Nix users consume the versioned flake and its `flake.lock`, not a foreign binary
package. `nix build` and `nix flake check` work for both supported Linux CPUs.
The NixOS module reuses the packaged unit and generates TOML configuration.
Nix uses rust-overlay to select stable Rust independently of the NixOS compiler.
Update it with `nix flake update rust-overlay` when a new stable compiler is
released; the lockfile keeps individual Nix builds reproducible.

## Build design

Native builds run in the target distribution container on a runner matching its
CPU. Rust uses the latest stable toolchain and is mounted read-only from the
runner; GCC, libudev headers, and other native libraries come from the container.
This prevents accidentally requiring a newer host glibc in a Debian/Ubuntu binary.
`--locked` is used throughout. Tests are serialized to keep forked signal-test
children from transiently holding other tests' file locks; explicit concurrency
tests still spawn their own workers.

`bootstrap.sh` installs native build dependencies **inside a disposable container**.
`container-build.sh FAMILY FORMAT ARCHIVE` builds and verifies packages there.
It expects `/opt/rust` to contain a complete native Rust sysroot, `/output` to be
writable, `BUILD_FLAVOR` to identify the distro, and `SOURCE_DATE_EPOCH` to come
from the release commit. See the release workflow for the exact Docker invocation.
The RPM template is rendered for Fedora or openSUSE; the Debian builder computes
shared-library requirements with `dpkg-shlibdeps`. Arch's `makepkg` runs as an
unprivileged build user. Native packages require systemd/udev integration.

Maintainer scripts reload systemd/udev when available, never auto-enable a fresh
install, restart an already active service on upgrade, and stop/disable on removal.
Native config handling uses Debian conffiles, RPM `%config(noreplace)`, or Arch
`backup`. No package creates a new access group; daemon socket permissions retain
the current application behavior.

## Local checks

```sh
cargo build --workspace --release --locked
packaging/test.sh
# From a committed checkout; tag must match the workspace versions:
RELEASE_TAG=v0.1.0 packaging/source.sh dist
nix flake check
```

`packaging/test.sh` stages installation twice, checks configuration preservation
and runtime paths, forbids systemctl/udevadm calls, validates the installed config,
and renders/checks templates. It runs in ordinary CI after the release build.
`make install DESTDIR=/tmp/stage PREFIX=/usr INSTALL_PREFIX=/usr/bin` only copies
files. Runtime paths never include `DESTDIR`. `make activate` is an explicit local
source-install action and must never be invoked during package builds or tests.
Do not run `bootstrap.sh`, `verify.sh`, or native package installation on your host
as a validation shortcut: they are intended for disposable containers only.
