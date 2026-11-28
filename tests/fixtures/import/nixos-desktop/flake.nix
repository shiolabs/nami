{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    home-manager.url = "github:nix-community/home-manager";
  };
  outputs = { self, nixpkgs, home-manager, ... }: {
    devShells.x86_64-linux.default = nixpkgs.legacyPackages.x86_64-linux.mkShell { packages = [ nixpkgs.legacyPackages.x86_64-linux.nixfmt ]; };
    nixosConfigurations.desk = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [ ./configuration.nix home-manager.nixosModules.home-manager ];
    };
  };
}
