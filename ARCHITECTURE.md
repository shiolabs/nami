# Architecture

A map of nami's code, for someone about to change it: what each part does and
where it lives. What nami is and how to use it is in [README.md](README.md)
and [docs/guide/](docs/guide/README.md); building and testing is in
[CONTRIBUTING.md](CONTRIBUTING.md).

nami is one Rust program. It installs packages from nixpkgs, which the Nix
community builds, signs and serves from cache.nixos.org. nami has no packages
of its own: nixpkgs is read ahead of time into a signed catalog, nami looks
names up there, and downloads the ready-built store paths, checking each one.
It never runs Nix's evaluator to install something.

| File | Whose | Applied to |
|---|---|---|
| `~/user.nami` | one person (`--user`, the default without sudo) | their own profile, dotfiles, shell and user services |
| `/etc/nami/system.nami` | everyone on the computer (`--system`, or sudo) | the system profile, system services, files in `/etc` |
| `project.nami` + `nami.lock` | a project | its tools, on `PATH` only inside `nami shell` and `nami run` |

The names are constants in `src/config/mod.rs` (`USER_FILE`, `SYSTEM_FILE`,
`PROJECT_FILE`); which file a command means is `default_config` in
`src/cmd/files.rs`.

## 1. `nami install firefox`, end to end

1. **Parse.** `src/main.rs` parses the command line (`src/cli.rs`, clap) and
   decides whether it needs the store, root or the daemon.
   `cmd::daemon::for_whom` sends an app that sets up the computer (a service,
   files in `/etc`) to system.nami through the daemon; the rest stays in the
   person's user.nami.
2. **Open the store.** `Store::open` (`src/store.rs`): root gets the store
   directly; anyone else a read-only view plus the daemon's socket.
3. **Edit the file.** `cmd::packages::install` finds the file
   (`ensure_user_config` writes `~/user.nami` the first time), refuses a name
   the catalog doesn't have (`config::check_tier`), and adds the line with
   `src/config_edit.rs`, which keeps comments and layout.
   `cmd::apply::edit_and_apply` applies it, and puts the old text back if
   the apply fails.
4. **Read the file.** `cmd::apply::apply_config_with` loads it with what it
   includes (`src/config/merge.rs`) and checks it (`src/config/check.rs`).
5. **Resolve.** A name pinned in `nami.lock` (`src/lock.rs`) keeps its store
   paths. A new one goes through `ops::resolve` (`src/ops.rs`) to `Index::get`
   (`src/index.rs`), which reads one catalog entry in place
   (`src/index_file.rs`) and returns its outputs (`Package::install_paths`).
   Unfree or uncached packages are built instead (`Builder::ensure_catalog`).
6. **Fetch the closure.** `Profiles::commit_with` (`src/profile.rs`) calls
   `Store::realise`: it walks each `.narinfo`'s `References`, checks every
   signature (`src/narinfo.rs`), downloads and unpacks each NAR while hashing
   it (`src/nar.rs`), and renames each path into `/nix/store`. Only when the
   whole closure is there does it write the records that make the paths
   valid. Someone who isn't root has the daemon do this (`Request::Realise`).
7. **Make a generation.** `commit_with` builds a tree of links (`default-N/`)
   and its manifest (`default-N.json`), then `Profiles::switch` renames the
   `default` link to it.
8. **Activate.** `switch` updates app menu entries (`src/desktop.rs`), links
   dotfiles (`src/home.rs`), writes `/etc` files and starts, stops or
   restarts services (`src/etc.rs`, `src/services.rs`).
9. **Record.** `apply_config_with` writes `nami.lock` if it changed, keeps the
   file's text with the generation (so `undo` can restore it), and prunes old
   generations when `history:` asks. `src/tell.rs` says what changed.

`nami remove`, `upgrade` and `apply` follow the same path from step 4.
`--dry-run` stops before the commit (`cmd::apply::dry_run_report`).

## 2. The catalog

### How it's built

`index/build-index.sh <release-url> <out-dir>` builds the catalog for one
nixpkgs release. It is the one place where Nix evaluates nixpkgs, in a
throwaway `nixos/nix` container or where Nix is installed.

