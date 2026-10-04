{
  rustPlatform,
  lib,
  pkg-config,
  alsa-lib,
  libopus,
  ...
}:

rustPlatform.buildRustPackage {
  pname = "tincan-cli";
  meta.mainProgram = "tincan";
  version = "0.3.3";

  cargoLock.lockFile = ./Cargo.lock;
  src = lib.cleanSource ./.;

  nativeBuildInputs = [
    pkg-config
  ];

  buildInputs = [
    alsa-lib
    libopus
  ];
}
