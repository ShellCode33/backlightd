# Maintaining documentation

There are two audiences:

- `backlightd/src/docs/` and Rust doc comments: code-focused rustdoc guides and API
  documentation. `backlightd/src/lib.rs` supplies a lightweight documentation target
  and re-exports `backlight_ipc`; daemon internals remain executable implementation.
- `docs/wiki/`: user-facing GitHub wiki pages with wiki links and `_Sidebar.md`.

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
--list`. Workspace-level user wiki files are maintained independently of crate
publication. Do not expose internal daemon types merely to create API pages.

## Publish the GitHub wiki later

Enable the wiki on the GitHub repository, then clone its separate repository:

```sh
git clone https://github.com/OWNER/REPOSITORY.wiki.git /path/to/backlightd-wiki
cp docs/wiki/*.md /path/to/backlightd-wiki/
```

Replace the placeholders with the actual repository and local path. Review the
copied diff in the wiki checkout before committing and pushing it. Do not remove
unrelated wiki pages. The copy includes `Home.md` and `_Sidebar.md`; page links use
GitHub's `[[Page-name]]` syntax. No wiki push is automated by this project.

## Keep the documentation accurate

Update the configuration reference when adding settings, and CLI pages when
changing flags or output fields. Preserve the distinction between logical targets,
representable values, and verified readback. Describe limitations and actual
behavior rather than proposed features. Runnable Rust examples must not access
live monitors. Check wiki links and fenced TOML/JSON examples when editing pages.
