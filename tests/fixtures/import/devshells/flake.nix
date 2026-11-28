{
  description = "A Go service";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      pkgs = nixpkgs.legacyPackages.x86_64-linux;
      tools = with pkgs; [ go gopls golangci-lint ];
    in
    {
      devShells.x86_64-linux.default = pkgs.mkShell {
        nativeBuildInputs = tools ++ [ pkgs.delve ];
        CGO_ENABLED = 0;
      };

      devShells.x86_64-linux.ci = pkgs.mkShell {
        packages = tools;
      };
    };
}