| File | What it adds |
|---|---|
| `index/catalog.nix`, `index/in-container.sh` | every installable package (walked with nix-eval-jobs): name and version, outputs, description, licence, main program |
| `index/closures.py`, `index/uncached.py` | how to build what cache.nixos.org doesn't have (Hydra doesn't build unfree software) |
| `index/apps.py`, `index/meta.py`, `index/desktop.py`, `index/nar.py` | which packages are apps; their icons, names, summaries, categories |
| `index/history.py`, `index/versions.py` | every version each package has had |
| `index/defs.py` | the build definitions (`drvs/`), in shards |

It writes `index-x86_64-linux.json(.xz)`, `drvs/`, `defs/`, `icons.tar` and
`versions/`, which `nami catalog sign <dir> --key <name>` signs
(`catalog::sign`). The catalogs people download are built and signed by the
maintainers.

On a person's computer Nix runs only in two optional places: a tool from a
repo's `flake.nix` (`src/flake.rs`) and `nami import` of a Nix setup
(`src/import/nixtool.rs`), each with the computer's Nix or one from the
catalog in a store of its own.

### Format

- `index.json` (`src/index.rs`): an object keyed by nixpkgs attribute. Each
  `Package` has `name`, `outputs`, `description`, `license`, `apps`, `drv`
  and `icon` (sha256 of its build definition and icon), `unfree`, `closure`,
  `uncached`, and for apps that aren't in nixpkgs, `cache` and `publisher`.
- `index.idx` (`src/index_file.rs`): a compact form made from the JSON on the
  computer, never downloaded: the names in order, where each entry is, and
  what search reads. Both are memory-mapped, so a lookup reads one entry and
  nothing is parsed up front. It's made again when it doesn't match its JSON.

### Signing and trust

Whoever signs the catalog decides what `nami install firefox` means, so nami
only takes one signed by a key the computer trusts.

- The signature is ed25519 over the file's sha256 (`catalog::verify`). The
  index carries the sha256 of every build definition and icon, so one
  signature covers them (`Builder::definition`, `Index::fetch_icons`).
- Trusted keys are in `/nami/catalog-keys.json`. The installer adds
  the publisher's; `nami update --trust <name:base64>` adds one.
  `packaging/keys` lists the keys the default catalog and releases use.
- `nami update --unsigned` accepts a catalog without a `.sig`, for one you
  built. An unsigned catalog names no extra caches.

### Channels and updates

`nami update --from <url>` takes an index file or a channel: a small file
naming a release folder next to it. The default is
`https://namios.dev/menu/nixos-unstable`, naming `https://namios.dev/menu/<rev>/`.
`Index::fetch_resolved` records `index-source` (what was asked for, followed
by later updates), `index-resolved` (the exact index), and `versions-source`
and `drvs-source`.

With a catalog already here, `Index::fetch_patched` reads the new one's signed
JSON (`index-x86_64-linux.json.sig`) first: nothing to download if it's the
same, else a zstd patch from the old one (`patches/<old sha256>.zst`, made by
`nami catalog patch`), checked against that signature. Icons come from
`icons-128.tar` (or `icons.tar`), or a few at a time from
`menu/objects/<sha256>`, each checked against the index.
`Index::keep_catalog` keeps the newest six catalogs in
`/nami/menus/<sha256>.json` for projects pinned to an older one
(`Index::load_pinned`). `src/site.rs` holds nami's web address; a lock names
a channel and release, never a server.

### Versions

`index/history.py` adds every earlier version of each package, from
nixos-unstable releases since 2016 and stable channels from 16.03 on. It reads
each release's package list and evaluates only the releases where a version
first appears. `index/versions.py` splits the result into 4096 shards by the
first three hex digits of `sha256(attr)`: `versions/<shard>.json`, each signed.

nami fetches a shard only when a version is asked for (`History::list`),
checks its signature and caches it. `nami versions firefox` lists them.
`firefox@150`, or `- firefox: 150` in a file (`src/config/packages.rs`), is
the newest build whose version is 150 or starts with `150.`, `150-` or
`150+` (`index::version_matches`, `Index::get`), siblings like `nodejs_20`
included.

