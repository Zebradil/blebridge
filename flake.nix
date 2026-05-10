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
