{ config, pkgs, lib, ... }:

let
  gaming = false;
  myAliases = {
    ll = "${pkgs.eza}/bin/eza -l";
    gs = "git status";
  };
in
{
  imports = [ ./git.nix ];

  home.username = "ada";
  home.homeDirectory = "/home/ada";
  home.stateVersion = "24.05";

  home.packages = with pkgs; [
    ripgrep
    fd
    htop
    nerd-fonts.fira-code
  ] ++ lib.optionals gaming [ steam ];

  home.sessionVariables = {
    EDITOR = "nvim";
    GOPATH = "${config.home.homeDirectory}/go";
  };
  home.sessionPath = [ "$HOME/.local/bin" ];

  home.shellAliases = myAliases;

  programs.bash = {
    enable = true;
    initExtra = ''
      set -o vi
    '';
  };

  programs.neovim = {
    enable = true;
    vimAlias = true;
    defaultEditor = true;
    plugins = with pkgs.vimPlugins; [ telescope-nvim ];
  };

  programs.starship = {
    enable = true;
    settings = {
      add_newline = false;
      character.success_symbol = "[➜](bold green)";
    };
  };

  programs.direnv = {
    enable = true;
    enableBashIntegration = true;
    nix-direnv.enable = true;
  };

  home.file.".vimrc".text = ''
    set number
    syntax on
  '';
  xdg.configFile."nvim".source = ./dotfiles/nvim;

  dconf.settings = {
    "org/gnome/desktop/interface" = {
      color-scheme = "prefer-dark";
      clock-format = "24h";
    };
    "org/gnome/desktop/wm/preferences".num-workspaces = 4;
    "org/gnome/desktop/peripherals/keyboard" = {
      repeat-interval = lib.hm.gvariant.mkUint32 30;
    };
  };

  services.syncthing.enable = true;
  services.gpg-agent = {
    enable = true;
    defaultCacheTtl = 1800;
  };

  systemd.user.services.notes-sync = {
    Unit.Description = "Sync my notes";
    Unit.After = [ "network-online.target" ];
    Service = {
      ExecStart = "${pkgs.rclone}/bin/rclone bisync remote:notes /home/ada/notes";
      Environment = [ "RCLONE_VERBOSE=1" ];
    };
    Install.WantedBy = [ "default.target" ];
  };
  systemd.user.services.watcher = {
    Service.ExecStart = "/home/ada/bin/watch";
    Service.ExecStartPre = "/home/ada/bin/prepare";
    Install.WantedBy = [ "default.target" ];
  };

  programs.home-manager.enable = true;
}
