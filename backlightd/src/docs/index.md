# backlightd developer documentation

backlightd is a Linux system daemon that coordinates brightness across sysfs
backlights and external DDC/CI displays. `backlightctl` communicates with it over
a Unix stream socket using the types in [`ipc`].

This library target provides technical guides and re-exports the real IPC library.
The daemon implementation remains in the executable target: its controllers and
hardware adapters are internal, not a supported embeddable library API. No hardware
is accessed by importing this library or building these docs.

## Start here

- [`architecture`]: ownership, request ordering, discovery, and lifecycle.
- [`brightness`]: mapping, numerical accuracy, and measurement boundaries.
- [`configuration`]: validation, reload, manual deadlines, and location caching.
- [`development`]: source navigation and regression-test strategy.
- [`ipc`]: public protocol types and serialization methods for client authors.

The user-facing manual is maintained separately under `docs/wiki/` in the source
repository. It covers installation, CLI commands, configuration examples, and
troubleshooting.

## A command without hardware I/O

```rust
use std::io::Cursor;
use backlightd::ipc::BacklightCommand;

let mut wire = Vec::new();
BacklightCommand::SetBrightness(60).serialize_into(&mut wire)?;
assert_eq!(wire, b"{\"type\":\"set_brightness\",\"value\":60}\n");

let command = BacklightCommand::deserialize_from(Cursor::new(&wire))?;
assert!(matches!(command, BacklightCommand::SetBrightness(60)));
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Scope and compatibility

The runtime requires Linux APIs including sysfs, `CLOCK_BOOTTIME`, timerfd,
signalfd, and Unix sockets. DDC uses `ddc-hi`; it does not depend on libddcutil.
The supplied installation integrates with systemd and udev.

The project is pre-1.0. Protocol additions and internal changes should be reviewed
for client compatibility. The wire format is newline-delimited serde JSON; the
former bincode format is not supported. Ordinary status retains the fields
`brightness_percent` and `mode`; detailed device requests add an optional `device`
field to the same response envelope.
