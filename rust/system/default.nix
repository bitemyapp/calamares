# SPDX-License-Identifier: GPL-3.0-or-later
# Static modules copied into /etc/nixos/calamares by the installer. The
# installation media evaluate the same files for their prebuilt reference
# systems, so an installation reuses packages already on the media.
{
  imports = [
    ./desktops.nix
    ./tuning.nix
  ];
}
