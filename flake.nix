{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    crane.url = "github:ipetkov/crane";
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
      rust-overlay,
      ...
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;

      mkPkgs =
        system:
        import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = mkPkgs system;
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
        in
        {
          default = pkgs.callPackage crateExpr { };
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = mkPkgs system;
          craneLib = (crane.mkLib pkgs).overrideToolchain (p: p.rust-bin.nightly.latest.default);
        in
        {
          default = craneLib.devShell {
            packages = with pkgs; [
              openssl
              pkg-config
              cargo-insta
              llvmPackages_latest.llvm
              llvmPackages_latest.bintools
              zlib.out
              llvmPackages_latest.lld
              (rust-bin.nightly.latest.default.override {
                extensions = [
                  "rust-src"
                  "rust-analyzer"
                ];
                targets = [ "wasm32-unknown-unknown" ];
              })
              eza
              fd
              ripgrep
            ];

            shellHook = ''
              alias ls=eza
              alias find=fd
              alias grep=ripgrep
            '';
          };
        }
      );
    };
}
