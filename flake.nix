{
  description = "NixOS-focused Rust Calamares installer";
  inputs.nixpkgs.url = "https://flakehub.com/f/NixOS/nixpkgs/0.1";
  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
    in
    {
      packages.${system}.default = import ./nix/package.nix { inherit pkgs; };
      checks.${system}.installer = self.packages.${system}.default;
      formatter.${system} = pkgs.nixfmt;
    };
}
