{ pkgs, ... }:
{
  home.packages = [ pkgs.ripgrep ];
  programs.fish = { enable = true; shellAbbrs = { g = "git"; }; };
  programs.starship.enable = true;
}
