{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    crane.url = "github:ipetkov/crane";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
      flake-utils,
      rust-overlay,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        craneLib = (crane.mkLib pkgs).overrideToolchain (p: p.rust-bin.nightly.latest.default);

        crateExpr =
          {
            perl,
            openssl,
            libiconv,
            lib,
            pkg-config,
            stdenv,
          }:
          craneLib.buildPackage {
            src = craneLib.cleanCargoSource ./.;
            strictDeps = true;

            doCheck = false;

            nativeBuildInputs = [
              perl
              pkg-config
            ]
            ++ lib.optionals stdenv.buildPlatform.isDarwin [
              libiconv
            ];

            buildInputs = [
              openssl
            ];
          };

        cargo-leptos = pkgs.callPackage crateExpr { };
      in
      {
        packages.default = cargo-leptos;

        devShells.default = craneLib.devShell {
          packages = with pkgs; [
            openssl
            pkg-config
            cargo-insta
            llvmPackages_latest.llvm
            llvmPackages_latest.bintools
            zlib.out
            llvmPackages_latest.lld
            (rust-bin.stable.latest.default.override {
              extensions= [ "rust-src" "rust-analyzer" ];
              targets = [ "wasm32-unknown-unknown" ];
            })
	    eza
	    fd
	    ripgrep
          ];

          shellHook = ''
            alias ls=exa
            alias find=fd
            alias grep=ripgrep
            '';
        };
      }
    );
}
