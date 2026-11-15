# Working on backlightd

## Scope and workflow

This is a Linux Rust workspace for controlling laptop and external-monitor
brightness through a system daemon and a Unix-socket client. Favor small,
reviewable changes; keep each independent fix in its own Conventional Commit.
Use the configured Git author identity. Do not install, restart the real daemon,
or change monitor state merely to validate a code change. Hardware tests affect
the user's live display session and should be explicitly coordinated.

Read the current diff before editing and preserve unrelated user changes. Keep
README.md short and user-facing; put architecture and contributor guidance here.
Sysfs raw brightness uses `u32`; shared percentage arithmetic uses `u64`.
DDC narrows values with a checked conversion at its 16-bit protocol boundary.
Legacy `acpi:` device IDs remain stable for existing configuration rules.
Laptop power control uses `bl_power` (0 on, 4 off), independently of brightness.
Do not emulate it by zeroing brightness or claim that it controls compositor DPMS.

## Workspace map

- `backlightd/src/main.rs`: event-driven listener, bounded client workers,
  request/reply routing, controller supervision, and coordinated shutdown.
- `backlightd/src/shutdown.rs`: blocks SIGTERM/SIGINT before spawning workers
  and receives them through signalfd on the main thread. SIGHUP loads and validates
  configuration before queueing its application through the controller.
- `backlightd/src/calibration.rs`: logical/hardware mapping, optional curves,
  per-device overrides, and stable EDID fingerprints. Preserve requested logical
  intent separately from hardware quantization.
- `backlightd/src/config.rs`: strict TOML parsing, validation, immutable snapshots,
  and compatibility environment overrides. Default path: `/etc/backlightd/config.toml`.
- `backlightd/src/controller.rs`: serializes commands, discovery, mode changes,
  brightness targets, deadlines, and hotplug debouncing.
- `backlightd/src/monitors.rs`: active devices, stable identities, manual-target
  inheritance, failed-device recovery, and brightness conversion helpers.
- `backlightd/src/ddc.rs`: external monitors via `ddc-hi`; VCP 0x10 is brightness,
  VCP 0xD6 is power. Initialization retries transient brightness-probe failures.
- `backlightd/src/sysfs.rs`: laptop backlight through `/sys/class/backlight`.
- `backlightd/src/auto.rs`: solar/clock schedules and next adjustment deadlines.
- `backlightd/src/location.rs`: environment configuration, IP lookup, persisted
  location, validation, timeouts, and retry backoff.
- `backlightd/src/socket.rs`: filesystem socket ownership, stale-socket checks,
  and a lifetime-held flock on a persistent `.lock` sidecar. Never unlink that
  lock file during cleanup; concurrent processes may already have it open.
- `backlightd/src/wake.rs`: command queue notifications via a Unix socket pair
  and suspend-aware deadline waits via libc timerfd/poll.
- `backlightctl/src/main.rs`: CLI and IPC client.
- `backlight_ipc/src/lib.rs`: shared commands, responses, and wire format.
- `Makefile` and the mirrored `etc/` tree: installation and runtime integration.
  `etc/backlightd/config.toml` is the commented configuration; service and udev
  files live under `etc/systemd/system/` and `etc/udev/rules.d/`. `$INSTALL_PREFIX` is substituted into the service and rule.

## Behavior to preserve

### Ordering and device discovery

The controller owns the sequence of device operations. Commands intentionally
wait for an active scan to finish so subsequent adjustments use refreshed devices
and readings. Do not move discovery to an independent worker without an explicit
ordering design and agreement on the behavioral change.

Full discovery runs at startup, on refresh, after detected resume, during error
recovery, and every 60 seconds. The periodic scan is a deliberate safeguard for
old or unusual hardware and missed notifications. Do not remove it or increase
its interval solely because hotplug notifications exist.

Udev invokes `backlightctl --notify-hotplug`, a notification that does not wait
for discovery or return status. The daemon groups notifications using a 250 ms
settling window with a one-second cap. Notifications received after a scan began
must remain eligible for a follow-up scan. A normal `--refresh` waits for completion;
requests covered by a later-started scan reuse its result, including errors.
Partial discovery retains usable monitors but reports failures; auto mode still
adjusts the valid devices.

### Brightness and modes

Manual brightness commands establish a target and a configurable override
(default 12 hours, or `"forever"`). Reload preserves the original adjustment time. Record
intent before device I/O so partial failures do not lose the requested target.
Relative adjustments operate on each device's current stored percentage; values
saturate at 100%, and relative decreases stop at 1%. Explicit 0% is allowed.

New or recovered devices inherit the manual target. Continuously present devices
retain externally changed brightness when ordinary discovery rereads them.
Identity must distinguish devices even when model names are identical. Failed
inheritance remains pending for retry. Detected resume reconciles known devices
too, since displays may reset brightness. Discovery must not extend the manual
override; expired overrides and auto mode must not reapply an obsolete target.
Status reports the integer average of logical device targets, falling back to
inverse-mapped hardware values when no target is retained, not a fresh hardware
read. Device details still report representable percentages independently. Relative adjustments use retained requested targets,
so coarse hardware steps do not prevent small repeated adjustments. Discovery
preserves this intent only if calibration and the observed raw value are unchanged;
an external raw change resets it. Disabled devices remain discoverable/listed but
are excluded from writes and averages. Detailed device data separates requested,
representable, raw control readback, and optional sysfs actual readings. A write
clears its readback evidence; do not label representable output as verified.
`GetDeviceInfo(index)` returns one optional device in `GetInfoResponse.device`,
allowing --devices to enumerate without exceeding the per-message limit. Normal
status omits this field to preserve its existing JSON shape.

