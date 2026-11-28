# What a NixOS or home-manager configuration sets, as Nix evaluates it:
# every option some file in the flake defines (not nixpkgs' or
# home-manager's own modules), with its value, in the shape the
# configuration is written in. `nami import` reads this like a file, so it
# works however the flake is put together (flake-parts, helper functions,
# hosts directories...).
#
# Values are data: a package is { __pkg = { name, pname, out }; }, a path
# { __path = "..."; }, a function or what fails to evaluate
# { __unknown = "..."; }.
# want: "user" (a home configuration, else a NixOS one's home-manager user
# and user units) or "system" (the NixOS configuration). part: "all",
# "config" or "home" (one of the two), "none" (only which configuration and
# its roots, for reading part by part with `only`), "files" (the files
# home-manager links into ~, to read).
{ flake, want, name ? "", user ? "", host ? "", only ? "", part ? "all" }:
let
  f = builtins.getFlake flake;
  # nixpkgs' lib: the configuration's own (a NixOS one has it, a home one
  # through its pkgs), else whichever input has it (not every flake calls
  # its nixpkgs `nixpkgs`).
  inputLib = builtins.foldl' (a: i: if a == null && (i ? lib) && (i.lib ? evalModules) then i.lib else a) null (builtins.attrValues (f.inputs or { }));
  lib = cfg.lib or cfg.pkgs.lib or (if inputLib != null then inputLib else f.inputs.nixpkgs.lib);
  src = toString f.outPath;
  # Definitions from the flake's own files; the generated hardware file is
  # this machine's disks and drivers, which the distro has its own of.
  mine = d: lib.hasPrefix src (toString d.file) && baseNameOf (toString d.file) != "hardware-configuration.nix";

  nixosNames = builtins.attrNames (f.nixosConfigurations or { });
  homeNames = builtins.attrNames (f.homeConfigurations or { });
  kind =
    if want == "user" && homeNames != [ ] then "home"
    else if nixosNames != [ ] then "nixos"
    else if homeNames != [ ] then "home"
    else "none";
  configs = if kind == "nixos" then f.nixosConfigurations else if kind == "home" then f.homeConfigurations else { };
  names = builtins.attrNames configs;
  # (builtins only: lib comes from the configuration picked here.)
  userAt = builtins.filter (n: builtins.substring 0 (builtins.stringLength user + 1) n == "${user}@") names;
  picked =
    if name != "" then name
    else if kind == "home" && builtins.elem user names then user
    else if kind == "home" && userAt != [ ] then builtins.head userAt
    else if kind == "nixos" && builtins.elem host names then host
    else builtins.head names;
  cfg = configs.${picked};

  try = v: let r = builtins.tryEval v; in if r.success then r.value else { __unknown = "doesn't evaluate"; };

  # mkIf, mkMerge, mkDefault/mkForce, mkBefore/mkAfter inside a value, as
  # what they mean here.
  plain = v:
    if builtins.isAttrs v && v ? _type then
      if v._type == "if" then (if v.condition then plain v.content else null)
      else if v._type == "merge" then merge (map plain v.contents)
      else if v._type == "override" || v._type == "order" then plain v.content
      else v
    else v;

  # Several definitions of one option, as the module system merges them for
  # the common types: lists add up, sets merge (later wins), else the last.
  merge = vals:
    let vs = builtins.filter (x: x != null) vals; in
    if vs == [ ] then null
    else if builtins.all builtins.isList vs then lib.concatLists vs
    else if builtins.all (x: builtins.isAttrs x && !lib.isDerivation x) vs then
      builtins.foldl' (a: b: a // lib.mapAttrs (k: bv: if a ? ${k} then merge [ a.${k} bv ] else bv) b) { } vs
    else lib.last vs;

  data = depth: v0:
    let v = plain v0; t = builtins.typeOf v; in
    if depth > 14 then { __unknown = "nested too deeply"; }
    else if lib.isDerivation v then {
      __pkg = {
        name = try (v.name or "");
        pname = try (v.pname or "");
        out = try (builtins.unsafeDiscardStringContext (v.outPath or ""));
      };
    }
    else if t == "set" then lib.mapAttrs (_: x: try (data (depth + 1) x)) (lib.filterAttrs (k: x: k != "_module" && plain x != null) v)
    else if t == "list" then map (x: try (data (depth + 1) x)) (builtins.filter (x: plain x != null) v)
    else if t == "lambda" then { __unknown = "a function"; }
    else if t == "path" then { __path = toString v; }
    else if t == "string" then builtins.unsafeDiscardStringContext v
    else v;

  # One option, as the flake's own files define it (not the final value,
  # which has every default of a submodule in it too).
  value = o:
    let defs = builtins.filter mine (o.definitionsWithLocations or [ ]); in
    if defs == [ ] then null
    else data 0 (merge (map (d: plain d.value) defs));

  # The parts of a configuration nami has a place for (or lists): the rest
  # (hardware, initrd, ...) is never forced, since an option nothing uses
  # may not even evaluate (hardware.nvidia.* without an NVIDIA card), and
  # that's an error tryEval can't catch.
  roots = {
    nixos = [ "programs" "services" "environment" "users" "systemd" "boot" "i18n" "time" "networking" "console" "fonts" "nixpkgs" "xdg" "security" "virtualisation" ];
    home = [ "home" "programs" "services" "xdg" "gtk" "qt" "dconf" "systemd" "fonts" "nixpkgs" "editorconfig" ];
  };
  bootKeys = [ "kernel" "kernelModules" "blacklistedKernelModules" "extraModprobeConfig" "loader" ];

  # Renamed and removed options are invisible shims whose value aborts
  # (uncatchable too): their value is never touched, but what the flake
  # wrote in them is read, under the old name (`programs.git.userName`),
  # which the mappings know; the new option only has the shim's copy, in
  # home-manager's file.
  shim = o: (o.visible or true) == false;

  # Something switched off (`enable` false, by anyone) isn't entered either,
  # nor what's only left as a renamed shim.
  off = opts: opts ? enable && lib.isOption opts.enable
    && (shim opts.enable || (let r = builtins.tryEval (opts.enable.value == false); in r.success && r.value));

  walk = depth: opts:
    if depth > 12 then { }
    else
      lib.filterAttrs (_: x: x != null) (lib.mapAttrs
        (n: o:
          let
            r = builtins.tryEval (
              if lib.isOption o then value o
              else if builtins.isAttrs o && !lib.isDerivation o && n != "_module" && !(depth > 0 && off o) then
                (let w = walk (depth + 1) (if depth == 0 && n == "boot" then lib.getAttrs (builtins.filter (k: o ? ${k}) bootKeys) o else o); in if w == { } then null else w)
              else null);
          in
          if r.success then r.value else null)
        opts);

  # `only`: one root, for reading a configuration part by part when
  # something in it aborts.
  top = which: opts: walk 0 (lib.getAttrs (builtins.filter (k: opts ? ${k} && (only == "" || only == k)) roots.${which}) opts);

  # home-manager inside NixOS: the user's own evaluation.
  hmUsers =
    if kind == "nixos" then (cfg.options.home-manager.users.valueMeta.attrs or { }) else { };
  hmUser =
    if hmUsers == { } then ""
    else if builtins.hasAttr user hmUsers then user
    else if builtins.length (builtins.attrNames hmUsers) == 1 then builtins.head (builtins.attrNames hmUsers)
    else "";
