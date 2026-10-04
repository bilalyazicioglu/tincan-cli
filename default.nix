{
  rustPlatform,
  lib,
  pkg-config,
  alsa-lib,
  ...
}:

rustPlatform.buildRustPackage {
  pname = "tican-cli";
  version = "0.3.3";

  cargoLock.lockFile = ./Cargo.lock;
  src = lib.cleanSource ./.;

  nativeBuildInputs = [
    pkg-config
  ];

  buildInputs = [
    alsa-lib
  ];
}