## 3. The store

- **Places.** Packages live in `/nix/store` (`store::STORE_DIR`), which can't
  move: nixpkgs programs have it compiled in. nami's state is `/nami`
  (`migrate::var_dir`; `/nix/var/nami`, its place before 0.20, a link to it,
  or the folder itself where `/` can't hold it).
- **Registry.** `/nami/db/<hash>-<name>.json` (`PathInfo`): each
  path's references, NAR hash and size. nami never touches a store path
  without a record, so what another tool put in the store is left alone.
- **Caches.** `Store::caches`: cache.nixos.org first (`NIXOS_CACHE`,
  `NIXOS_CACHE_KEY`, built in), then root's `caches:` (`caches.json`), then
  those the signed catalog names (`catalog-caches.json`, from
  `Index::trust_caches`). A catalog's caches add no trust beyond its signer.
  `/nami/nixos-mirror` can name a mirror of cache.nixos.org; what it
  serves is still checked against cache.nixos.org's key.
- **narinfo** (`NarInfo::verify`): one `Sig` must verify under a trusted key
  over `1;<path>;<narHash>;<narSize>;<references>`. `Store::find_narinfo`
  also refuses a narinfo for another path than the one asked for.
- **NAR** (`Store::unpack_nar`): decompressed (zstd, xz or none), unpacked by
  `nar::unpack` into `/nix/store/.nami-tmp-<name>` while
  `nar::HashingReader` hashes it, refused unless size and hash match, then
  renamed into place. `store::store_path` derives hashes as Nix does
  (`src/nix/base32.rs`); what nami adds or builds is content-addressed.
- **Next to Nix** (`src/nix/store.rs`). `Nix::detect` finds an installed Nix
  (`/nix/var/nix/db/db.sqlite` and a `nix-store`); then the store is Nix's.
  Downloads go through `nix-store --realise` with nami's extra caches,
  builds go in with `nix-store --import` from `/nami/staging`, and
  each path nami uses gets a GC root under `/nix/var/nix/gcroots/`, so
  `nix-collect-garbage` leaves nami's apps alone. `NAMI_NIX=off` turns this
  off; `tests/nix.sh` covers it.
- **Cleaning up.** `Store::gc_with` deletes registered paths no root reaches
  (`profile::all_roots`: every generation of every profile, existing
  projects, and what running programs use, read from `/proc` by
  `cleanup::running_paths`). It unregisters a path before deleting it; under
  Nix it asks `nix-store --delete`. The `cleanup:` policy (30 days, newest 3
  kept) is `src/cleanup.rs`; the daily update runs `nami gc --auto`, which
  spares paths registered in the last hour.

## 4. Profiles and generations

`src/profile.rs` (`Profiles`):

| Profile | Where | From |
|---|---|---|
| system | `/nami/profiles/default` -> `default-N/` | system.nami |
| graphics | `/nami/profiles/runtime` -> `runtime-N/` | added when an app needs the GPU (`src/runtime.rs`) |
| a person's | `/nami/profiles/per-user/<login>/default` -> `default-N/` | their user.nami |

`~/.local/state/nami/profile` links to a person's profile; shells and the
desktop find apps through it (`install.sh` writes `/etc/profile.d/nami.sh` and
the session environment). `profiles/per-user` is sticky and world-writable,
like `/tmp`, so people make their own profile without root.

- A **generation** is a tree of links merging every package, a manifest
  (`default-N.json`: packages, runtime, caches, home, services, `/etc` files)
  and links to its rendered parts (`.home`, `.units`, `.etc`).
- **Switching** (`Profiles::switch`) renames a fresh link over `default`,
  runtime first, so apps never see older drivers. The system profile's cache
  list belongs to the generation too.
- **History**: `nami history`, `nami diff [A] [B]` (`Profiles::diff`) and
  `nami undo [N]` (`ops::rollback`), in `src/cmd/generations.rs`.
- **Dotfiles** (`src/home.rs`): `files: ~/...`, `env:`, `aliases:` and
  `shell:` are rendered into one store path per generation. Each `~/<file>`
  links into `<profile>/default/.home/`, so a switch changes them all at once.
  A file in the way is kept as `<file>.nami-backup`.
- **Services** (`src/services.rs`) are systemd units: system units from
  system.nami, `systemctl --user` ones from user.nami. **`/etc` files**
  (`src/etc.rs`) come only from system.nami. Both are copied, not linked
  (they may be read before `/nix` is mounted), with their hashes in
  `units.json` and `etc-files.json` so hand edits are told apart.
  Activation stops removed units, writes files, reloads systemd, then starts
  and restarts what changed.
- **Graphics** (`src/runtime.rs`): nixpkgs programs never read `/usr/lib`, so
  when a closure has libglvnd, mesa or the Vulkan loader, the system profile
  gets nixpkgs' mesa, fonts and locales, linked at `/run/opengl-driver` as on
  NixOS. A person's app asks the daemon (`Request::EnsureGraphics`).

## 5. The daemon and polkit

`src/daemon.rs`. Only root writes to `/nix/store`; everyone else talks to the
hidden `nami daemon`, a root service from `install.sh` (`nami-daemon.service`,
or OpenRC and runit equivalents), on `/nami/daemon.sock`
(`store::DAEMON_SOCKET`).

- **Protocol**: one JSON request per connection, `{"log": ...}` lines while it
  works, then `{"ok": ...}` or `{"error": ...}`. Store changes are served one
  at a time.
- **Who's asking**: `peer` reads the caller's pid and uid from the socket
  (`SO_PEERCRED`). Admins are in `wheel`, `sudo` or `admin`
  (`profile::is_admin_user`).
- **Operations** (`Request`), following Nix's model: anyone may call them,
  because none can plant something under a path another person trusts.

| Request | Why it's safe |
|---|---|
| `realise` | downloads only from caches root trusts, signatures checked |
| `add-nar` | the path is computed from the archive's content |
| `build` | the outputs are the hash of the whole job (`build::Job`), built in the sandbox |
| `replace` | the new paths are computed from the old ones and the swap (`src/replace.rs`) |
| `build-catalog` | it names a package; only what the signed catalog describes is built |
| `ensure-graphics`, `refresh-index` | they act only on the system profile and root's catalog source |
| `run` | `nami <args>` as root, only for commands in `ADMIN_COMMANDS` |

- **Admins never need sudo.** An admin's system-wide command reaches the
  daemon as `run`: installs and removals through `cmd::daemon::for_whom`,
  the rest (`nami apply --system`, `nami undo --system`, ...) through
  `cmd::daemon::forward`. The daemon checks it against `ADMIN_COMMANDS`,
  allows `config` only on system.nami (`config_args_ok`), and streams back
  the output. Root only runs allowed nami commands, and a person's own files
  are read by `nami` running as them.
- **When polkit asks** (`asks_password`): a change to system.nami (install,
  remove, `config`, undo, applying another file as the system's, passwords)
  asks for the admin's password under polkit's `org.namios.change-computer`
  action (`allowed`, through `pkcheck`; `pkttyagent` in a terminal). Applying
  system.nami as it is, updates and cleaning up don't ask. Someone who isn't
  an admin needs an admin's name and password every time (`unlocked`).
  Without polkit, admins are trusted and others refused.

## 6. nami.lock and resolution

`src/lock.rs`. A nami file's lock is `nami.lock` next to it; a plain
`~/user.nami` keeps its lock in `~/.config/nami/nami.lock` (`lock::path_for`).
It holds `version`, `system` (a lock from another kind of computer is
refused), `catalog` (channel and release, or a source, and its sha256),
`packages` (each name's store paths; a `github:` package's commit),
`runtime`, `layers` (each repo at a commit) and `sources` (a recipe's
downloads and their sha256).

- A locked name keeps its paths after the catalog moves on; only new names
  are resolved. `nami apply --upgrade` re-resolves everything; `--locked`
  installs exactly the lock or fails.
- A project resolves against the catalog its lock pins (`Catalog::source_for`,
  `Index::load_pinned`), so it comes out the same on every computer.
- A name the catalog no longer has is left out with a message instead of
  stopping the apply.

**Sets and layers.** A file builds on another by listing it under
`packages:`: `./work.nami`, or one in a git repo, `github:alice/os/gaming.nami@v2`
(`config::is_set_file`). `src/gitref.rs` parses the one address form,
`github:owner/repo[/path][@ref][#name]` or `git+<url>[@ref][#name]`.
`src/layers.rs` fetches repos and pins each at a commit in `nami.lock`; only
`nami layers --update` or `nami apply --upgrade` moves a pin.
`github:owner/repo#tool` is a tool another project provides
(`ops::resolve_remote`).

**Merging** (`config::merge`): what's included goes underneath the file that
includes it. Maps merge key by key, lists are joined without duplicates, and
anything else is the including file's. `src/config/` also has the keys
(`read.rs`), the linter (`check.rs`, `nami check`) and every option's docs
(`options.rs`, `nami options`); JSON Schemas are `src/schema.rs`.

## 7. Recipes and builds

A recipe is a folder with a `package.nami`, listed as `./packages/mytool` or
built with `nami build ./mytool` ([docs/guide/recipes.md](docs/guide/recipes.md)).

- **The idea** (`src/build.rs`): a nixpkgs build is bash, `stdenv/setup` and
  environment variables, and stdenv, its hooks and the compilers are
  ordinary store paths in the binary cache. nami writes the variables from
  YAML and runs nixpkgs' own builder, so every nixpkgs hook works unchanged.
- **Sandbox** (`Builder::sandbox`): nixpkgs' bubblewrap, with only the inputs'
  closure visible, read-only, under `/nix/store`, a fresh `/build`, uid 1000,
  no network. Only downloads with a known hash get the network.
- **Output paths** (`Job::output_paths`) are a hash of the whole job, so a
  recipe gives the same path everywhere. `Builder::run` first asks the caches
  for it (`Store::substitutable`) and downloads it if someone pushed it.
- **`extends: jq`** (`Builder::build_extends`) starts from the package's build
  definition (`drvs/<attr>.json`, checked against the signed sha256), applies
  the recipe, and builds only that package; every input stays nixpkgs'.
- **Kinds of project** (`src/build_kinds.rs`): Cargo, Go, Meson, CMake,
  autotools, Python, Node.js and Make, from the source's files (read in place
  by `src/archive.rs`) unless `build-system:` says. `src/vendor.rs` fetches
  each dependency as the project's lock file pins it and checks its hash, so
  the build runs offline.
- **Failures** (`src/build_errors.rs`) are said as the line to add, with
  `src/recipe/names.rs` mapping a header, pkg-config module, library or
  command to a catalog package.
- **Writing recipes** (`src/recipe/`): `nami recipe from-url` and `new`
  (`from_source.rs`, `wants.rs`, `upstream.rs`), `from-deb` (`from_deb.rs`,
  `src/deb.rs`) and `update` (`update.rs`).
- **Without rebuilding**: `replace:` swaps a library in built packages
  (`src/replace.rs`); `src/python_env.rs` makes a Python with packages as
  `python3.withPackages` does.
- **Sharing builds** (`src/push.rs`, [docs/guide/share-builds.md](docs/guide/share-builds.md)):
  `nami keygen <name>` makes an ed25519 key in Nix's format; `nami push <dir>`
  writes what this computer built as a binary cache (`<hash>.narinfo`,
  `nar/<filehash>.nar.zst`, `nix-cache-info`). Other computers list it under
  `caches:` with its key. Nix can read it too.

## 8. Bundles

`nami bundle` makes one file that runs a program on any glibc Linux with
nothing installed first ([docs/guide/ship-your-app.md](docs/guide/ship-your-app.md)).

- **Format** (`src/bundle_format.rs`): a static runtime, one zstd frame per
  store path (its NAR), a JSON description (what to run, each frame's offset,
  size and sha256), and a 64-byte trailer with the description's sha256.
  `nami bundle --verify <file>` checks one.
- **Runtime** (`src/bundle_runtime.rs`): on start it unpacks the frames it
  doesn't have into one store all bundles share under `~/.cache`
  (`bundle_format::cache_dirs`), replacing every `/nix/store` with a
  same-length `/tmp/<prefix>` that links there, so no program needs patching.
  It then finds the host's graphics drivers and starts the program with the
  bundle's own glibc. `<file> --nami-bundle-help` lists what a file can do.
- **Binaries** (`src/bin/`): the runtimes aren't nami; they include a few of
  its files by `#[path]`. `--thin` bundles use `nami-bundle-runtime-net`,
  which fetches what was left out and checks it against the description
  (`src/bundle_fetch.rs`). `build.sh` attaches both to `nami` (hidden
  `nami attach-bundle-runtime`).
- **From a project** (`src/bundle_project.rs`, `src/bundle_langs.rs`):
  `nami bundle .` builds Rust, Go, CMake, Meson, Python, Node.js, Electron,
  Zig, Flutter or .NET with the catalog's tools.
- **Updates** (`src/bundle_update.rs`, `src/bundle_publish.rs`,
  `src/bundle_delta.rs`): `--update-url` puts the developer's ed25519 key and
  feed address in the description; `--publish-dir` writes the signed
  `update.json`, the description and the parts. The runtime checks the feed
  at most daily, assembles the new file beside the old one from frames it
  has plus downloads or zstd patches, checks every piece, and renames it into
  place at the next start.
- **Trust** (`src/bundle_trust.rs`): the first copy run says whose key signs
  its updates, remembered in `~/.config/nami-bundle/trusted/`. An update
  signed with another key is refused unless the trusted key handed over to it
  (`--rotate-key`).
- **Sandbox** (`src/bundle_sandbox.rs`): optional (`--sandbox`, or
  `bundle: sandbox:` in project.nami): bubblewrap, a home of its own, and
  only the rights the developer declared.
- `action/` is a GitHub Action that bundles a project for a release.

## 9. Updating nami itself

`src/selfupdate.rs`. The release server is the catalog's source without
`/menu/<channel>` (`selfupdate::base`); `<base>/releases/latest` names the
newest version. `nami self-update` fetches `SHA256SUMS` and `SHA256SUMS.sig`,
verifies the signature with the trusted catalog keys (`catalog::verify`),
checks the tarball against its line, and runs the release's `setup.sh`.

`src/auto_update.rs`, `src/cmd/update.rs`: `nami-update.timer` runs the hidden
`nami os-update` daily as root: the newest nami, then the newest catalog and
the system's apps. `updates: manual` in system.nami takes the timer away.

## 10. MCP

`src/mcp.rs` ([docs/guide/agents.md](docs/guide/agents.md)). `nami mcp` serves
nami's tools to an AI agent over the Model Context Protocol on stdio
(JSON-RPC, a message per line, no network). Each tool runs nami as the
person and returns what it prints, as JSON where nami has it: `search`,
`info`, `show`, `get`, `check`, `diff`, `history`, `install`, `remove`,
`config`, `apply`, `undo`, `propose` and `proposal`.

An agent changes only the person's own user.nami. For system.nami it
proposes: `nami propose` (`src/proposals.rs`, `src/cmd/propose.rs`) stores the
edit and its reason in `~/.local/state/nami/proposals/`, and nothing changes
until an administrator runs `nami proposals apply`, through the daemon and
polkit. `tests/mcp.rs` drives the real binary in a scratch root.

## 11. Module map

### src/

**Command line**
- `main.rs` entry point, which commands need the store, root or the daemon · `cli.rs` every command and option (clap)
- `cmd/` each command group (`packages`, `apply`, `config`, `generations`, `project`, `bundle`, `catalog`, `daemon`, `propose`, `setup`, `desktop`, `os`, `misc`); `cmd/files.rs` which nami file a command means
- `help.rs` `nami` and `nami help` · `show.rs` search, info and versions in a terminal · `query.rs` read-only questions, tables and `--json`
- `tell.rs` what a change did, in words · `style.rs` colour · `log.rs` progress lines, also from the daemon · `errors.rs` `--json` errors
- `complete.rs`, `complete/` tab completion for bash, zsh and fish

**nami files**
- `config/` types and loading (`mod.rs`), keys (`read.rs`), `packages:` entries (`packages.rs`), includes (`merge.rs`), the linter (`check.rs`), option docs (`options.rs`), `tests.rs`
- `config_edit.rs` edits that keep comments · `yaml_text.rs` YAML as written · `format.rs` the format version · `file_diff.rs` two versions of a file in words
- `lock.rs` nami.lock · `layers.rs` sets from git repos · `gitref.rs` git addresses
- `secrets.rs` sealed secrets (`nami secret`) · `schema.rs` JSON Schemas · `flat.rs` `nami show --flat` · `duration.rs` `30d`, `12h`
- `oldnames.rs`, `migrate.rs` compat: names nami had before, still read

**Catalog and store**
- `index.rs` the catalog and versions · `index_file.rs` its compact form · `catalog.rs` signatures and trusted keys · `site.rs` nami's web address
- `store.rs` the store and binary caches · `narinfo.rs` narinfo and its signature · `nar.rs` NAR · `nix/base32.rs` Nix's base32 · `nix/store.rs` sharing the store with Nix
- `cleanup.rs` the `cleanup:` policy, running programs' roots

**Profiles and what a file sets up**
- `profile.rs` profiles, generations, switching · `ops.rs` install, remove, upgrade, undo
- `home.rs` dotfiles, env, aliases · `drift.rs` settings files apps may edit · `settings.rs` desktop settings (GSettings)
- `services.rs` systemd units · `etc.rs` files in `/etc` · `runtime.rs` GPU drivers, fonts, locales · `desktop.rs` app menu entries

**Building**
- `build.rs` recipes and the sandbox · `build_kinds.rs` kinds of project · `vendor.rs` dependencies from lock files · `build_errors.rs` failed builds in words · `archive.rs` tar and zip
- `recipe/` `nami recipe from-url`, `new`, `from-deb`, `update`, and name lookups
- `push.rs` keygen and push · `replace.rs` `replace:` · `flake.rs` flake tools · `python_env.rs` Python with packages
- `deb.rs` .deb files · `rpm.rs` .rpm files · `elf.rs` ELF dynamic sections

**Bundles**
- `bundle.rs` `nami bundle` · `bundle_project.rs` from a project folder · `bundle_langs.rs` Python, Node.js, Electron, Zig, Flutter, .NET
- `bundle_format.rs` the file · `bundle_runtime.rs` its runtime · `bundle_fetch.rs` thin bundles' parts · `bundle_sandbox.rs` the sandbox
- `bundle_menu.rs` menu entries · `bundle_ask.rs` the first-start question · `bundle_test.rs`, `bundle_test.sh` `nami bundle --test`, and its script in the container
- `bundle_update.rs` self-updating · `bundle_publish.rs` the developer's side · `bundle_delta.rs` patches · `bundle_trust.rs` whose key is trusted
- `bundle_app.rs` a bundle installed as one person's app

**System and sharing the computer**
- `daemon.rs` the root daemon · `proposals.rs` proposed changes · `mcp.rs` the MCP server
- `selfupdate.rs` `nami self-update` · `auto_update.rs` the daily timer
- `doctor.rs` `nami doctor` · `hardware.rs` hardware a package is for · `report.rs` `nami report`
- `wifi.rs` `wifi:` networks · `backup.rs` home backups · `desktop_ctl.rs` `nami desktop`
- `nami_os.rs` where namiOS plugs in: `here()` (is this computer namiOS?), `plugin()` and the `NamiOs` trait. namiOS's part (making a bootable system from system.nami, and keeping it up to date) is a folder of its own, `namios/` (nami's part and the app's), built in when it's there (build.rs's `cfg(namios)`); it isn't part of nami's public repo, and nami builds, works and passes its tests without it · `config/computer.rs` a computer's own keys in system.nami (hostname, users, desktop...), which nami reads and checks on any distribution

**Projects, other apps, moving in**
- `project.rs` projects, `nami shell`, `run`, `env` · `up.rs` project services (`nami up`) · `new.rs` `nami new` · `run_once.rs` `nami try`
- `flathub.rs` Flathub apps · `foreign.rs` .deb, .rpm and AppImage downloads for one person · `foreign_update.rs` their updates · `openpgp.rs` repository signatures · `zsync.rs` AppImage delta updates
- `other_linux.rs` programs built for other distributions
- `import.rs`, `import/` `nami import` of Nix setups · `nix/expr.rs` a small Nix-language reader
- `machine.rs` a computer from a config repo · `setup_sync.rs` `nami sync`

### src/bin/

- `nami-bundle-runtime.rs`, `nami-bundle-runtime-net.rs` the start of every bundle (section 8)

### gui/

The nami app: a crate of its own (`gui/Cargo.toml`), built on GPUI, the UI
framework of the Zed editor. It runs the `nami` command underneath and edits
the same files, keeping their comments. One of nami's own modules is built
into it as it is: `file_diff.rs`.

- `src/main.rs` **nami-gui**: the nami app on any distribution · `pages/` a page each (Discover, Installed, Your Setup, Projects, History, Sync, Requests, Move to namiOS) · `shell.rs`, `shell/` the window: what the files say, the sidebar and the toolbar
- `src/os.rs` where the window meets the system it's on: `Os`, what a system adds to it (its pages, sidebar lines, keys); `Plain`, nami's own window, adds nothing
- `src/stick/` making a namiOS USB drive with your apps (Move to namiOS): the latest release, the drives, the writer, your files for namiOS's first start
- Shared: `look.rs` colours, type and controls · `nami.rs` running `nami` and reading its JSON · `editor.rs`, `editor_lang.rs` the text editor for nami files · `motion.rs` animation
- `vendor/` GPUI and fatfs, with small changes marked `namiOS:` · `NOTICE` what's someone else's, and its licence

### The rest of the repository

| Path | What it is |
|---|---|
| `index/` | builds the catalog from a nixpkgs release (section 2) |
| `packaging/keys` | the keys the default catalog and releases are signed with |
| `packaging/apps/` | recipes for apps that aren't in nixpkgs, added to the catalog |
| `packaging/check-app.sh` | builds one of those as a release would, and shows what people would see |
| `packaging/extra-apps.py` | adds a built extra app to a catalog |
| `packaging/bundle.sh` | an offline installer (`.run`): nami, a catalog, the NixOS module |
| `packaging/setup.sh` | what every installer runs: nami, then the signed catalog |
| `packaging/install-web.sh` | the web installer: checks the release, then runs `setup.sh` |
| `packaging/distro-packages.sh` | a `.deb` and `.rpm` around the release |
| `packaging/nixos/nami.nix` | a NixOS module, to declare nami instead of installing it |
| `install.sh`, `uninstall.sh` | set up and remove nami's files, daemon and shell integration |
| `build.sh` | the static (musl) binary in a container, bundle runtimes attached |
| `build.rs` | builds `examples/` and release notes into the binary |
| `action/` | the GitHub Action for `nami bundle` |
| `examples/` | example user, project, service and package files, for `nami new` |

## 12. Tests

- **Unit tests** sit next to their code (`#[cfg(test)]`, and
  `src/config/tests.rs`) and touch only temp folders: `cargo test --release`,
  as yourself, not as root.
- **`tests/mcp.rs`** runs the built `nami` as an agent would, in a scratch
  computer (`NAMI_ROOT`, `HOME` and `XDG_STATE_HOME` in a temp folder).
- **`tests/*.sh`** test whole features on other distributions in containers
  (rootless podman, through `tests/lib/docker`), with the static build; most
  have an `-inner.sh` that runs inside. Each says what it checks in its first
  lines. Fixtures are in `tests/fixtures/`.
- `NAMI_ROOT` makes nami treat a folder as `/` (`src/main.rs`), so tests never
  touch the computer they run on.

How and when to run which: [CONTRIBUTING.md](CONTRIBUTING.md).

---

nami stands on nixpkgs and the Nix community, who build, sign and serve every
package it installs.