Auto mode computes both the target and the next time it can change. Keep the
configured solar ramps (four hours by default); missing solar events or overlapping transitions fall back
to the clock schedule rather than compressing ramps. Preserve behavior across
midnight and unusual time zones.

### Clocks, I/O, and protocol

Use `boot_time::Instant` for elapsed deadlines that must include suspend. The
controller detects suspend by comparing BOOTTIME and MONOTONIC progress when it
wakes; this is not a separate immediate resume notification. The timerfd wait
uses CLOCK_BOOTTIME. Keep the service's syscall allowances consistent with it.

IPC is newline-delimited serde JSON with a 4 KiB message limit. Keep one buffered
reader per connection, preserve command ordering and error propagation, and
bound client resources and socket I/O. There is no bincode compatibility layer.
The default socket is `/run/backlightd.sock`.

On termination, stop accepting clients, close their connections, and signal the
controller to exit after its current operation. Join the controller and client
workers before releasing socket ownership. Do not execute queued commands after
shutdown is requested. `NotifyShutdown` from IPC closes only that connection;
it is not a public daemon shutdown command. Signal tests must run in isolated
child processes with fake controllers, never against the live daemon. A blocked
hardware operation may still require systemd's stop timeout to terminate it.

Location comes from `BACKLIGHTD_LOCATION=latitude,longitude` first. IP lookup is
opt-in through `[location] source = "ip"`; `BACKLIGHTD_ENABLE_LOCATION_API=0` disables it. The lookup
uses HTTP ip-api.com with a two-second request budget. Valid stale coordinates
survive lookup failures; retries back off for five minutes. The TOML cache is
`/var/cache/backlightd/last_known_location.toml`, refreshed after 12 hours.
Cache write failures must not discard a successful lookup.

Avoid new dependencies when existing facilities suffice. External-monitor support
currently uses `ddc-hi`, not libddcutil bindings. Broad Linux distribution support
matters; do not introduce a dependency on a particular libddcutil shared library
version without discussion.

## Documentation

The user manual lives in the separate GitHub wiki repository:
`git@github.com:ShellCode33/backlightd.wiki.git`, published at
https://github.com/ShellCode33/backlightd/wiki. Clone it separately to edit usage
pages; keep `Home.md`, `_Sidebar.md`, and wiki page links consistent. User manual
Markdown is no longer stored or installed from this repository.
Code-focused guides live under `backlightd/src/docs/` and are included by the
lightweight `backlightd/src/lib.rs` documentation target; runtime internals remain
in the executable. Document public wire types in `backlight_ipc/src/lib.rs`.
Run rustdoc with warnings denied and run doctests after changing Rust examples.
See `docs/README.md` for build and future publication instructions. Do not publish
crates or push the separate wiki repository without explicit authorization.

## Build and validation

Use the latest stable Rust toolchain, selected by `rust-toolchain.toml`; older
compiler versions are not supported. CI and native packaging use stable. Nix
selects stable through rust-overlay, pinned by `flake.lock`; update that input
when a new stable compiler is released. Linux builds need a C toolchain,
pkg-config, and libudev development files. Use the Cargo lockfile.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --release --locked
```

Add `--offline` when dependencies are already cached and network access is not
needed. Socket and timer tests need real Unix socket/timer syscalls, which some
sandboxes deny; request the appropriate execution permission rather than skipping
them. Use fake devices and injected clocks/lookup results for regression tests.
For documentation-only changes, check accuracy and the diff; no full test run is
needed. Solar/time changes warrant checks in multiple time zones.

Validate packaging changes with staged files or dry runs rather than installing
into the host. `make install DESTDIR=...` only copies files, preserves an existing
configuration, and expects release binaries to exist already. `make activate`
explicitly reloads integrations and enables/starts the service; never run it as
validation. Native packages use vendor unit/rule directories under `/usr/lib`.

`packaging/` contains native recipes, lifecycle scripts, and container build/check
helpers. `packaging/test.sh` checks staged installation without host side effects.
The packaging workflow tests Debian/Ubuntu, RPM, and Arch packages on every push
in native distro containers on matching CPU runners, and checks the Nix
flake/module. Branch pushes only upload workflow artifacts. Version tags or an
explicit tagged manual dispatch publish releases after all required jobs succeed. Keep all three Cargo package versions in
sync; version tags must match `vX.Y.Z`. See `packaging/README.md`. Do not push tags,
publish releases, or modify distribution repositories without authorization.

Debug logging uses `BACKLIGHTD_LOG_LEVEL=backlightd=debug`; inspect system logs
with `journalctl -u backlightd`, not the user service journal. Discovery timings
separate enumeration, initialization, and adjustment. A successful DDC write
means the protocol operation succeeded, not that a hardware readback verified it.
When reporting hardware results, distinguish logged success from observed output.
