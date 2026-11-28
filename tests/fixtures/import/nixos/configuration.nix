# Edit this configuration file to define what should be installed on
# your system. Help is available in the configuration.nix(5) man page.
{ config, pkgs, lib, ... }:

{
  imports = [
    ./hardware-configuration.nix
  ];

  boot.loader.systemd-boot.enable = true;
  boot.loader.efi.canTouchEfiVariables = true;

  networking.hostName = "laptop";
  networking.networkmanager.enable = true;

  time.timeZone = "Europe/Copenhagen";

  services.openssh = {
    enable = true;
    ports = [ 2222 ];
    settings = {
      PasswordAuthentication = false;
      PermitRootLogin = "no";
      AllowUsers = [ "ada" "backup" ];
      KexAlgorithms = [ "curve25519-sha256" "sntrup761x25519-sha512" ];
    };
    openFirewall = true;
  };
  services.xserver.enable = true;
  services.tailscale = {
    enable = true;
    useRoutingFeatures = "client";
  };
  services.syncthing = {
    enable = true;
    user = "ada";
    dataDir = "/home/ada/Sync";
  };
  services.postgresql.enable = true;
  services.printing.enable = false;

  environment.etc."motd".text = ''
    Welcome to the laptop.
  '';
  environment.etc."myapp/config.toml" = {
    source = ./myapp.toml;
    mode = "0640";
  };
  environment.etc."sudoers".text = "root ALL=(ALL) ALL\n";
  environment.etc."resolv.conf".source = "${pkgs.writeText "resolv" "nameserver 1.1.1.1"}";

  systemd.services.myapp = {
    description = "My app";
    after = [ "network-online.target" ];
    wants = [ "network-online.target" ];
    wantedBy = [ "multi-user.target" ];
    path = [ pkgs.curl ];
    environment = { RUST_LOG = "info"; PORT = 8080; };
    serviceConfig = {
      ExecStart = "${pkgs.hello}/bin/hello --greeting hi";
      User = "myapp";
      Restart = "always";
    };
  };
  systemd.services.backup = {
    wantedBy = [ "multi-user.target" ];
    script = ''
      cd /srv
      tar czf /backup/srv.tgz .
    '';
  };
  systemd.services.ping = {
    wantedBy = [ "multi-user.target" ];
    script = "ping -c1 example.org";
  };
  systemd.services.nix-daemon.environment.TMPDIR = "/var/tmp";
  systemd.tmpfiles.rules = [ "d /srv 0755 root root -" ];

  users.users.ada = {
    isNormalUser = true;
    description = "Ada";
    extraGroups = [ "wheel" "networkmanager" "docker" ];
    packages = with pkgs; [ thunderbird ];
  };

  home-manager.users.ada = { pkgs, ... }: {
    home.shellAliases.update = "sudo nixos-rebuild switch";
    home.stateVersion = "24.05";
  };

  virtualisation.docker = {
    enable = true;
    logDriver = "json-file";
    autoPrune.enable = true;
    daemon.settings = {
      data-root = "/srv/docker";
      registry-mirrors = [ "https://mirror.example.org" ];
      default-address-pools = [ { base = "172.30.0.0/16"; size = 24; } ];
    };
  };
  virtualisation.podman = {
    enable = true;
    dockerCompat = true;
    defaultNetwork.settings.dns_enabled = true;
  };
  virtualisation.libvirtd.enable = true;

  nixpkgs.config.allowUnfree = true;

  environment.systemPackages = with pkgs; [
    vim
    wget
    git
    firefox
    htop
  ];

  fonts.packages = [ pkgs.noto-fonts ];

  programs.steam.enable = true;
  programs.zsh.enable = true;
  programs.dconf.enable = true;

  system.stateVersion = "24.05";
}
