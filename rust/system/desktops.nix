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
    # Plasma's module turns on SDDM's experimental Wayland greeter. Its
    # hand-off to the user session races kwallet-pam (sddm/sddm#1443): a login
    # can stall for 30 s and leave the session inactive on a black VT. The X11
    # greeter is SDDM's default and what the other desktops already use.
    services.displayManager.sddm.wayland.enable = false;
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
    programs.gnupg.agent.pinentryPackage =
      if has "plasma" then pkgs.pinentry-qt else pkgs.pinentry-gnome3;
    # Xfce installs polkit-gnome system-wide, and its autostart entry has no
    # OnlyShowIn, so it also starts in Plasma, Hyprland and the others and
    # races their own authentication agents. /etc/xdg is searched first.
    environment.etc."xdg/autostart/polkit-gnome-authentication-agent-1.desktop" =
      lib.mkIf (has "xfce")
        {
          text = ''
            [Desktop Entry]
            Type=Application
            Name=PolicyKit Authentication Agent
            Exec=${pkgs.polkit_gnome}/libexec/polkit-gnome-authentication-agent-1
            NoDisplay=true
            OnlyShowIn=XFCE;
          '';
        };
    # GNOME enables IBus, whose autostart entry skips only GNOME and KDE. In
    # the Hyprland sessions it only reports that it should run from GNOME.
    environment.etc."xdg/autostart/ibus-daemon.desktop" =
      lib.mkIf
        (
          (has "hyprland" || has "omarchy")
          && config.i18n.inputMethod.enable
          && config.i18n.inputMethod.type == "ibus"
        )
        {
          text = ''
            [Desktop Entry]
            Type=Application
            Name=IBus
            Exec=${config.i18n.inputMethod.package}/bin/ibus-daemon --daemonize --xim
            NotShowIn=GNOME;KDE;Hyprland;
          '';
        };
  };
}
