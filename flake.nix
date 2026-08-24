{
  description = "appcast: cast a single app's screen into a native window on this desktop";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/e7a3ca8092b61ff85b6a45bf863ea2b2d6a661b3";
  };

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "appcast";
            version = "0.1.1";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            meta = with pkgs.lib; {
              description = "Cast a remote/local app's screen into a native window (adb/scrcpy)";              longDescription = ''
                appcast "flows" the GUI of a single application on a local or
                remote device into its own window on this desktop.

                - appcast run adb <serial> <package>: launch an Android app on
                  a free virtual display via adb, then mirror it locally with
                  scrcpy.
                - Profiles under $XDG_CONFIG_HOME/appcast/profiles/*.yaml store
                  parameter bundles; every field can be overridden from the CLI.
                - snapshot prints the fully merged command line without running.
              '';
              homepage = "https://github.com/AndroidAppsUsedByMyself/appcast";
              license = licenses.asl20;
              platforms = platforms.unix;
              mainProgram = "appcast";
            };
          };
        }
      );

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/appcast";
        };
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = pkgs.mkShell {
            buildInputs = with pkgs; [
              cargo
              rustc
              clippy
              rustfmt
            ];
          };
        }
      );
    };
}
