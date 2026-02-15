{
  description = "Android coordinator dev environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    flake-utils.url = "github:numtide/flake-utils";
    flakebox.url = "github:rustshop/flakebox";
  };

  outputs = { self, nixpkgs, flake-utils, flakebox }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        flakeboxLib = flakebox.lib.mkLib pkgs {
          config = {
            github.ci.enable = false;
            flakebox.init.enable = false;
            flakebox.lint.enable = false;
          };
        };

        stdTargets = flakeboxLib.mkStdTargets {};

        targets = {
          default = stdTargets.default;
          x86_64-android = stdTargets.x86_64-android;
          aarch64-android = stdTargets.aarch64-android;
          # zig-based cross targets (linker provided by cargo-zigbuild)
          aarch64-darwin = _: {
            args = {};
            componentTargets = [ "aarch64-apple-darwin" ];
          };
          x86_64-darwin = _: {
            args = {};
            componentTargets = [ "x86_64-apple-darwin" ];
          };
        };

        src = flakeboxLib.filterSubPaths {
          root = self;
          paths = [
            "Cargo.toml"
            "Cargo.lock"
            ".cargo"
            "device"
            "andy-cli"
            "md/SKILL.md"
          ];
        };

        androidSdk = flakeboxLib.android-nixpkgs.sdk.${system} (sdkPkgs: with sdkPkgs; [
          cmdline-tools-latest
          build-tools-32-0-0
          platforms-android-32
        ]);
        androidSdkRoot = "${androidSdk}/share/android-sdk";

        fenixPkgs = flakebox.inputs.fenix.packages.${system};
        muslToolchain = fenixPkgs.combine [
          (fenixPkgs.stable.withComponents flakeboxLib.config.toolchain.components)
          fenixPkgs.targets.x86_64-unknown-linux-musl.stable.rust-std
          fenixPkgs.targets.aarch64-unknown-linux-musl.stable.rust-std
          fenixPkgs.targets.aarch64-apple-darwin.stable.rust-std
          fenixPkgs.targets.x86_64-apple-darwin.stable.rust-std
        ];
        andyCraneLib = (flakebox.inputs.crane.mkLib pkgs).overrideToolchain muslToolchain;

        zigDarwinSysroot = "${pkgs.zig}/lib/zig/libc/darwin";

        # Linker wrapper: uses rust-lld (deterministic) instead of zig's wild linker (random UUIDs).
        # cargo-zigbuild still handles CC for -sys crates; only the linker is overridden.
        mkLldDarwinLinker = target: let
          target_underscores = builtins.replaceStrings ["-"] ["_"] target;
          rustLld = "${muslToolchain}/lib/rustlib/x86_64-unknown-linux-gnu/bin/rust-lld";
        in pkgs.writeShellScript "lld-darwin-cc" ''
          args=""
          for arg in "$@"; do
              case "$arg" in
                  -Wl,*)
                      rest="''${arg#-Wl,}"
                      old_ifs="$IFS"
                      IFS=","
                      for wlarg in $rest; do args="$args $wlarg"; done
                      IFS="$old_ifs" ;;
                  -mmacosx-version-min=*) ;;
                  -nodefaultlibs|-nostdlib) ;;
                  -liconv|-lc|-lm) ;; # provided by libSystem
                  *) args="$args $arg" ;;
              esac
          done
          exec ${rustLld} -flavor darwin \
              -L ${zigDarwinSysroot} -lSystem \
              -no_uuid \
              -platform_version macos 11.0.0 11.0.0 \
              $args
        '';

        mkAndyDarwin = target: let
          target_underscores = builtins.replaceStrings ["-"] ["_"] target;
          target_upper = pkgs.lib.toUpper (builtins.replaceStrings ["-"] ["_"] target);
          linker = mkLldDarwinLinker target;
        in andyCraneLib.buildPackage {
          pname = "andy-${target}";
          version = "0.2.0";
          inherit src;
          cargoExtraArgs = "--package andy-cli";
          strictDeps = true;
          doCheck = false;
          dontStrip = true;
          CARGO_BUILD_TARGET = target;
          cargoBuildCommand = "cargo zigbuild --release";
          cargoCheckCommand = "true"; # skip; cargo check can't use zigbuild
          nativeBuildInputs = [ pkgs.cargo-zigbuild pkgs.zig ];
          preBuild = "export HOME=$(mktemp -d)";
          SKILL_MD = "${./md/SKILL.md}";
          "CARGO_TARGET_${target_upper}_LINKER" = "${linker}";
          COORDINATOR_JAR = "${coordinator-jar}/coordinator-server.jar";
          COORDINATOR_SO_X86_64 = "${multiOutput.x86_64-android.release.coordinatorSo}/lib/libcoordinator.so";
          COORDINATOR_SO_AARCH64 = "${multiOutput.aarch64-android.release.coordinatorSo}/lib/libcoordinator.so";
        };

        multiOutput = (flakeboxLib.craneMultiBuild {}) (craneLib':
          let
            craneLib = craneLib'.overrideArgs {
              pname = "coordinator";
              version = "0.2.0";
              inherit src;
            };
          in {
            coordinatorSo = craneLib.buildPackage {
              cargoExtraArgs = "--package coordinator";
            };
          }
        );

        coordinator-jar = pkgs.stdenv.mkDerivation {
          name = "coordinator-server-jar";
          src = builtins.path { path = self + "/device/java"; name = "device-java-src"; };
          nativeBuildInputs = [ pkgs.jdk17 ];
          buildPhase = ''
            mkdir -p classes dex

            find . -name '*.java' | sort | xargs \
              javac -source 11 -target 11 \
              -cp "${androidSdkRoot}/platforms/android-32/android.jar" \
              -d classes

            find classes -name '*.class' | sort | xargs \
              "${androidSdkRoot}/build-tools/32.0.0/d8" \
              --lib "${androidSdkRoot}/platforms/android-32/android.jar" \
              --output dex

            jar --create --file coordinator-server.jar --date=1980-01-01T00:00:02Z -C dex classes.dex
          '';
          installPhase = ''
            mkdir -p $out
            cp coordinator-server.jar $out/
          '';
        };
      in
      {
        packages = rec {
          default = andy;
          andy = andyCraneLib.buildPackage {
            pname = "andy";
            version = "0.2.0";
            inherit src;
            cargoExtraArgs = "--package andy-cli";
            strictDeps = true;
            dontStrip = true;
            CARGO_BUILD_TARGET = "x86_64-unknown-linux-musl";
            CARGO_BUILD_RUSTFLAGS = "-C target-feature=+crt-static";
            SKILL_MD = "${./md/SKILL.md}";
            COORDINATOR_JAR = "${coordinator-jar}/coordinator-server.jar";
            COORDINATOR_SO_X86_64 = "${multiOutput.x86_64-android.release.coordinatorSo}/lib/libcoordinator.so";
            COORDINATOR_SO_AARCH64 = "${multiOutput.aarch64-android.release.coordinatorSo}/lib/libcoordinator.so";
          };

          andy-aarch64-linux = andyCraneLib.buildPackage {
            pname = "andy-aarch64-linux";
            version = "0.2.0";
            inherit src;
            cargoExtraArgs = "--package andy-cli";
            strictDeps = true;
            doCheck = false;
            dontStrip = true;
            CARGO_BUILD_TARGET = "aarch64-unknown-linux-musl";
            cargoBuildCommand = "cargo zigbuild --release";
            cargoCheckCommand = "true";
            nativeBuildInputs = [ pkgs.cargo-zigbuild pkgs.zig ];
            preBuild = "export HOME=$(mktemp -d)";
            SKILL_MD = "${./md/SKILL.md}";
            COORDINATOR_JAR = "${coordinator-jar}/coordinator-server.jar";
            COORDINATOR_SO_X86_64 = "${multiOutput.x86_64-android.release.coordinatorSo}/lib/libcoordinator.so";
            COORDINATOR_SO_AARCH64 = "${multiOutput.aarch64-android.release.coordinatorSo}/lib/libcoordinator.so";
          };

          andy-aarch64-darwin = mkAndyDarwin "aarch64-apple-darwin";
          andy-x86_64-darwin = mkAndyDarwin "x86_64-apple-darwin";

          coordinator-device = pkgs.runCommand "coordinator-device" {} ''
            mkdir -p $out
            cp ${multiOutput.x86_64-android.release.coordinatorSo}/lib/libcoordinator.so $out/libcoordinator-x86_64.so
            cp ${multiOutput.aarch64-android.release.coordinatorSo}/lib/libcoordinator.so $out/libcoordinator-aarch64.so
            cp ${coordinator-jar}/coordinator-server.jar $out/
          '';

          andy-archive = pkgs.runCommand "andy-archive" { nativeBuildInputs = [ pkgs.xz ]; } ''
            mkdir -p $out
            ${pkgs.lib.concatStringsSep "\n" (map ({ drv, name }: ''
              tar --sort=name --mtime='1980-01-01' --owner=0 --group=0 --numeric-owner -cJf $out/${name}.tar.xz -C ${drv} .
            '') [
              { drv = andy; name = "andy-x86_64-unknown-linux-musl"; }
              { drv = andy-aarch64-linux; name = "andy-aarch64-unknown-linux-musl"; }
              { drv = andy-aarch64-darwin; name = "andy-aarch64-apple-darwin"; }
              { drv = andy-x86_64-darwin; name = "andy-x86_64-apple-darwin"; }
            ])}
          '';
        };

        devShells = flakeboxLib.mkShells {
          inherit targets;
          crossTargets = targets;

          packages = [
            pkgs.jdk17
            pkgs.android-tools
            pkgs.jq
            pkgs.bun
            pkgs.cargo-zigbuild
            pkgs.zig
          ];

          shellHook = ''
            export ANDY_PACKAGE="com.fedi"
          '';
        };
      }
    );
}
