# Declarative installation of appcast plus transporter plugins.
#
# Usage:
#   imports = [ inputs.appcast.nixosModules.default ];
#   nixpkgs.overlays = [ inputs.appcast.overlays.default ];
#   programs.appcast = {
#     enable = true;
#     plugins = [ pkgs.appcast-tpt-webview ];
#   };
#
# Plugin loading model: the module wires a merged read-only store directory
# of `libappcast_tpt_*.so` files into every `appcast` invocation through a
# thin wrapper. A per-user $APPCAST_TRANSPORTER_DIR keeps its meaning and is
# searched FIRST, so users can still override system plugins by name.
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.programs.appcast;

  # Store dir whose entries are symlinks to each plugin's .so files.
  # basename dedup happens naturally (symlinkJoin last-wins), matching the
  # registry's later-registration-overrides semantics.
  systemTransporters = pkgs.symlinkJoin {
    name = "appcast-transporters";
    paths = map (p: "${p}/lib") cfg.plugins;
  };

  # Resolved late (inside config) on purpose: binding pkgs.appcast in the
  # option *declaration* would force the module-argument pkgs during module
  # merging, which infinitely recurses once nixpkgs.overlays re-imports
  # nixpkgs. Null means "expect the overlay to provide pkgs.appcast".
  resolvedPackage =
    if cfg.package != null then
      cfg.package
    else
      pkgs.appcast or (throw ''
        programs.appcast: no package configured.
        Either add the flake overlay:
          nixpkgs.overlays = [ <appcast>.overlays.default ];
        or pin one explicitly:
          programs.appcast.package = <appcast>.packages.''${pkgs.system}.default;
      '');

  # Wrapper prepends any pre-existing user dirs so they keep priority over
  # the declarative set; without one, only the system dir is searched.
  wrapped =
    if cfg.plugins == [ ] then
      resolvedPackage
    else
      pkgs.writeShellScriptBin "appcast" ''
        export APPCAST_TRANSPORTER_DIR="''${APPCAST_TRANSPORTER_DIR:+''${APPCAST_TRANSPORTER_DIR}:}${systemTransporters}"
        exec ${resolvedPackage}/bin/appcast "$@"
      '';
in
{
  options.programs.appcast = {
    enable = lib.mkEnableOption "appcast, the single-app screen-casting CLI";

    package = lib.mkOption {
      type = lib.types.nullOr lib.types.package;
      default = null;
      description = ''
        The appcast package to install. Defaults to `pkgs.appcast`, which
        exists once the flake's overlay is applied; otherwise set this
        explicitly.
      '';
    };

    plugins = lib.mkOption {
      type = lib.types.listOf lib.types.package;
      default = [ ];
      example = lib.literalExpression "[ pkgs.appcast-tpt-webview ]";
      description = ''
        Transporter plugin packages to install system-wide. Each package
        must expose its libappcast_tpt_* shared library under `$out/lib`.
        Loaded on top of the built-in backends; a plugin may override a
        built-in under the same name.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ wrapped ];
  };
}
