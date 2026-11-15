# Development and verification

## Workspace layout

- `backlightd`: executable implementation, this documentation library target,
  controller, hardware adapters, scheduling, and configuration.
- `backlightctl`: command-line client and JSON/text presentation.
- `backlight_ipc`: shared wire types and bounded serialization, re-exported as
  [`crate::ipc`].
- `etc/`: installed configuration tree mirrored in the repository.
- `docs/wiki/`: user manual pages intended for a GitHub wiki.

Executable source files are `main.rs`, `controller.rs`, `monitors.rs`, `sysfs.rs`,
`ddc.rs`, `calibration.rs`, `config.rs`, `auto.rs`, `location.rs`, `socket.rs`,
`shutdown.rs`, and `wake.rs`. Generate private-item documentation locally to browse
the internal types and functions; their visibility is intentionally not a public
library commitment.

## Build commands

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --release --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo test --workspace --doc --locked
```

Add `--offline` when dependencies are cached. Linux builds need a C toolchain,
pkg-config, and libudev development headers. The declared minimum Rust version is
1.86. System integration uses Linux-specific APIs; docs.rs builds should target
`x86_64-unknown-linux-gnu`.

## Regression strategy

Use fake devices for partial discovery, write retry, mode ordering, inheritance,
and calibration. Temporary directories model sysfs, including absent attributes,
invalid maxima, large raw values, and rejected writes. Unix socket tests cover
ownership races, stale sockets, framing, and bounded clients. Signal/reload tests
run in isolated child processes with fake controllers and inherited signal masks.

Schedule tests should also run under different `TZ` values, including
`America/New_York`, `Europe/Berlin`, `Pacific/Apia`, and `Pacific/Kiritimati`, to
exercise DST, skipped dates, overnight transitions, and extreme offsets.

Never start the real daemon or mutate live displays merely to build documentation
or run ordinary tests. Installation writes system files and restarts the service;
use staged files and dry runs for packaging checks. Hardware verification requires
a separately coordinated user session.

## Documentation targets

`cargo doc -p backlightd --lib --no-deps` builds the technical guide.
`cargo doc -p backlight_ipc --no-deps` builds the protocol reference.
`cargo rustdoc -p backlightd --bin backlightd --target-dir target/internal-doc
--locked -- -D warnings --document-private-items` explicitly selects executable
internals locally, with separate output so it does not replace the library guide.
Cargo may emit its known same-name library/binary output-collision warning for
this explicit runtime build. The selected binary documentation is generated in
`target/internal-doc`; keeping that directory separate preserves the public guide. Rustdoc examples use in-memory messages, not hardware.

The Markdown guides are included from inside the package so they remain available
in a published crate. Publication itself still requires release metadata and
versioned workspace dependencies; document generation does not publish anything.