in
if kind == "none" then { inherit kind; } else
if part == "files" then {
  files =
    let
      hc = if kind == "home" then cfg.config else if hmUser != "" then cfg.config.home-manager.users.${hmUser} else null;
      home = hc.home.homeDirectory or "";
      drvs = s: builtins.filter (lib.hasSuffix ".drv") (builtins.attrNames (builtins.getContext s));
      # Each file home-manager links into ~: its text when it has one,
      # else its source and the derivations that make it, so nami reads
      # what it can without building everything they refer to.
      one = f:
        let
          src = toString f.source;
          text = if f.text or null != null then f.text else null;
          t = f.target;
        in {
          target = if lib.hasPrefix "${home}/" t then lib.removePrefix "${home}/" t else t;
          source = builtins.unsafeDiscardStringContext src;
          text = if text == null then null else builtins.unsafeDiscardStringContext text;
          # What the source is made by, and what its text refers to.
          drvs = drvs src;
          refs = if text == null then [ ] else drvs text;
        };
    in
    if hc == null then null
    else map (f: try (one f)) (builtins.filter (f: f.enable or true) (builtins.attrValues hc.home.file));
} else
{
  inherit kind src;
  name = picked;
  names = names;
  roots = roots.${kind};
  homeRoots = roots.home;
  config = if part == "home" || part == "none" then null else top kind cfg.options;
  homeUser = hmUser;
  homeUsers = builtins.attrNames hmUsers;
  home = if hmUser == "" || part == "config" || part == "none" then null else top "home" hmUsers.${hmUser}.configuration.options;
}
