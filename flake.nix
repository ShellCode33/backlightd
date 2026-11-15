{
  description = "Linux brightness daemon, CLI, and NixOS module";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      eachSystem = nixpkgs.lib.genAttrs systems;
    in {
      packages = eachSystem (system:
        let pkgs = import nixpkgs { inherit system; };
        in { default = pkgs.callPackage ./nix/package.nix { }; });
      nixosModules.default = import ./nix/module.nix;
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
                inherit package;
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
