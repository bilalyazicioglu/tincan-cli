{
  description = "Serverless peer-to-peer voice and text chat for your terminal.";
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

  };
  outputs = { nixpkgs, ... }:
    let
      supportedSys = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forallSys = nixpkgs.lib.genAttrs supportedSys;
      pkgs = nixpkgs.legacyPackages;

    in
    {
      packages = forallSys (sys: {

        default = pkgs.${sys}.callPackage ./. { };

      });

      overlays.default = final: prev: {
        tincan-cli = final.callPackage ./. { };
      };

    };

}
