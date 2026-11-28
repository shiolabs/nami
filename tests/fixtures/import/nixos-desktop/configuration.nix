{ config, pkgs, lib, ... }:
{
  home-manager.users.sam = import ./home.nix;
  users.users.sam = { isNormalUser = true; extraGroups = [ "wheel" "video" ]; shell = pkgs.fish; };

  boot.loader.grub.enable = true;
  boot.kernel.sysctl = { "vm.swappiness" = 10; "net.ipv4.ip_forward" = true; };
  boot.kernelModules = [ "v4l2loopback" ];
  boot.blacklistedKernelModules = [ "pcspkr" ];
  boot.extraModprobeConfig = "options v4l2loopback exclusive_caps=1";

  i18n.defaultLocale = "en_GB.UTF-8";
  i18n.extraLocaleSettings.LC_TIME = "de_DE.UTF-8";
  console.keyMap = "uk";
  services.xserver.xkb = { layout = "gb"; variant = "extd"; };

  environment.sessionVariables = { MOZ_ENABLE_WAYLAND = "1"; };
  environment.systemPackages = [ pkgs.htop ];

  services.udev.extraRules = ''
    SUBSYSTEM=="usb", ATTR{idVendor}=="1209", MODE="0666"
  '';
  systemd.tmpfiles.settings."10-media"."/srv/media".d = { mode = "0775"; user = "sam"; group = "users"; };

  programs.hyprland.enable = true;
  programs.firefox = { enable = true; policies = { DisableTelemetry = true; }; };
  programs.git = { enable = true; config = { init.defaultBranch = "main"; }; };
  programs.localsend = { enable = true; openFirewall = true; };

  systemd.user.services.notes = {
    description = "Notes server";
    wantedBy = [ "default.target" ];
    serviceConfig.ExecStart = "notes-server --port 4000";
  };

  nixpkgs.config.allowUnfree = false;
  system.stateVersion = "26.05";
}
