{ lib, stdenv, rustPlatform, pkg-config, udev }:
rustPlatform.buildRustPackage {
  pname = "backlightd";
  version = (builtins.fromTOML (builtins.readFile ../backlightd/Cargo.toml)).package.version;
  # Explicit source set excludes local credentials, build output, and agent state.
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml ../Cargo.lock ../rust-toolchain.toml ../Makefile ../LICENSE ../README.md
      ../backlightd ../backlightctl ../backlight_ipc ../etc ../docs
    ];
  };
  cargoLock.lockFile = ../Cargo.lock;
  nativeBuildInputs = [ pkg-config ];
  buildInputs = [ udev ];
  doCheck = true;
  dontUseCargoParallelTests = true;
  installPhase = ''
    runHook preInstall
    make install BUILD_DIR=target/${stdenv.hostPlatform.rust.rustcTarget}/release PREFIX=$out INSTALL_PREFIX=$out/bin \
      CONFIG_DIR=$out/share/backlightd SYSTEMD_UNIT_DIR=$out/lib/systemd/system \
      UDEV_RULES_DIR=$out/lib/udev/rules.d DOC_DIR=$out/share/doc/backlightd
    runHook postInstall
  '';
  meta = {
    description = "Coordinated laptop and external monitor brightness";
    homepage = "https://github.com/ShellCode33/backlightd";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
    mainProgram = "backlightctl";
  };
}
