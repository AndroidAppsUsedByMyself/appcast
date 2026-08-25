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
      forAllSystems = f: nixpkgs.lib.genAttrs systems f;
      linuxSystems = [
        "x86_64-linux"
        "aarch64-linux"
      ];

      # The CLI itself. Workspace-aware: builds only the root `appcast`
      # package, so plugin members (and their WebKitGTK stack) stay out.
      mkAppcast =
        pkgs:
        pkgs.rustPlatform.buildRustPackage {
          pname = "appcast";
          version = "0.1.4";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          meta = with pkgs.lib; {
            description = "Cast a remote/local app's screen into a native window (adb/scrcpy)";
            longDescription = ''
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

      # The embedded-WebView transporter plugin. Built from the same source
      # tree (`buildAndTestSubdir` keeps the relative SDK path dep intact)
      # and linked against the Nix-provided WebKitGTK stack, so the
      # resulting .so carries store rpaths and dlopen-resolves anywhere.
      mkWebviewPlugin =
        pkgs:
        pkgs.rustPlatform.buildRustPackage {
          pname = "appcast-tpt-webview";
          version = "0.1.0";
          src = ./.;
          buildAndTestSubdir = "plugins/webview";
          cargoLock.lockFile = ./Cargo.lock;
          doCheck = false; # cdylib; smoke-tested at runtime, not in checkPhase

          nativeBuildInputs = with pkgs; [ pkg-config ];
          buildInputs =
            with pkgs;
            lib.optionals stdenv.isLinux [
              dbus
              glib
              gtk3
              libsoup_3
              webkitgtk_4_1
            ];

          postInstall = ''
            mkdir -p $out/lib
            find . -name 'libappcast_tpt_webview.so' -exec cp {} $out/lib/ \;
          '';

          meta = with pkgs.lib; {
            description = "appcast transporter plugin: embedded WebView window via wry";
            license = licenses.asl20;
            platforms = platforms.linux;
          };
        };
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = mkAppcast pkgs;
        }
        // nixpkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
          webview-plugin = mkWebviewPlugin pkgs;
        }
      );

      # Static name set on purpose: any construction-time conditional over
      # `final` (even just final.system) recurses inside the pkgs fixpoint
      # on current nixpkgs. Values stay lazy; misuse on non-Linux fails
      # with a clear platform error only when actually built.
      overlays.default =
        final: _prev: {
          appcast = mkAppcast final;
          appcast-tpt-webview = mkWebviewPlugin final;
        };

      nixosModules.default = import ./nixos/module.nix;

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
          # For building the web-webview plugin (wry/tao need the WebKitGTK
          # stack). Linux-only extras on top of the default shell, plus an
          # install-plugin helper for the edit→build→deploy loop.
          plugin = pkgs.mkShell {
            buildInputs =
              with pkgs;
              [
                cargo
                rustc
                clippy
                pkg-config
                dbus
                glib
                gtk3
                libsoup_3
                webkitgtk_4_1
              ]
              ++ lib.optionals stdenv.isLinux [
                udev
                alsa-lib
              ];
            shellHook = ''
              install-plugin() {
                set -e
                cargo build --release -p appcast_tpt_webview
                mkdir -p ~/.config/appcast/transporters
                cp -f target/release/libappcast_tpt_webview.so \
                  ~/.config/appcast/transporters/
                echo "installed → ~/.config/appcast/transporters/"
              }
              echo "appcast plugin dev shell — iterate: cargo build -p appcast_tpt_webview && install-plugin"
            '';
          };
        }
      );
    };
}
