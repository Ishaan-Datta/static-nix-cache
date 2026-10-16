{
  lib,
  rustPlatform,
  rev ? "dirty",
}:

let
  cargoToml = fromTOML (builtins.readFile ./Cargo.toml);
in
rustPlatform.buildRustPackage {
  pname = cargoToml.package.name;
  version = cargoToml.package.version;

  src = lib.cleanSource ./.;

  cargoLock.lockFile = ./Cargo.lock;

  passthru.revision = rev;

  meta = {
    inherit (cargoToml.package) description;
    homepage = "https://github.com/Ishaan-Datta/static-nix-cache";
    license = lib.licenses.gpl2Only;
    mainProgram = "static-nix-cache";
    platforms = lib.platforms.unix;
  };
}
