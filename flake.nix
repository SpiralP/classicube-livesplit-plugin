{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    flake-utils.url = "github:SpiralP/nix-flake-utils";
  };

  outputs = inputs@{ flake-utils, ... }:
    flake-utils.lib.makeOutputs inputs
      ({ lib, pkgs, makeRustPackage, dev, ... }:
        let
          args = {
            src = ./.;

            nativeBuildInputs = with pkgs; [
              pkg-config
              rustPlatform.bindgenHook
            ];

            useNextest = true;
          };
        in
        {
          default = makeRustPackage pkgs (self: args);
          debug = makeRustPackage pkgs (self: args // {
            buildType = "debug";
            hardeningDisable = [ "all" ];
          });
        });
}
