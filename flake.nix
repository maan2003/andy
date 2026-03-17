{
  description = "Prebuilt andy binaries";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs = { nixpkgs, ... }:
    let
      base = "https://github.com/maan2003/andy/releases/download/v0.4.0";
      pkg = system: name: sha256:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        pkgs.stdenv.mkDerivation {
          pname = "andy";
          version = "0.4.0";
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
        x86_64-linux.default = pkg "x86_64-linux" "andy-x86_64-unknown-linux-musl" "0b8e83ec45eeee7f916165c52b1cb93aca5192cb1d1a4f54ce72f70fe9d324c2";
        aarch64-linux.default = pkg "aarch64-linux" "andy-aarch64-unknown-linux-musl" "6a3c4f4bcc73efefe542741b28117fb868520dead52e4d31f948b28f1197480b";
        x86_64-darwin.default = pkg "x86_64-darwin" "andy-x86_64-apple-darwin" "2cc28c14bb2f538298f1913d36d4f2f122e90d782cef2fd163a4201381d852b5";
        aarch64-darwin.default = pkg "aarch64-darwin" "andy-aarch64-apple-darwin" "f87c6435c77533e24838c23387bc7b37d5b1c627069c42033e5ec42457a1cd34";
      };
    };
}
