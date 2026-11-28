{ pkgs, ... }:
{
  programs.git = {
    enable = true;
    userName = "Ada";
    userEmail = "ada@example.org";
    aliases = {
      co = "checkout";
      st = "status -sb";
    };
    extraConfig = {
      init.defaultBranch = "main";
      pull.rebase = true;
      url."git@github.com:".insteadOf = "https://github.com/";
    };
    ignores = [ ".direnv" "result" ];
  };
}
