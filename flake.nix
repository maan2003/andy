{
  description = "Prebuilt andy binaries";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs = { nixpkgs, ... }:
    let
      base = "https://github.com/maan2003/andy/releases/download/v0.5.0";
      pkg = system: name: sha256:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        pkgs.stdenv.mkDerivation {
          pname = "andy";
          version = "0.5.0";
          src = pkgs.fetchurl {
            url = "${base}/${name}.tar.xz";
            inherit sha256;
          };
          sourceRoot = ".";
          installPhase = ''
            mkdir -p $out
            mv bin $out/bin
          '';
          meta.mainProgram = "andy";
        };
    in
    {
      packages = {
        x86_64-linux.default = pkg "x86_64-linux" "andy-x86_64-unknown-linux-musl" "496a79708e658baddae25ff49da3c7890490d6f3b062487bd1abae40d876a65b";
        aarch64-linux.default = pkg "aarch64-linux" "andy-aarch64-unknown-linux-musl" "fd23f7fea4c018985feaea980bed6dd0ffe060fd093d69123c5a8b692f6acab3";
        x86_64-darwin.default = pkg "x86_64-darwin" "andy-x86_64-apple-darwin" "45c39b5d5ef7936bed585a50e96f8b8be574945ed9a9898af40dae5c87e4d97b";
        aarch64-darwin.default = pkg "aarch64-darwin" "andy-aarch64-apple-darwin" "c08c2063346657644ed5f71a1166e4377447024a4f3a2477bd6d125a3eb41e14";
      };
    };
}
