{
  description = "Visage — Linux face authentication via PAM";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    let
      # NixOS module — works on all architectures
      nixosModule = import ./packaging/nix/module.nix;
    in
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        visage = pkgs.callPackage ./packaging/nix/default.nix { };
      in
      {
        packages = {
          default = visage;
          visage = visage;
        };

        # nix develop — drop into a shell with all build dependencies
        devShells.default = pkgs.mkShell {
          inputsFrom = [ visage ];
          packages = with pkgs; [
            rust-analyzer
            cargo-deb
            cargo-watch
          ];
        };
      }
    ) // {
      # NixOS module export
      nixosModules.default = nixosModule;
      nixosModules.visage = nixosModule;
    };
}
