{
  description = "NixOS-focused Rust Calamares installer and the modules of the systems it installs";
  inputs.nixpkgs.url = "https://flakehub.com/f/NixOS/nixpkgs/0.1";
  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
    in
    {
      packages.${system} = {
        default = import ./nix/package.nix { inherit pkgs; };
        # The Omarchy-style session's helper, also built by its module.
        omarchy = pkgs.callPackage ./rust/system/omarchy/tool/package.nix { };
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
        # Desktops, Hyprland, Omarchy-style Hyprland, swap/zswap and tuning.
        desktops = ./rust/system;
        # The installer's application catalog. Expects the specialArgs
        # applicationPkgs, aiPackages and ompPackage of the installed flake.
        applications = ./rust/src/applications.nix;
      };
      checks.${system} = {
        installer = self.packages.${system}.default;
        omarchy = self.packages.${system}.omarchy;
        storage = import ./nix/storage-test.nix { inherit pkgs; };
      };
      formatter.${system} = pkgs.nixfmt;
    };
}
