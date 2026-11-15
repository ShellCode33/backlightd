{ config, lib, pkgs, ... }:
let
  cfg = config.services.backlightd;
  format = pkgs.formats.toml { };
in {
  options.services.backlightd = {
    enable = lib.mkEnableOption "backlightd brightness management";
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./package.nix { };
      description = "The backlightd package to run.";
    };
    settings = lib.mkOption {
      type = format.type;
      default = { };
      description = "Settings written to /etc/backlightd/config.toml; omitted keys use daemon defaults.";
    };
  };
  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
    environment.etc."backlightd/config.toml".source = format.generate "backlightd.toml" cfg.settings;
    services.udev.packages = [ cfg.package ];
    systemd.packages = [ cfg.package ];
    # Reuse the packaged unit, including hardening, instead of duplicating it.
    systemd.services.backlightd = {
      overrideStrategy = "asDropin";
      wantedBy = [ "multi-user.target" ];
      restartTriggers = [ config.environment.etc."backlightd/config.toml".source ];
    };
  };
}
