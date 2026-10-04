{ rustPlatform
, lib
, ...
}:

rustPlatform.buildRustPackage {
  pname = "tican-cli";
  version = "0.3.3";

  cargoLock.lockFile = ./Cargo.lock;

  src = lib.cleanSource ./.;
}
