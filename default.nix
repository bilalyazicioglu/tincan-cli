{
  rustPlatform,
  lib,
  stdenv,
  pkg-config,
  alsa-lib,
  libopus,
  ...
}:

rustPlatform.buildRustPackage {
  pname = "tincan-cli";
  version = (lib.importTOML ./Cargo.toml).package.version;

  cargoLock.lockFile = ./Cargo.lock;
  src = lib.cleanSource ./.;

  nativeBuildInputs = [
    pkg-config
  ];

  buildInputs = [ libopus ] ++ lib.optionals stdenv.hostPlatform.isLinux [ alsa-lib ];

  meta = {
    description = "Serverless peer-to-peer voice and text chat for your terminal";
    homepage = "https://tincan.rs";
    license = lib.licenses.mit;
    mainProgram = "tincan";
  };
}
