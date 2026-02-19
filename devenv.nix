{ pkgs, ... }:

{
  cachix.enable = false;
  android.enable = true;
  packages = [
    pkgs.jdk17
    pkgs.jq
    pkgs.bun
    pkgs.cargo-zigbuild
    pkgs.zig
  ];

  languages.rust = {
    enable = true;
    channel = "stable";
    targets = [
      "x86_64-linux-android"
      "aarch64-linux-android"
      "x86_64-unknown-linux-musl"
      "aarch64-unknown-linux-musl"
    ];
  };

  env.ANDY_PACKAGE = "com.fedi";
}
