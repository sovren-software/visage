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
            # Toolchain extras that match the CI gates
            # (`cargo fmt --check`, `cargo clippy -- -D warnings`).
            # `inputsFrom = [ visage ]` brings the compiler but not these.
            rustfmt
            clippy
            # `bindgen` (transitively via `v4l2-sys-mit`) needs libclang.so
            # at build time; without LIBCLANG_PATH set, `cargo build -p
            # visaged` in the devshell fails with "Unable to find libclang".
            llvmPackages.libclang
            rust-analyzer
            cargo-deb
            cargo-watch
          ];
          # Tell bindgen where to find libclang at build time.
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
        };
      }
    ) // {
      # NixOS module export
      nixosModules.default = nixosModule;
      nixosModules.visage = nixosModule;
    };
}
