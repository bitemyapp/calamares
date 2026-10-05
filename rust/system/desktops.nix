# SPDX-License-Identifier: GPL-3.0-or-later
# Desktop sessions chosen in the installer.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.calamares;
  # Login session name for each installer choice.
  sessions = {
    plasma = "plasma";
    gnome = "gnome";
    xfce = "xfce";
    cinnamon = "cinnamon";
    mate = "mate";
    lxqt = "lxqt";
    # uwsm starts graphical-session.target, which Hyprland alone never does.
    hyprland = "hyprland-uwsm";
    omarchy = "omarchy";
  };
  desktop = lib.types.enum (builtins.attrNames sessions);
  has = name: builtins.elem name cfg.desktops;
in
{
  imports = [
    ./hyprland.nix
    ./omarchy
  ];
  options.calamares = {
    desktops = lib.mkOption {
      type = lib.types.listOf desktop;
      default = [ "plasma" ];
      description = "Desktop sessions selected in the graphical installer.";
    };
    defaultDesktop = lib.mkOption {
      type = desktop;
      default = "plasma";
      description = "Default login session selected in the graphical installer.";
    };
  };
  config = {
    assertions = [
      {
        assertion = cfg.desktops != [ ] && lib.allUnique cfg.desktops;
        message = "Select one or more distinct desktops.";
      }
      {
        assertion = has cfg.defaultDesktop;
        message = "The default desktop must be one of the selected desktops.";
      }
      {
        assertion = !(has "gnome" && has "cinnamon");
        message = "GNOME and Cinnamon cannot be combined: their NixOS modules conflict on GSettings overrides.";
      }
    ];
    services.xserver.enable = true;
    services.displayManager.gdm.enable = cfg.desktops == [ "gnome" ];
    services.displayManager.sddm.enable = cfg.desktops != [ "gnome" ];
    services.displayManager.defaultSession = sessions.${cfg.defaultDesktop};
    services.desktopManager.plasma6.enable = has "plasma";
    services.desktopManager.gnome.enable = has "gnome";
    services.xserver.desktopManager.xfce.enable = has "xfce";
    services.xserver.desktopManager.cinnamon.enable = has "cinnamon";
    services.xserver.desktopManager.mate.enable = has "mate";
    services.xserver.desktopManager.lxqt.enable = has "lxqt";
    calamares.hyprland.enable = has "hyprland";
    calamares.omarchy.enable = has "omarchy";
    # Several desktops set equally-prioritized defaults for these shared
    # helpers; resolve them explicitly. Portals remain per-session.
    programs.ssh.askPassword =
      if has "plasma" then
        "${pkgs.kdePackages.ksshaskpass}/bin/ksshaskpass"
      else
        "${pkgs.x11_ssh_askpass}/libexec/x11-ssh-askpass";
    programs.gnupg.agent.pinentryPackage = if has "plasma" then pkgs.pinentry-qt else pkgs.pinentry-gnome3;
  };
}
