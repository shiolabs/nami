# Shells and the desktop's look, the ways people write them in home-manager.
{ config, pkgs, lib, ... }:
let
  plugin = "${pkgs.zsh-you-should-use}/share/zsh/plugins/you-should-use/you-should-use.plugin.zsh";
in
{
  home.packages = [ pkgs.ripgrep ];

  programs.zsh = {
    enable = true;
    enableCompletion = true;
    autosuggestion = { enable = true; highlight = "fg=#888"; };
    syntaxHighlighting.enable = true;
    historySubstringSearch.enable = true;
    defaultKeymap = "viins";
    autocd = true;
    history = { size = 50000; save = 50000; ignoreDups = true; share = true; };
    shellGlobalAliases = { G = "| grep"; };
    plugins = [
      { name = "nix-shell"; src = "${pkgs.zsh-nix-shell}/share/zsh-nix-shell"; file = "nix-shell.plugin.zsh"; }
      { name = "fetched"; src = pkgs.fetchFromGitHub { owner = "a"; repo = "b"; rev = "c"; sha256 = ""; }; }
    ];
    initContent = ''
      source ${plugin}
      bindkey '^R' history-incremental-search-backward
    '';
  };

  programs.fish = {
    enable = true;
    interactiveShellInit = "set fish_greeting";
    functions = {
      mkcd = { body = "mkdir -p $argv[1]; and cd $argv[1]"; description = "make and enter"; };
      gst = "git status";
    };
  };

  programs.zoxide.enable = true;                 # hooks into zsh and fish by default
  programs.fzf = { enable = true; enableFishIntegration = false; };
  programs.atuin = { enable = true; enableNushellIntegration = true; };
  programs.eza.enable = true;
  programs.kitty = { enable = true; shellIntegration.enableZshIntegration = true; };

  home.pointerCursor = {
    name = "Bibata-Modern-Classic";
    package = pkgs.bibata-cursors;
    size = 32;
    gtk.enable = true;
  };

  gtk = {
    enable = true;
    theme = { name = "Adwaita-dark"; package = pkgs.gnome-themes-extra; };
    iconTheme = { name = "Papirus-Dark"; package = pkgs.papirus-icon-theme; };
    font = { name = "Inter"; size = 11; };
    gtk3.extraConfig.gtk-application-prefer-dark-theme = 1;
    gtk3.bookmarks = [ "file:///home/sam/Projects" ];
  };

  programs.tmux = {
    enable = true;
    baseIndex = 1;
    keyMode = "vi";
    mouse = true;
    shortcut = "a";
    plugins = with pkgs.tmuxPlugins; [ sensible { plugin = resurrect; extraConfig = "set -g @resurrect-strategy-nvim 'session'"; } ];
    extraConfig = "bind r source-file ~/.config/tmux/tmux.conf";
  };

  programs.ssh = {
    enable = true;
    includes = [ "~/.ssh/config.d/*" ];
    serverAliveInterval = 60;
    matchBlocks = {
      "*.example.org" = { user = "sam"; forwardAgent = true; };
      # Ordered: before work, though it sorts after it.
      zeta = lib.hm.dag.entryBefore [ "work" ] { hostname = "zeta.example.org"; };
      work = { hostname = "10.0.0.5"; port = 2222; identityFile = [ "~/.ssh/work" ]; identitiesOnly = true; localForwards = [ { bind.port = 8080; host = { address = "localhost"; port = 80; }; } ]; };
    };
  };

  programs.fzf.defaultCommand = "fd --type f";
  programs.fzf.defaultOptions = [ "--height 40%" "--border" ];
}
