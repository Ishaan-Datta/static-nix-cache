{
  description = "Publish Nix binary caches to GitHub Releases and static hosting";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    fenix.url = "github:nix-community/fenix";
    fenix.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      fenix,
      ...
    }:
    let
      rev = self.shortRev or self.dirtyShortRev or "dirty";
    in
    {
      overlays.default = final: _: {
        static-nix-cache = final.callPackage ./package.nix { inherit rev; };
      };

      lib.substituterConfig =
        { url, publicKey ? null }:
        {
          extra-substituters = [ url ];
          extra-trusted-public-keys = nixpkgs.lib.optional (publicKey != null) publicKey;
        };
    }
    // flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        fenixPkgs = fenix.packages.${system};

        rustComponents = [
          fenixPkgs.stable.rustc
          fenixPkgs.stable.cargo
          fenixPkgs.stable.clippy
          fenixPkgs.default.rustfmt
          fenixPkgs.stable.rust-src
          fenixPkgs.stable.rust-std
        ];
        rustToolchain = fenixPkgs.combine rustComponents;
        releaseRustToolchain = fenixPkgs.combine (
          rustComponents
          ++ nixpkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [
            fenixPkgs.targets.x86_64-unknown-linux-musl.stable.rust-std
            fenixPkgs.targets.aarch64-unknown-linux-musl.stable.rust-std
          ]
        );

        staticNixCache = pkgs.callPackage ./package.nix { inherit rev; };

        devShell = pkgs.mkShell {
          RUSTC_WRAPPER = "${pkgs.sccache}/bin/sccache";

          buildInputs = [
            rustToolchain
            fenixPkgs.stable.rust-analyzer
            pkgs.sccache
            pkgs.cargo-nextest
            pkgs.taplo
            staticNixCache
          ];
        };

        releaseDevShell = pkgs.mkShell {
          buildInputs = [ releaseRustToolchain ]
          ++ nixpkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [
            pkgs.cargo-zigbuild
            pkgs.zig
          ];
        };
      in
      {
        devShells.default = devShell;
        devShells.release = releaseDevShell;

        packages = {
          static-nix-cache = staticNixCache;
          default = staticNixCache;
        };
      }
    );
}
