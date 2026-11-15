# Nix and NixOS

The repository includes a locked flake for `x86_64-linux` and `aarch64-linux`.
Use a released version tag to pin the application version. Replace `v0.1.0` in
these examples with the desired release.

## CLI installation

```sh
nix profile install github:ShellCode33/backlightd/v0.1.0
```

This provides both commands. It does not activate the system daemon, install udev
rules, or create mutable system configuration. For NixOS use the module below.

## NixOS module

Add the input and module to your system flake:

```nix
{
  inputs.backlightd.url = "github:ShellCode33/backlightd/v0.1.0";

  # Within the modules list of your existing nixosSystem:
  # modules = [ backlightd.nixosModules.default ./configuration.nix ];
}
```

The following configuration enables the daemon, integrates its udev rule, and
writes its configuration declaratively:

```nix
{ ... }: {
  services.backlightd = {
    enable = true;
    settings = {
      location.source = "none";
      manual.override_duration = "12h";
      logging.level = "info";
    };
  };
}
```

See [[Configuration]] for supported settings. Omitted keys use the daemon's
built-in defaults. Edit `services.backlightd.settings` and rebuild your system
instead of editing `/etc/backlightd/config.toml`. The module reuses the packaged
systemd unit and its hardening settings. Configuration changes restart the daemon,
which starts in auto mode. `services.backlightd.package` can override the package.

To build and check the package/module from a checkout:

```sh
nix build
nix flake check
```

Nix builds from the locked Rust dependencies and native system inputs. GitHub
releases provide the versioned source and flake; there is no separate binary
cache maintained by this project.
