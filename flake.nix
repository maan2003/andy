{
  description = "Prebuilt andy binaries";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs = { nixpkgs, ... }:
    let
      base = "https://github.com/maan2003/andy/releases/download/v0.3.0";
      pkg = system: name: sha256:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        pkgs.stdenv.mkDerivation {
          pname = "andy";
          version = "0.3.0";
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
        x86_64-linux.default = pkg "x86_64-linux" "andy-x86_64-unknown-linux-musl" "0e8b6aafc58f147c012e66f194464660d29c95e567f94338c0a993b5b7c73357";
        aarch64-linux.default = pkg "aarch64-linux" "andy-aarch64-unknown-linux-musl" "cff3c2d42f365ad4f7eeb1fe8ca816742bec7c85b093f704c46d9fa3b7ee6cb5";
        x86_64-darwin.default = pkg "x86_64-darwin" "andy-x86_64-apple-darwin" "f57971f14dd1a17bb0b1fbfb0a4f3c67d9883834410ac68e2fa2a09196969c19";
        aarch64-darwin.default = pkg "aarch64-darwin" "andy-aarch64-apple-darwin" "e3655eb066dfa2415cdccbf3a31e7294fbdc64ddb55b3e94f26ba2f6f877d985";
      };
    };
}
