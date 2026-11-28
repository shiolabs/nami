# Nami on NixOS, declared. Optional: the installer sets up the same without
# touching your configuration (packaging/setup.sh). To declare it instead,
# import this module and run `sudo nixos-rebuild switch`.
#
# Nami shares the Nix store (it goes through nix-store, never writes to
# /nix/store itself), so this only adds what NixOS keeps in its own config:
# the daemon for users without sudo, and Nami's apps on PATH and in the menu.
#
# Nami's own folder is /nami: there, on the file system /nix is on; where / is
# another (an ephemeral root, wiped at each start), it stays in /nix/var/nami,
# and /nami is a link to it, made at each start below.
{ config, ... }:
{
  systemd.tmpfiles.rules = [ "L /nami - - - - nix/var/nami" ];

  systemd.services.nami-daemon = {
    description = "Nami daemon (store access for users without root)";
    wantedBy = [ "multi-user.target" ];
    path = [ config.nix.package ];
    serviceConfig = {
      ExecStart = "/nami/bin/nami daemon";
      Restart = "on-failure";
    };
  };

  environment.extraInit = ''
    export PATH="$HOME/.local/state/nami/profile/bin:/nami/profiles/default/bin:/nami/bin:$PATH"
    export XDG_DATA_DIRS="$HOME/.local/state/nami/profile/share:/nami/profiles/default/share:''${XDG_DATA_DIRS:-/run/current-system/sw/share}"
    export XCURSOR_PATH="$HOME/.icons:$HOME/.local/state/nami/profile/share/icons:/nami/profiles/default/share/icons:''${XCURSOR_PATH:-/run/current-system/sw/share/icons}"
  '';
}
