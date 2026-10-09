{
  description = "NixOS-focused Rust Calamares installer and the modules of the systems it installs";
  inputs.nixpkgs.url = "https://flakehub.com/f/NixOS/nixpkgs/0.1";
  # The Tatami desktop, in its own repository; installed systems follow its
  # stable branch through their own top-level `tatami` input.
  inputs.tatami = {
    url = "github:bitemyapp/tatami/stable";
    inputs.nixpkgs.follows = "nixpkgs";
  };
  # Yukimi, the app for seeing and changing what is installed, in its own
  # repository; installed systems follow it the same way as Tatami.
  inputs.yukimi = {
    url = "github:bitemyapp/yukimi/stable";
    inputs.nixpkgs.follows = "nixpkgs";
  };
  outputs =
    {
      self,
      nixpkgs,
      tatami,
      yukimi,
    }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
    in
    {
      packages.${system} = {
        default = import ./nix/package.nix { inherit pkgs; };
        # The Tatami session's helper, from the tatami input.
        tatami = tatami.packages.${system}.tatami;
        # Yukimi, from the yukimi input.
        yukimi = yukimi.packages.${system}.yukimi;
        # Restores the user manager's environment between desktop sessions.
        session-env = pkgs.callPackage ./rust/system/session-env/package.nix { };
      };
      # Installed systems import these from their flake's `calamares` input,
      # which follows the `stable` branch: `nix flake update calamares` and a
      # rebuild bring fixes without reinstalling. Options live under
      # `calamares.*`; the installer writes them to configuration.nix.
      nixosModules = {
        default = {
          imports = [
            self.nixosModules.desktops
            self.nixosModules.applications
          ];
        };
        # Desktops, Hyprland, Tatami (from the tatami input), Yukimi (from the
        # yukimi input), swap/zswap and tuning.
        desktops = {
          imports = [
            ./rust/system
            tatami.nixosModules.default
            yukimi.nixosModules.default
          ];
        };
        # The installer's application catalog. Expects the specialArgs
        # applicationPkgs, aiPackages and ompPackage of the installed flake.
        applications = ./rust/src/applications.nix;
      };
      checks.${system} = {
        installer = self.packages.${system}.default;
        tatami = self.packages.${system}.tatami;
        yukimi = self.packages.${system}.yukimi;
        session-env = self.packages.${system}.session-env;
        storage = import ./nix/storage-test.nix { inherit pkgs; };
      };
      formatter.${system} = pkgs.nixfmt;
    };
}
