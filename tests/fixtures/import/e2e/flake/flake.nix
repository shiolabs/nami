# The same setup as ../home.nix, as a home-manager flake: for
# tests/import.sh, where there's no Nix (nami fetches it to read this).
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    home-manager = {
      url = "github:nix-community/home-manager";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs = { nixpkgs, home-manager, ... }: {
    homeConfigurations.alice = home-manager.lib.homeManagerConfiguration {
      pkgs = nixpkgs.legacyPackages.x86_64-linux;
      modules = [
        ./home.nix
        {
          home.username = "alice";
          home.homeDirectory = "/home/alice";
          home.stateVersion = "25.05";
        }
      ];
    };
  };
}
