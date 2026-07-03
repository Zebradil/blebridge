{
  description = "blebridge development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { nixpkgs, ... }:
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
          # Vendored libusb1-sys/libdbus-sys build C from source under the cross
          # toolchain; that C friction is exactly what `cross`'s image hides.
          mkBridge = crossPkgs: crossPkgs.rustPlatform.buildRustPackage {
            pname = "blebridge";
            version = "0.1.0";
            src = ./.;
            cargoLock = {
              lockFile = ./Cargo.lock;
              # ant-rs is an unpublished git dep; nix needs its content hash.
              outputHashes = {
                "ant-0.1.0" = "sha256-RrEgrBb2GZD4hSCqEQ6C9rX6Wbrx8VHOmableszPJP0=";
              };
            };
            # Static musl, matching the cross/CI build. -lgcc resolves the
            # __aarch64_ldadd4_sync atomics helper libdbus' vendored C needs but
            # the -nodefaultlibs static-musl link otherwise drops.
            env.RUSTFLAGS = "-C target-feature=+crt-static -C link-arg=-lgcc";
            # libdbus-sys vendored build shells out to pkg-config/autotools.
            nativeBuildInputs = [ pkgs.pkg-config ];
            doCheck = false;
          };
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
