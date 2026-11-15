{
  description = "Linux brightness daemon, CLI, and NixOS module";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
  inputs.rust-overlay = {
    url = "github:oxalica/rust-overlay";
    inputs.nixpkgs.follows = "nixpkgs";
  };
  outputs = { self, nixpkgs, rust-overlay }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      eachSystem = nixpkgs.lib.genAttrs systems;
    in {
      packages = eachSystem (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          rustPlatform = pkgs.makeRustPlatform { cargo = toolchain; rustc = toolchain; };
        in { default = pkgs.callPackage ./nix/package.nix { inherit rustPlatform; }; });
      nixosModules.default = { lib, pkgs, ... }: {
        imports = [ ./nix/module.nix ];
        services.backlightd.package = lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      };
      checks = eachSystem (system:
        let
          pkgs = import nixpkgs { inherit system; };
          package = self.packages.${system}.default;
          evaluated = nixpkgs.lib.nixosSystem {
            inherit system;
            modules = [ self.nixosModules.default {
              system.stateVersion = "26.05";
              services.backlightd = {
                enable = true;
                settings.location.source = "none";
              };
            } ];
          };
        in {
          inherit package;
          module = assert evaluated.config.systemd.services.backlightd.overrideStrategy == "asDropin";
            pkgs.runCommand "backlightd-module-check" { } ''
              ${package}/bin/backlightd --config ${evaluated.config.environment.etc."backlightd/config.toml".source} --check-config
              test -f ${package}/lib/systemd/system/backlightd.service
              test -f ${package}/lib/udev/rules.d/95-backlightd.rules
              touch $out
            '';
        });
    };
}
