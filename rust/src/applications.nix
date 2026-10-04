# SPDX-License-Identifier: GPL-3.0-or-later
# Copied verbatim into /etc/nixos; all sources come from the installed flake.lock.
{
  config,
  lib,
  pkgs,
  applicationPkgs ? pkgs,
  aiPackages ? { },
  ompPackage ? null,
  ...
}:
let
  catalog = builtins.fromJSON (builtins.readFile ./applications.json);
  ids = config.calamares.applications;
  has = id: builtins.elem id ids;
  selected = builtins.filter (app: has app.id) catalog;
  packagesFor =
    app:
    if (app.source or "nixpkgs") == "omp" then
      [ ompPackage ]
    else
      map (
        name: (if (app.source or "nixpkgs") == "ai" then aiPackages else applicationPkgs).${name}
      ) app.packages;
  packages = lib.unique (lib.concatMap packagesFor selected);
  launchers = map (
    app:
    pkgs.makeDesktopItem {
      name = "calamares-${app.id}";
      desktopName = app.name;
      comment = app.description;
      exec = app.terminal;
      terminal = true;
      icon = "utilities-terminal";
      categories = [
        "Development"
        "ConsoleOnly"
      ];
    }
  ) (builtins.filter (app: app ? terminal && app.id != "neovim") selected);
in
{
  options.calamares = {
    applications = lib.mkOption {
      type = lib.types.listOf (lib.types.enum (map (app: app.id) catalog));
      default = [ "firefox" ];
      description = "Applications selected in the graphical installer.";
    };
    installUser = lib.mkOption {
      type = lib.types.str;
      description = "User created by the graphical installer.";
    };
  };
  config = {
    assertions = [
      {
        assertion = lib.length ids == lib.length (lib.unique ids);
        message = "Duplicate installer applications.";
      }
      {
        assertion = !has "rustup" || has "build-tools";
        message = "Rustup requires Development build tools.";
      }
      {
        assertion =
          (config.nixpkgs.config.allowUnfree or false) || builtins.all (app: !(app.unfree or false)) selected;
        message = "Selected applications require allowing proprietary software.";
      }
    ];
    environment.systemPackages = packages ++ launchers;
    environment.etc."installer-applications.json".text = builtins.toJSON {
      selected = ids;
      packages = map (package: {
        inherit (package) name;
        path = package.outPath;
        version = package.version or null;
      }) packages;
    };
    # A GC root for the exact selected packages is built BEFORE any disk write.
    system.build.installerApplications = pkgs.linkFarm "installer-applications" (
      lib.imap0 (i: package: {
        name = "${toString i}-${lib.getName package}";
        path = package;
      }) (packages ++ launchers)
    );
    programs.firefox = lib.mkIf (has "firefox") {
      enable = true;
      package = applicationPkgs.firefox;
    };
    programs.steam = lib.mkIf (has "steam") {
      enable = true;
      package = applicationPkgs.steam;
    };
    virtualisation.docker = lib.mkIf (has "docker") {
      package = applicationPkgs.docker;
      rootless = {
        enable = true;
        setSocketVariable = true;
      };
    };
    users.users.${config.calamares.installUser}.linger = lib.mkIf (has "docker") true;
  };
}
