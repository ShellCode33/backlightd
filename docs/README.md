# Maintaining documentation

There are two audiences:

- `backlightd/src/docs/` and Rust doc comments: code-focused rustdoc guides and API
  documentation. `backlightd/src/lib.rs` supplies a lightweight documentation target
  and re-exports `backlight_ipc`; daemon internals remain executable implementation.
- [GitHub wiki](https://github.com/ShellCode33/backlightd/wiki): user-facing pages,
  maintained in the separate `backlightd.wiki.git` repository.

## Build and check rustdoc

```sh
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo test --workspace --doc --locked
```

Open `target/doc/backlightd/index.html` for the technical guides and
`target/doc/backlight_ipc/index.html` for the protocol reference. To include
executable internals locally, select that target explicitly and keep its output
separate from the library guide:

```sh
cargo rustdoc -p backlightd --bin backlightd --target-dir target/internal-doc \
    --locked -- -D warnings --document-private-items
```

Open `target/internal-doc/doc/backlightd/index.html` for the internal runtime.
The guide Markdown is inside the crate so it is included when the crate is packaged.
Both documented library crates specify a Linux target in docs.rs metadata.

Nothing here publishes to crates.io or docs.rs. Before a release, complete package
metadata and version the path dependencies; publish the IPC dependency before its
consumers. Confirm crate contents include the Markdown files with `cargo package
--list`. The user wiki is maintained independently of crate publication. Do not expose internal daemon types merely to create API pages.

## Maintain the GitHub wiki

Clone the wiki's separate Git repository:

```sh
git clone git@github.com:ShellCode33/backlightd.wiki.git /path/to/backlightd-wiki
```

Edit pages directly in that checkout, review the diff, then commit and push there.
The wiki owns the user manual; this repository owns the rustdoc guides. There is
no copy or synchronization step. Preserve unrelated wiki pages, `Home.md`, and
`_Sidebar.md`. Wiki links use GitHub's `[[Page-name]]` syntax. Wiki publication is
separate from application releases and is not automated by project CI.

## Keep the documentation accurate

Update the configuration reference when adding settings, and CLI pages when
changing flags or output fields. Preserve the distinction between logical targets,
representable values, and verified readback. Describe limitations and actual
behavior rather than proposed features. Runnable Rust examples must not access
live monitors. Check wiki links and fenced TOML/JSON examples when editing pages.
