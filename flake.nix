{
  description = "blebridge development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    { nixpkgs, crane, ... }:
    let
      supportedSystems = [
        "aarch64-darwin"
        "x86_64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
    in
    {
      # Static-musl cross builds via nixpkgs' pkgsCross -- no docker, no rustup.
      # This is the DEFAULT local build path (`mise run build-rust`), because on
      # a nix-managed host `cross` leaks the devshell's nix compiler env into its
      # build container and breaks. Build a binary directly with e.g.
      #   nix build .#blebridge-arm64 -L
      packages.x86_64-linux =
        let
          pkgs = import nixpkgs { system = "x86_64-linux"; };
          lib = pkgs.lib;
          # Only the files that affect the build -- otherwise any tracked repo
          # change (a new unrelated file) bumps the derivation hash and forces
          # a full recompile.
          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./src
            ];
          };
          # crane splits dependency compilation into its own cached derivation
          # (cargoArtifacts), so editing src/ no longer recompiles every dep.
          # Vendored libusb1-sys/libdbus-sys build C from source under the cross
          # toolchain; that C friction is exactly what `cross`'s image hides.
          mkBridge =
            crossPkgs:
            let
              cc = crossPkgs.stdenv.cc;
              prefix = cc.targetPrefix; # e.g. "aarch64-unknown-linux-musl-"
              rustTarget = lib.removeSuffix "-" prefix;
              underscored = builtins.replaceStrings [ "-" ] [ "_" ] rustTarget;
              upper = lib.toUpper underscored;
              crossCc = "${cc}/bin/${prefix}cc";
              # pkgsBuildHost = tools that run on the x86_64 build host and
              # target the cross platform, shipping that target's std. (Plain
              # crossPkgs.rustc splices to the target-native rustc and triggers
              # a from-source rebuild.) crane needs cargo+rustc in one dir.
              toolchain = pkgs.symlinkJoin {
                name = "rust-cross-${rustTarget}";
                paths = [
                  crossPkgs.pkgsBuildHost.rustc
                  crossPkgs.pkgsBuildHost.cargo
                ];
              };
              craneLib = (crane.mkLib pkgs).overrideToolchain (_: toolchain);
              commonArgs = {
                inherit src;
                pname = "blebridge";
                version = "0.1.0";
                strictDeps = true;
                doCheck = false;
                # Wiring buildRustPackage's setup hooks gave cargo for free.
                CARGO_BUILD_TARGET = rustTarget;
                "CARGO_TARGET_${upper}_LINKER" = crossCc;
                "CC_${underscored}" = crossCc; # for the cc crate (vendored C)
                HOST_CC = "${pkgs.stdenv.cc}/bin/cc";
                # Static musl, matching the cross/CI build. -lgcc resolves the
                # __aarch64_ldadd4_sync atomics helper libdbus' vendored C needs
                # but the -nodefaultlibs static-musl link otherwise drops.
                RUSTFLAGS = "-C target-feature=+crt-static -C link-arg=-lgcc";
                # libdbus-sys vendored build shells out to pkg-config/autotools.
                nativeBuildInputs = [
                  pkgs.pkg-config
                  cc
                ];
              };
            in
            craneLib.buildPackage (commonArgs // {
              cargoArtifacts = craneLib.buildDepsOnly commonArgs;
            });
        in
        {
          blebridge-arm64 = mkBridge pkgs.pkgsCross.aarch64-multiplatform-musl;
          blebridge-amd64 = mkBridge pkgs.pkgsCross.musl64;
        };

      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          
          # Create a wrapper for uv to run in an FHS environment so it can download and run binary wheels like ninja
          uv-fhs = pkgs.writeShellScriptBin "uv-fhs" ''
            exec ${pkgs.buildFHSEnv {
              name = "uv-fhs-env";
              targetPkgs = pkgs: with pkgs; [ 
                zlib.dev openssl.dev readline.dev sqlite.dev libffi.dev xz.dev bzip2.dev gcc gnumake pkg-config 
                glib.dev gobject-introspection.dev dbus.dev cairo.dev ninja meson 
                libxcb.dev libx11.dev libxext.dev libxrender.dev xorgproto 
              ];
              runScript = "uv";
            }}/bin/uv-fhs-env "$@"
          '';
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              mise
              python314
              uv-fhs
              # `cross` binary for the parity task `mise run build-rust-cross`.
              # NOTE: cross fights this nix-managed host (leaked devshell compiler
              # env + nix-wrapped rustup), so run that task from a CLEAN shell with
              # a vanilla rustup toolchain. The DEFAULT `build-rust` uses nix
              # pkgsCross instead (packages.*.blebridge-<arch>) and needs none of this.
              cargo-cross
            ];
            
            shellHook = ''
              export GI_TYPELIB_PATH="${pkgs.glib.out}/lib/girepository-1.0:${pkgs.gobject-introspection.out}/lib/girepository-1.0:$GI_TYPELIB_PATH"
              mise install || true
              source <(mise activate)
            '';
          };
        }
      );
    };
}
