{
  description = "Prebuilt andy binaries";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs = { nixpkgs, ... }:
    let
      base = "https://github.com/maan2003/andy/releases/download/v0.2.0";
      pkg = system: name: sha256:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        pkgs.stdenv.mkDerivation {
          pname = "andy";
          version = "0.2.0";
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
        x86_64-linux.default = pkg "x86_64-linux" "andy-x86_64-unknown-linux-musl" "18312fe3ecc4323e9081d252ecaa6b462a0c6f70fc869b6779ac024bc46edef0";
        aarch64-linux.default = pkg "aarch64-linux" "andy-aarch64-unknown-linux-musl" "a344cdc7ca57fdee61fb4fe52142c6d056fffae35f242782d3ac8f273a424a26";
        x86_64-darwin.default = pkg "x86_64-darwin" "andy-x86_64-apple-darwin" "8052c23ef2f2f88c6abb6cbd16674bdcb8f7f9e445f7e354d24decdcbd212594";
        aarch64-darwin.default = pkg "aarch64-darwin" "andy-aarch64-apple-darwin" "c1436d7201a289edceb63f2c02d0f3ba2fb451ac9585db1557dad28732157dd9";
      };
    };
}
