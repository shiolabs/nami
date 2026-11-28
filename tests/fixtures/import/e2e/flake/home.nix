# A typical home-manager setup, for tests/import.sh: zsh with oh-my-zsh and
# plugins, a prompt and tools that hook into the shell, tmux, variables.
{ config, pkgs, ... }:
{
  home.packages = with pkgs; [ ripgrep jq ];
  home.sessionVariables = {
    EDITOR = "vim";
    NOTES = "$HOME/notes";
  };
  home.shellAliases.gs = "git status";

  programs.zsh = {
    enable = true;
    autosuggestion.enable = true;
    syntaxHighlighting.enable = true;
    oh-my-zsh = { enable = true; plugins = [ "git" ]; };
    initContent = ''
      export NAMI_TEST_INIT=yes
    '';
  };
  programs.starship.enable = true;
  programs.zoxide.enable = true;
  programs.eza.enable = true;
  programs.git = { enable = true; userName = "Alice"; userEmail = "alice@example.org"; };
  programs.tmux = { enable = true; mouse = true; baseIndex = 1; };
}
