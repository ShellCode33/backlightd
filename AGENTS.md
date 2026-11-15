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
Laptop power control uses `bl_power` (0 on, 4 off), independently of brightness.
Do not emulate it by zeroing brightness or claim that it controls compositor DPMS.

## Workspace map

- `backlightd/src/main.rs`: event-driven listener, bounded client workers,
  request/reply routing, controller supervision, and coordinated shutdown.
- `backlightd/src/shutdown.rs`: blocks SIGTERM/SIGINT before spawning workers
  and receives them through signalfd on the main thread.
- `backlightd/src/controller.rs`: serializes commands, discovery, mode changes,
  brightness targets, deadlines, and hotplug debouncing.
- `backlightd/src/monitors.rs`: active devices, stable identities, manual-target
  inheritance, failed-device recovery, and brightness conversion helpers.
- `backlightd/src/ddc.rs`: external monitors via `ddc-hi`; VCP 0x10 is brightness,
  VCP 0xD6 is power. Initialization retries transient brightness-probe failures.
- `backlightd/src/acpi.rs`: laptop backlight through `/sys/class/backlight`.
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
- `Makefile`, `backlightd.service`, `95-backlightd.rules`: installation and runtime
  integration. `$INSTALL_PREFIX` is substituted into the service and rule.

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

Manual brightness commands establish a target and a 12-hour override. Record
intent before device I/O so partial failures do not lose the requested target.
Relative adjustments operate on each device's current stored percentage; values
saturate at 100%, and relative decreases stop at 1%. Explicit 0% is allowed.

New or recovered devices inherit the manual target. Continuously present devices
retain externally changed brightness when ordinary discovery rereads them.
Identity must distinguish devices even when model names are identical. Failed
inheritance remains pending for retry. Detected resume reconciles known devices
too, since displays may reset brightness. Discovery must not extend the manual
override; expired overrides and auto mode must not reapply an obsolete target.
Status reports the integer average of stored device percentages, not a fresh
hardware read. Avoid treating those values as authoritative forever.

Auto mode computes both the target and the next time it can change. Keep the
four-hour solar ramps; missing solar events or overlapping transitions fall back
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
enabled by default; `BACKLIGHTD_ENABLE_LOCATION_API=0` disables it. The lookup
uses HTTP ip-api.com with a two-second request budget. Valid stale coordinates
survive lookup failures; retries back off for five minutes. The TOML cache is
`/var/cache/backlightd/last_known_location.toml`, refreshed after 12 hours.
Cache write failures must not discard a successful lookup.

Avoid new dependencies when existing facilities suffice. External-monitor support
currently uses `ddc-hi`, not libddcutil bindings. Broad Linux distribution support
matters; do not introduce a dependency on a particular libddcutil shared library
version without discussion.

## Build and validation

The workspace declares Rust 1.86 as its minimum version. Linux builds need a C
toolchain, pkg-config, and libudev development files. Use the lockfile.

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
into the host. `make install` writes system files, reloads udev and systemd, enables
the service, and restarts it. It expects release binaries to exist already.

Debug logging uses `BACKLIGHTD_LOG_LEVEL=backlightd=debug`; inspect system logs
with `journalctl -u backlightd`, not the user service journal. Discovery timings
separate enumeration, initialization, and adjustment. A successful DDC write
means the protocol operation succeeded, not that a hardware readback verified it.
When reporting hardware results, distinguish logged success from observed output.
