//! nami's command line: every command and its options, as clap reads them (help,
//! completions and the docs' reference come from here).

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::{backup, desktop_ctl, import, recipe, setup_sync, style};

/// Declarative packages from nixpkgs, on any Linux distribution, without Nix.
#[derive(Parser)]
#[command(version, styles = style::HELP)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
    /// When to use colour: auto, always or never (auto: in a terminal, unless NO_COLOR is set)
    #[arg(long, global = true, value_name = "WHEN", default_value = "auto", hide_possible_values = true)]
    pub(crate) color: style::Choice,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Get the newest app catalog
    ///
    /// Download a package index (URL or file, .json or .json.xz); with
    /// --refresh, fetch it again from where root last got it (any user)
    Update {
        #[arg(long, required_unless_present = "refresh")]
        from: Option<String>,
        #[arg(long)]
        refresh: bool,
        /// Trust this catalog signing key (name:base64, from `nami keygen`)
        #[arg(long)]
        trust: Option<String>,
        /// Accept a catalog without a signature (one you built yourself)
        #[arg(long)]
        unsigned: bool,
        /// Where the app icons (icons.tar) are: a URL or file. Default: next to --from
        #[arg(long)]
        icons: Option<String>,
        /// Where build definitions live (dir or URL), for `extends:` recipes
        #[arg(long)]
        drvs: Option<String>,
        /// Print JSON: {"ok": true} once it's done; an error as {"error": {"code", "message", "hint"}}
        #[arg(long)]
        json: bool,
    },
    /// Start from an example computer, project or service
    ///
    /// Start from one of nami's examples: a computer, a project, or a service (no name: list them)
    ///
    /// The example's files become yours: `nami new laptop --from namios`
    /// writes laptop/system.nami and copies namios with every package it
    /// lists into ../packages (what's there is kept); `nami new shop
    /// --from node` writes shop/project.nami; `nami new shop --from postgres`
    /// shop/services/postgres.nami.
    New {
        /// The folder: a computer's (named after it), or a project's (default: here)
        folder: Option<PathBuf>,
        /// The example
        #[arg(long)]
        from: Option<String>,
        /// Replace a project.nami or service file that's there
        #[arg(long)]
        force: bool,
        /// The examples as JSON (no --from), for nami-gui
        #[arg(long)]
        json: bool,
    },
    /// Make this computer match its nami file
    ///
    /// Make your apps and settings match your ~/user.nami; with sudo, system.nami, /etc/nami/system.nami; or the one named
    ///
    /// Default: your ~/user.nami (or your config repo's). As root (`sudo nami apply`) or
    /// with --system, the system's. Never a file in the current folder by itself. Any other file by its path
    /// (`nami apply ./user.nami` in a dotfiles repo); a project's project.nami is
    /// for `nami shell` and `nami run`. Keeps the versions in nami.lock; only
    /// new packages are resolved.
    ///
    /// Or a config repo (github:owner/repo[@branch], or any git URL[@branch]):
    /// set this computer up from it in one step. It's cloned into
    /// ~/.config/nami (what was there is moved aside) and its nami file
    /// applied; the system.nami in this computer's folder (named after it), if any, becomes system.nami
    /// (admins). Run it again to pull and apply what changed.
    Apply {
        /// A nami file, or a config repo to set up from
        file: Option<PathBuf>,
        /// Install exactly what nami.lock says, or fail
        #[arg(long, conflicts_with = "upgrade")]
        locked: bool,
        /// Re-resolve everything from the current catalog and update nami.lock
        #[arg(long)]
        upgrade: bool,
        /// Show what would change (packages, services, /etc files) without switching anything; what's needed is still downloaded
        #[arg(long)]
        dry_run: bool,
        /// Passwords for people new on the computer (or a new one for
        /// someone), from stdin: a login:password line each, as chpasswd reads them
        #[arg(long)]
        password_stdin: bool,
        /// Print JSON: with --dry-run, what would change; else {"ok": true} once it's done
        #[arg(long)]
        json: bool,
        /// The system's file, system.nami, as `sudo nami apply` (for programs with no terminal for sudo; admins)
        #[arg(long = "system", alias = "computer", conflicts_with = "file")]
        system: bool,
        /// Your own file, ~/user.nami (the default without sudo)
        #[arg(long, conflicts_with_all = ["file", "system"])]
        user: bool,
    },
    /// Pin and update layers from git repos
    ///
    /// The layers a nami file includes from git repos (github:owner/repo/layer,
    /// git+<url>#layer), each at the commit nami.lock pins it to
    ///
    /// New ones are pinned (and said); --update moves each to the newest
    /// commit of its branch or tag. Nothing else moves a pin: not the
    /// computer's automatic updates (a project's: `nami shell --upgrade` too).
    Layers {
        /// The nami file, or a project's folder (default: the project you're in; else system.nami for admins, your ~/user.nami for others)
        file: Option<PathBuf>,
        /// Move the pins to the newest commits, and say what moved
        #[arg(long)]
        update: bool,
        /// Print JSON: {"file", "lock", "layers": [{"include", "rev", "url"}]}
        #[arg(long)]
        json: bool,
    },
    /// Find problems in a nami file
    ///
    /// Find problems in a nami file without applying it (`-` reads stdin);
    /// a config repo's folder: all of it (each computer's two files)
    Check {
        /// The nami file, or a config repo's folder (default: the project you're in, else the computer's)
        file: Option<PathBuf>,
        /// Where relative paths are relative to (default: the file's directory)
        #[arg(long)]
        base: Option<PathBuf>,
        /// Print the problems as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show the whole computer as one file
    ///
    /// The whole computer in one file: the system file merged with all it
    /// includes, each line saying which file it came from
    ///
    /// `--flat` prints it as YAML, each value's line ending in `# <file>`
    /// (system.nami, ../packages/niri/package.nami, a repo's layer
    /// at its commit). Secrets
    /// stay sealed. A computer nami makes has it in /etc/nami/computer.flat.nami.
    Show {
        /// The system file, or a computer's folder, or a config repo's (its only computer) (default: /etc/nami/system.nami)
        file: Option<String>,
        #[arg(long, required_unless_present = "json")]
        flat: bool,
        /// Print it as JSON (without the comments saying where each value came from)
        #[arg(long)]
        json: bool,
    },
    /// List every option a nami file can have
    ///
    /// Every nami file option, with examples
    Options {
        /// Print them as JSON
        #[arg(long)]
        json: bool,
        /// A JSON Schema of one kind of file, for editors: computer, user, package, project or checks
        #[arg(long, value_name = "FILE")]
        schema: Option<String>,
    },
    /// Turn a Nix setup or old computer into nami files
    ///
    /// Turn a Nix setup into nami files (a flake's dev shell into the project's project.nami, home-manager into your ~/user.nami, a NixOS configuration into system.nami), or an old computer into a config repo (`nami import /` on Ubuntu or Debian, or Windows' disk)
    ///
    /// Read without Nix (when Nix is installed, `nix eval` fills in what the
    /// reader can't). Merges into the target, keeping what's there; what has
    /// no nami equivalent is listed, and noted at the end of the file.
    /// Applies nothing: run `nami apply` (or `nami shell`) next.
    ///
    /// A whole computer (`/`, an old disk mounted somewhere, or Windows'
    /// C:) becomes a config repo, ~/nami-config: the apps, Wi-Fi networks
    /// (sealed; `sudo` reads them), settings, shell and dotfiles. Your files
    /// stay out of it: `--home` says how to move them.
    Import {
        /// A flake (its directory or flake.nix), shell.nix, a home-manager file (home.nix or a module), a NixOS configuration.nix; or a computer: / (Ubuntu, Debian), or Windows' disk
        path: PathBuf,
        /// A computer: the config repo to write (default ~/nami-config); run again, it merges
        #[arg(long)]
        out: Option<PathBuf>,
        /// A computer: whose settings, when several people use it
        #[arg(long)]
        user: Option<String>,
        /// A computer: the repo's address once pushed (github:you/nami-config), for your first login
        #[arg(long)]
        repo: Option<String>,
        /// A computer: write recipes from Ubuntu's packages for apt apps the catalog doesn't have
        #[arg(long)]
        recipes: bool,
        /// A computer: how to move your files; with a folder (or host:folder), copy them there
        #[arg(long, num_args = 0..=1, default_missing_value = "", value_name = "TARGET")]
        home: Option<String>,
        /// A computer, with --home: only these of your files (`Documents,firefox`; --home --json lists them)
        #[arg(long, value_delimiter = ',')]
        only: Option<Vec<String>>,
        /// Which file it goes into (default: by what it is)
        #[arg(long, value_enum)]
        to: Option<import::Tier>,
        /// Write into this nami file instead
        #[arg(long)]
        file: Option<PathBuf>,
        /// Show the result; write nothing
        #[arg(long)]
        dry_run: bool,
        /// What was found, and the resulting file, as JSON (a computer, with --home: your files, for nami-gui)
        #[arg(long)]
        json: bool,
    },
    /// Run the automatic updates
    ///
    /// Automatic updates: the newest nami, then the newest catalog, and on a
    /// computer `nami usb` made a new system from /etc/nami for the next
    /// start if anything changed; on another distribution the computer's
    /// apps at their newest (root, or an admin through the daemon;
    /// nami-update.timer runs it daily). --set-up: the timer, as the
    /// computer's file's `updates:` says (install.sh, on another distribution)
    #[command(hide = true)]
    OsUpdate {
        #[arg(long)]
        set_up: bool,
    },
    /// Run the daily update of your casks
    ///
    /// As a person on a Mac (launchd's dev.namios.update runs it as each one with nami apps of
    /// their own): their apps from Homebrew's casks at their newest, except apps that update
    /// themselves and apps that are open; the rest of their file stays as it is
    #[command(hide = true)]
    OsUpdateCasks,
    /// Undo what nami set up, when it's uninstalled
    ///
    /// Uninstalling (uninstall.sh): the services nami set up stop and go,
    /// its /etc files go (one changed by hand stays), and what they
    /// replaced comes back (root)
    #[command(hide = true)]
    LetGo,
    #[command(hide = true)]
    /// Print a nami file of what's installed
    ///
    /// Print a nami file for the current change
    Export,
    /// Change a nami file, keeping its comments
    ///
    /// Change a nami file in place, keeping its comments
    /// (e.g. `nami config set env.EDITOR nvim`, `nami config add packages tree`)
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
        /// The file (default: your ~/user.nami; with sudo, system.nami; a person's keys always yours); `-`: stdin (an edit prints the changed file)
        #[arg(long, global = true)]
        file: Option<PathBuf>,
        /// The system's file, /etc/nami/system.nami, as with sudo (for programs with no terminal for sudo)
        #[arg(long = "system", alias = "computer", global = true, conflicts_with = "file")]
        system: bool,
        /// Your own file, ~/user.nami (the default without sudo)
        #[arg(long, global = true, conflicts_with_all = ["file", "system"])]
        user: bool,
    },
    /// Open a shell with a project's tools
    ///
    /// A shell with this project's tools (its nami file), like a dev shell
    Shell {
        /// A directory, a project's project.nami, or github:owner/repo[/dir][@ref] (default: the nearest project)
        #[arg(value_name = "PROJECT")]
        dir: Option<String>,
        #[arg(long = "project", hide = true)]
        project: Option<String>,
        /// Fail instead of changing nami.lock
        #[arg(long)]
        locked: bool,
        /// Take the catalog's current versions and rewrite the lock
        #[arg(long)]
        upgrade: bool,
        /// Run this command in the project's shell, then leave it (as `sh -c`)
        #[arg(short = 'c', long = "command", value_name = "COMMAND")]
        command: Option<String>,
    },
    /// Run a project's task, or a command with its tools
    ///
    /// Run a task from the project's project.nami (`nami run test`), or any command (`nami run -- npm test`)
    Run {
        /// A directory, a project's project.nami, or github:owner/repo[/dir][@ref] (default: the nearest project)
        #[arg(long)]
        project: Option<String>,
        /// Fail instead of changing nami.lock
        #[arg(long)]
        locked: bool,
        /// A task's name, or `--` and a command
        #[arg(trailing_var_arg = true, required = true)]
        command: Vec<String>,
    },
    #[command(hide = true)]
    /// Print a project's environment
    ///
    /// Print this project's environment as exports (direnv: `eval "$(nami env)"`)
    Env {
        /// A directory, a project's project.nami, or github:owner/repo[/dir][@ref] (default: the nearest project)
        #[arg(long)]
        project: Option<String>,
        /// Fail instead of changing nami.lock
        #[arg(long)]
        locked: bool,
        /// fish syntax instead of sh
        #[arg(long)]
        fish: bool,
    },
    /// Start a project's services
    ///
    /// Start the project's services (its project.nami's `services:`, or the ones named):
    /// in the foreground, their output prefixed with their names, until Ctrl-C stops them
    Up {
        /// Only these (default: all of them)
        names: Vec<String>,
        /// In the background: back once they're ready
        #[arg(short = 'd', long)]
        detach: bool,
        /// A directory or a project's project.nami (default: the nearest project)
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        locked: bool,
    },
    /// Stop a project's services
    ///
    /// Stop the project's services (or the ones named)
    Down {
        /// Only these (default: all of them)
        names: Vec<String>,
        /// A directory or a project's project.nami (default: the nearest project)
        #[arg(long)]
        project: Option<String>,
    },
    /// Follow a project's services' output
    ///
    /// Follow the output of the project's services (or one's), until Ctrl-C
    Logs {
        /// Only this service's
        name: Option<String>,
        /// A directory or a project's project.nami (default: the nearest project)
        #[arg(long)]
        project: Option<String>,
    },
    /// List a project's tasks and services, or projects
    ///
    /// A project's tasks or services, or your projects (`nami new <folder> --from <starter>` starts one)
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// Run a command without installing it
    ///
    /// Run a command once, without installing it: `nami try cowsay hi` (`, cowsay hi` in namiOS's shells)
    ///
    /// The package that has the command is fetched into the store if it
    /// isn't there (anyone may: no admin needed), run, and left for the
    /// next `nami gc`; again, it starts at once. Several packages have it:
    /// you pick (or --package names one).
    // (`run-once`: its name before, which shells' hooks on computers still call.)
    #[command(alias = "run-once")]
    Try {
        /// The package to run it from (default: the one the command's in)
        #[arg(long, short)]
        package: Option<String>,
        /// What the shell runs when a command isn't found: say which package has it, and ask
        #[arg(long, hide = true)]
        not_found: bool,
        /// Find out again which packages have which commands (weekly by itself)
        #[arg(long)]
        update_table: bool,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    #[command(hide = true)]
    /// Load a project's tools when you cd into it
    ///
    /// Load a project's tools by themselves when you cd into it:
    /// `eval "$(nami hook bash)"` in ~/.bashrc (zsh: same; fish: `nami hook fish | source`)
    Hook {
        /// bash, zsh or fish
        shell: String,
    },
    /// What nami hook runs before each prompt
    ///
    /// What `nami hook` runs before each prompt
    #[command(hide = true)]
    HookEnv {
        #[arg(long)]
        fish: bool,
    },
    /// Back up your home, and restore from it
    ///
    /// Your home's backups, every hour, to go back to: now, list, restore, status
    ///
    /// namiOS backs up each person's home every hour (kept hourly for a day, daily
    /// for a month, weekly after), on this computer and onto a drive when one's
    /// chosen: `namios: backups:` in your user.nami (Settings → Backups).
    Backup {
        #[command(subcommand)]
        command: backup::BackupCommand,
    },
    /// Keep your setup the same on all your computers
    ///
    /// Your setup on all your computers: on, now, status, undo, off
    ///
    /// Your user.nami (apps, shell, settings) kept the same on each computer you turn it
    /// on on, Ubuntu with nami or namiOS alike, through a private git repo of yours
    /// (GitHub's, signed in to in your browser, or any git server's). What changes on one
    /// reaches the others within 15 minutes; where both changed one thing, the later
    /// change wins and the other is in `nami sync status`, to undo. A computer's own
    /// displays, graphics card, backup drive and sound stay its own.
    Sync {
        #[command(subcommand)]
        command: setup_sync::SyncCommand,
    },
    /// Control the namiOS desktop from scripts
    ///
    /// The namiOS desktop from the command line, for scripts and agents: what Control Center does
    /// (sound, brightness, Wi-Fi, Bluetooth, Do Not Disturb, dark or light, Night Shift), Settings'
    /// pages, screenshots, windows, notifications and locking (--json for JSON)
    ///
    /// It asks the desktop running in this session (namiOS), which does it as its own switches
    /// do; what's kept (look, Night Shift, Do Not Disturb, wallpaper) goes into your user.nami.
    Desktop(desktop_ctl::DesktopArgs),
    /// Keep or undo what apps changed in their settings
    ///
    /// Settings files apps may edit (mutable: true): what they changed, and
    /// keeping or undoing it
    Home {
        #[command(subcommand)]
        command: HomeCommand,
        /// The nami file to write kept changes into
        #[arg(long, global = true)]
        config: Option<PathBuf>,
    },
    /// Seal secrets in nami files, and open them
    ///
    /// Secrets in nami files (`key: !secret ...`): seal, edit, show, and who can open them
    Secret {
        #[command(subcommand)]
        command: SecretCommand,
    },
    /// Build a package recipe
    ///
    /// Build a package recipe (a directory with package.nami) and print its store path
    Build {
        recipe: PathBuf,
        /// Keep the build directory when the build fails
        #[arg(long, short = 'K')]
        keep_failed: bool,
    },
    /// Make your program one file that runs on any Linux
    ///
    /// Make your program one file that runs on any Linux distribution (glibc, x86_64), with nothing to install first
    ///
    /// From a recipe folder (`nami bundle ./myapp`), a package from the
    /// catalog (`nami bundle jq`), a store path, or a program you built
    /// (`nami bundle ./target/release/myapp`). The file carries the
    /// program's whole closure, glibc
    /// and its loader too; started, it unpacks itself once into ~/.cache
    /// and runs from there, with the computer's own graphics drivers,
    /// fonts, locale and certificates. With --update-url, it updates itself.
    /// `<file> --nami-bundle-help` lists what the file itself can do.
    #[command(verbatim_doc_comment)]
    Bundle {
        /// A project folder (`.`: Rust, Go, CMake, Meson, or programs already built), a release tarball, a recipe folder, a package, a store path, or a program
        #[arg(required_unless_present_any = ["verify", "show_signing_key", "rotate_key"])]
        what: Option<String>,
        /// The file to write (default: ./<program>-x86_64)
        #[arg(long, short)]
        out: Option<PathBuf>,
        /// Which of the package's programs runs (default: its main one)
        #[arg(long)]
        program: Option<String>,
        /// Also carry nixpkgs' Mesa, for computers with no graphics driver the bundle can use (bigger)
        #[arg(long)]
        mesa: bool,
        /// Also write <file>.AppImage: the same, in AppImage's format (for AppImage tools; it needs FUSE, or --appimage-extract-and-run)
        #[arg(long)]
        appimage: bool,
        /// Also write <file>.desktop and its icon, the app menu entry
        #[arg(long)]
        desktop: bool,
        /// zstd level, 1-22: higher is smaller and slower to make
        #[arg(long, default_value_t = 19)]
        level: i32,
        /// Leave out store paths named so (`--exclude gst-plugins-bad`), and what only they need: for what the program never uses
        #[arg(long, value_name = "NAME")]
        exclude: Vec<String>,
        /// Leave out what cache.nixos.org has: a small file that fetches (and checks) the rest when it first starts, so it needs the network once
        #[arg(long)]
        thin: bool,
        /// Check a bundle against its own hash instead
        #[arg(long, value_name = "FILE", conflicts_with = "what")]
        verify: Option<PathBuf>,
        /// A project's name, where its files don't say (also `bundle: name:` in project.nami)
        #[arg(long)]
        name: Option<String>,
        /// A project's version, where its files and tags don't say (also `bundle: version:`)
        #[arg(long, value_name = "VERSION")]
        app_version: Option<String>,
        /// A project's icon, .png or .svg (also `bundle: icon:`)
        #[arg(long, value_name = "FILE")]
        icon: Option<PathBuf>,
        /// A project's app menu entry, a .desktop file (also `bundle: desktop:`)
        #[arg(long, value_name = "FILE")]
        desktop_file: Option<PathBuf>,
        /// Make it update itself from this folder (an https:// URL, or github:<owner>/<repo> for its latest release): updates are signed with your key (~/.config/nami/bundle-signing.key, made the first time; NAMI_BUNDLE_SIGNING_KEY in CI)
        #[arg(long, value_name = "URL")]
        update_url: Option<String>,
        /// Also write what to upload to the --update-url folder here: the signed feed, the parts and the bundle
        #[arg(long, value_name = "DIR", requires = "update_url")]
        publish_dir: Option<PathBuf>,
        /// With --publish-dir, the release before this one, for updates that download only what changed: its bundle file, the folder it was published from, or its URL; `none` for none (default: what --publish-dir has from the last time, else what --update-url serves now)
        #[arg(long, value_name = "FILE|DIR|URL", requires = "publish_dir")]
        previous: Option<String>,
        /// Your name, as people know you: shown with the key that signs its updates (also `bundle: developer:`)
        #[arg(long, value_name = "NAME")]
        developer: Option<String>,
        /// Run it in a sandbox (bubblewrap) where the computer has one, with only these rights: network, documents, downloads, music, pictures, videos, desktop, home, gpu, audio, camera (also `bundle: sandbox:`)
        #[arg(long, value_name = "RIGHTS", num_args = 0..=1, default_missing_value = "")]
        sandbox: Option<String>,
        /// Print your bundle signing key's public half (making the key if there's none)
        #[arg(long, conflicts_with = "what")]
        show_signing_key: bool,
        /// Make a new signing key, handed over to by the one you have: your next release carries the handover, and the copies people have follow it (the old key is kept beside it)
        #[arg(long, conflicts_with = "what")]
        rotate_key: bool,
        /// Then run it on Ubuntu 20.04 and 24.04, Debian 12, Fedora, Arch, openSUSE and Linux Mint, each in a clean container (rootless podman), and say where it started and opened its window (`nami bundle --test <bundle>`: one made before)
        #[arg(long)]
        test: bool,
        /// With --test: only these distributions (ubuntu:20.04, ubuntu:24.04, debian:12, fedora, arch, opensuse, mint)
        #[arg(long, value_name = "DISTRO", value_delimiter = ',', requires = "test")]
        test_on: Vec<String>,
        /// With --test, for a command: what to run it with (default: --version)
        #[arg(long, value_name = "ARGS", requires = "test", allow_hyphen_values = true)]
        test_args: Option<String>,
        /// With --test: the results as JSON
        #[arg(long, requires = "test")]
        json: bool,
        /// The computers it's for: this one's kind (x86_64 or aarch64) unless you say, or the other for a package from the catalog (`nami bundle jq --arch aarch64`)
        #[arg(long, value_parser = ["x86_64", "aarch64"])]
        arch: Option<String>,
    },
    /// Put the bundle runtime in a nami binary
    ///
    /// Put the bundle runtime in a nami binary (build.sh does, after building)
    #[command(hide = true)]
    AttachBundleRuntime { nami: PathBuf, runtime: PathBuf, net: Option<PathBuf>, arm: Option<PathBuf>, arm_net: Option<PathBuf> },
    /// Write a recipe for you, or update one
    ///
    /// Write a recipe (package.nami) for you, ready for `nami build`: from source code (`nami recipe from-url <repo or tarball URL>`, `nami recipe new <folder>`) or a Debian package (`nami recipe from-deb <url | file.deb | Ubuntu package>`); move one to its newest release (`nami recipe update <dir>`)
    Recipe {
        #[command(subcommand)]
        command: RecipeCommand,
    },
    /// Find the libraries a program is missing
    ///
    /// Why a program built for another distribution won't start: the libraries it needs that this computer doesn't have, and the packages that have them (--add: add them to your programs' libraries)
    Libraries {
        program: PathBuf,
        #[arg(long)]
        add: bool,
        /// It just failed in a terminal: say something only if it's for another distribution and lacks a library
        #[arg(long, hide = true)]
        after_failure: bool,
    },
    /// Start a program built for another distribution
    ///
    /// Start a program built for another distribution (a download, an AppImage), as Files does when you open one: it's made a program first if it isn't one yet (asked), and what it lacks is said
    Start {
        file: PathBuf,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<std::ffi::OsString>,
    },
    /// Add a downloaded app to the launcher
    ///
    /// Put a program built for another distribution (an AppImage, a downloaded app) in the launcher, with its own name and picture when it brings them
    AddToLauncher { file: PathBuf },
    /// Describe a download before installing it
    ///
    /// What a downloaded package (.deb, .rpm, AppImage, tarball) is, before it's installed: its name, version, maker, size, and what won't work
    PackageInfo {
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Open a download as Files does
    ///
    /// Open a download as Files does: an app to install shows in Settings; a tarball of plain files is unpacked beside it
    #[command(hide = true)]
    OpenPackage { file: PathBuf },
    /// List and update apps installed from downloads
    ///
    /// The apps you installed from downloads (`nami install ./foo.deb`); --updates: whether their makers have newer ones; --update [app]: update them now
    FileApps {
        #[arg(long)]
        json: bool,
        /// Say whether there's a newer version of each (nothing is installed)
        #[arg(long)]
        updates: bool,
        /// Update them now (or only APP): each checked as its maker signed it, then put in place
        #[arg(long)]
        update: bool,
        /// Turn APP's automatic updates on or off
        #[arg(long, value_name = "on|off", value_parser = ["on", "off"])]
        automatic: Option<String>,
        /// The daily run's (nami-app-updates.timer): only if this computer updates itself, and quietly
        #[arg(long, hide = true)]
        background: bool,
        /// Where a download's updates would come from, checked (nothing installed)
        #[arg(long, hide = true, value_name = "FILE")]
        check: Option<PathBuf>,
        /// One app: its id, package or name
        app: Option<String>,
    },
    #[command(hide = true)]
    /// Sign a catalog you built
    ///
    /// Sign a catalog you built (index/build-index.sh output) so machines can trust it
    Catalog {
        #[command(subcommand)]
        command: CatalogCommand,
    },
    #[command(hide = true)]
    /// Make a key for signing what nami push uploads
    ///
    /// Create a key for signing what `nami push` uploads; prints the public key
    Keygen {
        /// The key's name (no : or /)
        name: String,
    },
    /// Upload what this computer built to a cache
    ///
    /// Upload what this machine built to a cache directory (serve it anywhere)
    Push {
        /// The cache directory
        dest: PathBuf,
        /// Recipes or store paths (default: everything in the current change)
        targets: Vec<String>,
        /// Sign with this key (its name, from `nami keygen`)
        #[arg(long)]
        key: Option<String>,
    },
    /// Install apps
    ///
    /// They go into your user.nami, for you alone: apps from nami's catalog by
    /// name, from Flathub as flathub:<app id>, a recipe folder (./path), or a
    /// download (.deb, .rpm, AppImage). On a Mac, apps from Homebrew's casks too,
    /// as cask:<token> (cask:rectangle).
    ///
    /// What sets up the computer (a service, files in /etc, drivers: docker, steam, tailscale)
    /// is installed for everyone on it, in system.nami, by an administrator.
    Install {
        /// Apps: names from the catalog, flathub:<app id>, cask:<token> (a Mac), ./recipe folders or downloads (.deb, .rpm, AppImage)
        packages: Vec<String>,
        /// For the system: everyone on this computer, in system.nami (administrators)
        #[arg(long = "system", aliases = ["for-everyone", "everyone"])]
        system: bool,
        /// Print JSON: {"ok": true} once it's done; an error as {"error": {"code", "message", "hint"}}
        #[arg(long)]
        json: bool,
    },
    /// Remove apps
    ///
    /// Each comes off from wherever it's installed: your user.nami, or the
    /// computer's system.nami (administrators). `nami uninstall` is the same command; nami
    /// itself is uninstalled with `sudo nami self-uninstall`.
    #[command(alias = "uninstall")]
    Remove {
        /// The apps, by the names they were installed with
        packages: Vec<String>,
        /// From the system's apps (system.nami), not yours (administrators)
        #[arg(long = "system", aliases = ["for-everyone", "everyone"])]
        system: bool,
        /// Print JSON: {"ok": true} once it's done; an error as {"error": {"code", "message", "hint"}}
        #[arg(long)]
        json: bool,
    },
    /// Update your apps to their newest versions
    ///
    /// Move your apps to the versions in the current catalog (and your Flathub apps to Flathub's newest); with sudo (or --system), the system's apps
    Upgrade {
        /// Only show what would be updated
        #[arg(long)]
        dry_run: bool,
        /// Print JSON
        #[arg(long)]
        json: bool,
        /// Only your apps from Flathub (what the daily timer runs)
        #[arg(long)]
        flathub: bool,
        /// The system's apps, as `sudo nami upgrade` (for programs with no terminal for sudo)
        #[arg(long = "system", alias = "computer", conflicts_with_all = ["mine", "flathub"])]
        system: bool,
        /// Yours, from your ~/user.nami (the default without sudo)
        #[arg(long = "user", alias = "mine")]
        mine: bool,
    },
    /// List what's installed
    List {
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Find apps and packages
    ///
    /// Apps come first; --all includes tools and libraries too. With no
    /// search term, the most used apps. On a Mac, apps from Homebrew's casks
    /// come after the catalog's (cask:<token>).
    Search {
        /// What to look for; left out, the most used apps
        #[arg(default_value = "")]
        term: String,
        /// Tools and libraries too, not only apps
        #[arg(long)]
        all: bool,
        /// Only one kind of app: office, graphics, audio-video, games, internet, coding, learning, tools
        #[arg(long)]
        category: Option<String>,
        /// Print JSON
        #[arg(long)]
        json: bool,
        /// Flathub's apps too, after the catalog's (`flathub:<app id>`; those the catalog has are left out)
        #[arg(long)]
        flathub: bool,
    },
    /// Show details about packages
    ///
    /// What it is, whether you have it, and how many versions there are;
    /// --all shows all of its description and its screenshots;
    /// --screenshots downloads them. `nami versions` lists the versions.
    Info {
        /// Names, or a version of one: firefox@150
        packages: Vec<String>,
        /// Print JSON
        #[arg(long)]
        json: bool,
        /// All of the description, and the screenshots
        #[arg(long)]
        all: bool,
        /// Every version there is of each, as `nami versions` lists them
        #[arg(long)]
        versions: bool,
        /// Download the apps' screenshots (kept in ~/.cache/nami/screenshots): their files instead of URLs
        #[arg(long)]
        screenshots: bool,
    },
    /// List the versions of a package
    ///
    /// Every version the catalog has had, by major version (or minor, like
    /// python3's 3.12 and 3.13), newest first, with when each was current.
    /// Pick one with `nami install firefox@159`, or `firefox: 159` in a nami
    /// file's packages.
    Versions {
        /// Package names: firefox
        #[arg(required = true)]
        packages: Vec<String>,
        /// Every release, not only the newest of each version
        #[arg(long)]
        all: bool,
        /// Print JSON: {"<name>": [{"version", "date", "nixpkgs"}]}, newest first
        #[arg(long)]
        json: bool,
    },
    /// Show who you are to nami, and its catalog
    ///
    /// Who you are to nami, your profile and the catalog
    Status {
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// List past changes, to go back to
    ///
    /// Every change of yours, each one you can go back to; with sudo (or --system), the system's
    #[command(name = "history", alias = "generations")]
    Generations {
        /// Print JSON
        #[arg(long)]
        json: bool,
        /// The system's changes, as `sudo nami history`
        #[arg(long = "system", alias = "computer", conflicts_with = "mine")]
        system: bool,
        /// Yours, from your ~/user.nami (the default without sudo)
        #[arg(long = "user", alias = "mine")]
        mine: bool,
    },
    /// Show what changed between two changes
    ///
    /// What changed between two changes, in words (`nami diff 12`: from change 12 to now;
    /// default: the previous and the current)
    Diff {
        /// The change to compare from (default: the one before the current)
        from: Option<u64>,
        /// The change to compare to (default: the current)
        to: Option<u64>,
        /// The files' lines as `diff -u` has them
        #[arg(long)]
        raw: bool,
        /// As JSON (for nami-gui)
        #[arg(long)]
        json: bool,
        /// Between the system's changes, as `sudo nami diff`
        #[arg(long = "system", alias = "computer")]
        system: bool,
        /// Between your own changes (the default without sudo)
        #[arg(long, conflicts_with = "system")]
        user: bool,
    },
    /// Undo the last change
    ///
    /// Your last change (your apps, your ~/user.nami); with sudo, the computer's.
    /// Or go back to a given change: `nami history` lists them.
    #[command(name = "undo", alias = "rollback")]
    Rollback {
        #[arg(value_name = "CHANGE")]
        generation: Option<u64>,
        /// Print JSON: {"ok": true} once it's done; an error as {"error": {"code", "message", "hint"}}
        #[arg(long)]
        json: bool,
        /// The system's last change, as `sudo nami undo` (for programs with no terminal for sudo; admins)
        #[arg(long = "system", alias = "computer", conflicts_with = "mine")]
        system: bool,
        /// Yours, from your ~/user.nami (the default without sudo)
        #[arg(long = "user", alias = "mine")]
        mine: bool,
    },
    /// Check that this computer's setup works
    ///
    /// Show what GUI support (drivers, locales, fonts) is active; on a
    /// computer nami made, whether what its packages set up works (their checks)
    Doctor {
        /// Each check's result as a JSON line (for nami-gui)
        #[arg(long)]
        json: bool,
        /// Only which of nami's packages this computer's hardware is for
        #[arg(long)]
        hardware: bool,
        /// Only what the computer is: its model, processor, memory, graphics, disks, battery and network
        #[arg(long)]
        about: bool,
    },
    /// Tell nami's makers something's wrong
    ///
    /// Tell nami's makers something's wrong: what happened, with nami doctor,
    /// the last changes and your nami files (secrets left out), shown first
    Report {
        /// What happened (asked for if not given)
        #[arg(long)]
        message: Option<String>,
        /// Send without asking
        #[arg(long)]
        yes: bool,
        /// Leave out the nami files
        #[arg(long)]
        no_files: bool,
        /// Only show what would be sent
        #[arg(long)]
        show: bool,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Move nami's folder to /nami
    ///
    /// nami's folder moves from /nix/var/nami to /nami, and the places older
    /// versions used move to nami's, their old names left pointing there.
    /// Every run of nami as root does it; the installer runs this first, and
    /// on a new computer it makes nami's folder.
    #[command(hide = true)]
    Migrate,
    /// Make system.nami from a personal file
    ///
    /// Make /etc/nami/system.nami from a personal nami file's apps, and apply it (root)
    #[command(hide = true)]
    SystemInit {
        #[arg(long)]
        from: PathBuf,
    },
    /// Make a config repo the computer's /etc/nami
    ///
    /// Make a config repo's system/ the computer's /etc/nami, and apply it (root)
    #[command(hide = true)]
    SystemUse {
        #[arg(long)]
        from: PathBuf,
        #[arg(long)]
        locked: bool,
        #[arg(long)]
        upgrade: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Update nami itself
    ///
    /// Installs the newest nami, signed, from where the catalog comes from.
    SelfUpdate {
        /// Only say whether there's a newer version
        #[arg(long)]
        check: bool,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Uninstall nami itself from this computer
    ///
    /// nami, every app and tool installed with it, and its store (a Mac's Nix Store volume)
    /// go; your nami files (~/user.nami, ~/.config/nami, /etc/nami) and your home folder stay.
    /// It says what it removes first, and asks you to type `uninstall`. Run it with sudo. To
    /// remove one app, use `nami remove <name>`.
    SelfUninstall {
        /// Don't ask: uninstall at once (for scripts)
        #[arg(long, short)]
        yes: bool,
        /// (A package named by mistake: `nami remove` is for those)
        #[arg(hide = true)]
        packages: Vec<String>,
    },
    /// Print the shell script for Tab completion
    ///
    /// Tab completes nami's commands, options, packages (`nami install fi<Tab>`), versions
    /// (`firefox@<Tab>`), the project's tasks and services, changes and nami file keys.
    /// nami's installer and namiOS set it up for every shell they find; for your own
    /// dotfiles: `eval "$(nami completions bash)"` in ~/.bashrc, `eval "$(nami completions
    /// zsh)"` in ~/.zshrc (after compinit), `nami completions fish | source` in fish's config.
    Completions {
        /// bash, zsh or fish
        #[arg(value_parser = ["bash", "zsh", "fish"])]
        shell: String,
    },
    /// Free space: delete old changes
    ///
    /// Delete old changes (by the rules given), then unused store paths. Without
    /// --old, --keep or --older-than, only what nothing uses goes. Old changes also go
    /// by themselves, as `cleanup:` says (30 days, the 3 newest kept): with --auto, as
    /// the daily update does it
    Gc {
        /// As the daily update does: old changes as `cleanup:` says (root: the
        /// computer's, then each person's as them), then what nothing uses, quietly
        #[arg(long, conflicts_with_all = ["old", "keep", "older_than", "dry_run", "now"])]
        auto: bool,
        /// By `cleanup:`'s rules, without waiting: every old change but the newest it
        /// keeps (3) and the current one (Settings' Clean Up Now)
        #[arg(long, conflicts_with_all = ["old", "keep", "older_than"])]
        now: bool,
        /// Delete every change but the current one
        #[arg(long)]
        old: bool,
        /// Keep the newest N changes
        #[arg(long)]
        keep: Option<usize>,
        /// Keep changes younger than this (30d, 12h, 2w)
        #[arg(long)]
        older_than: Option<String>,
        /// Delete nothing: say what would go, and how much it frees
        #[arg(long)]
        dry_run: bool,
        /// Only your own old changes, as you (an administrator's not needed): the space they
        /// used is freed the next time the store is cleaned up (sudo nami gc)
        #[arg(long = "user", alias = "mine", conflicts_with = "auto")]
        mine: bool,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// See how full the disk is and what nami takes
    ///
    /// The disk nami's store is on (on a Mac, the APFS container the Nix Store volume shares),
    /// what nami takes, in parts: your apps, apps for everyone, projects' tools, older changes
    /// kept for undo, what nothing uses any more and nami's own files. Then the apps that take
    /// the most (what removing each would free) and what cleaning up now frees (the nami app's
    /// Clean Up: your own old changes, `nami gc --now --user`, then the computer's and what
    /// nothing uses, `nami gc --now`). Each store path is counted once, in the first part that
    /// uses it; sizes are what the store records for each path (as `nix path-info -S` adds them
    /// up), not walked file by file
    Storage {
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Serve people without root (run as root)
    ///
    /// Serve users without root (run as root; install.sh sets up a service)
    #[command(hide = true)]
    Daemon,
    /// Ask an administrator to change system.nami
    ///
    /// Ask an administrator to change system.nami: for agents and other tools, without the password
    ///
    /// The change is an edit as `nami config json` takes it, or a list of them
    /// (`-` reads it from stdin). It's checked (`nami check`) and kept until
    /// someone decides: Settings shows it with its reason, and Apply asks for an
    /// administrator's password. `nami proposals` lists them. Your own
    /// user.nami needs no approval: change it with `nami config`.
    Propose {
        /// The edit: {"add": {"path": ["packages"], "value": "docker"}}, or a list of edits
        edit: String,
        /// Why, in a sentence, for the person approving it
        #[arg(long)]
        reason: String,
        /// Who's asking, as the person approving it reads it (default: "An app")
        #[arg(long)]
        by: Option<String>,
        /// Wait for the decision, then print the result as JSON
        #[arg(long)]
        wait: bool,
        /// With --wait: give up after this many seconds (default: wait as long as it takes)
        #[arg(long, value_name = "SECONDS", default_value_t = 0)]
        timeout: u64,
        /// Print the proposal as JSON
        #[arg(long)]
        json: bool,
        /// Don't post a notification
        #[arg(long)]
        quiet: bool,
    },
    /// List, approve or reject proposed changes
    ///
    /// Changes to system.nami waiting for approval (`nami propose`): list, show, apply or reject them
    Proposals {
        #[command(subcommand)]
        command: ProposalsCommand,
    },
    /// Connect an AI agent to nami over MCP
    ///
    /// Serve nami's tools to an AI agent over MCP (stdio): `claude mcp add nami -- nami mcp`
    ///
    /// Search, look up, install and remove your apps, change and apply your
    /// user.nami, look back and undo, and propose changes to system.nami for
    /// an administrator to approve. It never changes system.nami itself.
    Mcp,
}

#[derive(Subcommand)]
pub(crate) enum ProposalsCommand {
    /// The proposals waiting for a decision (--all: those decided lately too)
    List {
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// One proposal: who asks, why, and what it changes, in words and lines
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Approve one: an administrator's password is asked for, then system.nami is changed and applied
    Apply {
        id: String,
        /// Only if it's still the proposal with this fingerprint (`show --json`), as it was shown
        #[arg(long)]
        expect: Option<String>,
    },
    /// Don't allow one: it's discarded, and whoever asked is told
    Reject { id: String },
    /// Post the notification of a proposal, and open Settings at it when it's clicked
    #[command(hide = true)]
    Notify { id: String },
}

#[derive(Subcommand)]
pub(crate) enum RecipeCommand {
    /// A recipe from source code at a URL: a GitHub, GitLab or Codeberg
    /// repository (its latest release, or its newest tag), a release or
    /// tag's page, or any tarball. Writes <dir>/package.nami, and the
    /// source's sha256 in nami.lock next to it.
    FromUrl {
        url: String,
        /// A version (or tag) other than the newest
        #[arg(long)]
        version: Option<String>,
        /// Where to write it (default: ./<name>)
        #[arg(long, short)]
        out: Option<PathBuf>,
    },
    /// A recipe for source code in a folder (default: this one)
    New {
        #[arg(default_value = ".", value_hint = clap::ValueHint::DirPath)]
        folder: PathBuf,
        /// Where to write it (default: ./<name>)
        #[arg(long, short)]
        out: Option<PathBuf>,
    },
    /// Move a recipe to its project's newest release (or --version), with
    /// its source's sha256 and every other hash it records made again.
    /// One line per field it changed.
    Update {
        #[arg(default_value = ".", value_hint = clap::ValueHint::DirPath)]
        dir: PathBuf,
        /// A version (or tag) other than the newest
        #[arg(long)]
        version: Option<String>,
        /// Fetch its sources again at the same version, and record their
        /// hashes: for a file that changed upstream, when that's right
        #[arg(long)]
        refetch: bool,
    },
    /// A recipe from a Debian package: a URL, a .deb file, or a package in
    /// Ubuntu's archive by name. Writes <dir>/package.nami to check and build.
    FromDeb {
        what: String,
        /// Where to write it (default: ./<package name>)
        #[arg(long, short)]
        out: Option<PathBuf>,
        /// The Ubuntu release to look a name up in
        #[arg(long, default_value = recipe::RELEASE)]
        release: String,
    },
}

#[derive(Subcommand)]
pub(crate) enum CatalogCommand {
    /// Sign the index in a catalog directory (or one index file): writes <file>.sig
    Sign {
        path: PathBuf,
        #[arg(long)]
        key: Option<String>,
    },
    /// Make the patch from an older catalog's JSON to a newer one's, which a
    /// nami that has the older one downloads instead of the whole catalog
    /// (packaging/publish.sh: patches/<older's sha256>.zst)
    Patch { old: PathBuf, new: PathBuf, out: PathBuf },
    /// The casks the icons of Homebrew's casks are made for (packaging/cask-icons.py), as JSON: those nami installs on a Mac, the most installed first, each with its download and where its app is in it
    CaskIcons {
        /// Homebrew's casks (formulae.brew.sh/api/cask.json)
        #[arg(long)]
        casks: PathBuf,
        /// Homebrew's install counts (formulae.brew.sh/api/analytics/cask-install/365d.json)
        #[arg(long)]
        installs: PathBuf,
        /// Only so many of the most installed (default: all of them)
        #[arg(long)]
        top: Option<usize>,
        /// And these too, when nami installs them (cask:<token> or <token>)
        #[arg(long)]
        also: Vec<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum ConfigCommand {
    /// Set a value (YAML: 10, true, "text", [a, b])
    Set { path: String, value: String },
    /// Remove a key
    Unset { path: String },
    /// Add to a list
    Add { path: String, value: String },
    /// Remove from a list
    Remove { path: String, value: String },
    /// Apply an edit given as JSON ({"set": {"path": [...], "value": ...}} and so on; `{"!secret": "..."}` is a secret, to `nami secret seal`); `-` reads it from stdin. For tools
    Json { op: String },
    /// Replace the whole file with what's on stdin, as it is (for tools: an editor's save)
    Write {
        /// Only if the file is still this text (its sha256), else nothing is written
        #[arg(long, hide = true)]
        expect: Option<String>,
    },
    /// Ask for an admin's password now, for system.nami: the changes of the next few minutes don't ask again (for tools)
    Unlock {
        /// What's changing, in a few plain words, for the box that asks
        about: Option<String>,
    },
    /// Print what the file says at a path (all of it without one): its own lines, not what it lists
    Get {
        path: Option<String>,
        /// As JSON (nothing there: null), for tools
        #[arg(long)]
        json: bool,
        /// The whole file as it's written, comments and all (no path)
        #[arg(long, conflicts_with_all = ["json", "path"])]
        text: bool,
    },
    /// Print which nami file nami uses: your ~/user.nami; with sudo (or --system), system.nami
    Path {
        /// Your own (home settings) instead; as --user (kept for scripts that say it)
        #[arg(long, hide = true)]
        personal: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum ProjectCommand {
    /// The tasks this project defines
    Tasks {
        /// Print JSON: [{"name", "run", "description", "depends"}]
        #[arg(long)]
        json: bool,
    },
    /// The services this project defines, and which are running (`nami up` starts them)
    Services {
        #[arg(long)]
        json: bool,
    },
    /// Projects on this computer you've used (`--json` for the app)
    List {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum HomeCommand {
    /// What apps changed in their settings files, setting by setting
    Diff {
        #[arg(long)]
        json: bool,
    },
    /// Write what apps changed into your nami file (all files, or the ones named), then apply
    Keep { files: Vec<String> },
    /// Put the spec's version back, throwing away what the app changed
    Undo { files: Vec<String> },
    /// Start managing an app's settings file: its current settings go into
    /// nami file (as an editable file), then apply
    Adopt { files: Vec<String> },
}

#[derive(Subcommand)]
pub(crate) enum SecretCommand {
    /// Encrypt each `!secret` still written plain, in place (no files: the repo's nami files)
    Seal { files: Vec<PathBuf> },
    /// Edit a nami file in $EDITOR with its secrets opened; they're sealed again after
    Edit { file: PathBuf },
    /// Print one secret, like `env.STRIPE_KEY` (for scripts); without a key, list them
    Show { file: PathBuf, key: Option<String> },
    /// Let someone (or a computer) open the repo's secrets: add their public key
    /// to .nami-recipients and seal everything again; --remove takes them out
    Allow {
        name: String,
        /// An age key (age1...), an SSH public key, or a .pub file
        key: Option<String>,
        #[arg(long)]
        remove: bool,
    },
    /// Your public key, for someone to `nami secret allow` (made if you have none)
    Key,
    /// Install a git pre-commit hook that refuses secrets that aren't sealed
    Hook,
    /// List secrets that aren't sealed (and fail if there are); --staged: in what git is about to commit
    Check {
        files: Vec<PathBuf>,
        #[arg(long)]
        staged: bool,
    },
}

/// nami's command line, with namiOS's commands when its part is built in.
pub(crate) fn command() -> clap::Command {
    let cmd = <Cli as clap::CommandFactory>::command();
    match crate::nami_os::plugin() {
        Some(os) => os.commands(cmd),
        None => cmd,
    }
}

/// What the command line says: one of nami's commands, or namiOS's (its own to read).
/// (Made once, at the start: its size doesn't matter.)
#[allow(clippy::large_enum_variant)]
pub(crate) enum Parsed {
    Nami(Cli),
    NamiOs(clap::ArgMatches),
}

impl Parsed {
    pub(crate) fn from_matches(mut m: clap::ArgMatches) -> Result<Parsed, clap::Error> {
        use clap::FromArgMatches;
        match crate::nami_os::plugin() {
            Some(os) if m.subcommand_name().is_some_and(|n| os.owns(n)) => Ok(Parsed::NamiOs(m)),
            _ => Cli::from_arg_matches_mut(&mut m).map(Parsed::Nami),
        }
    }

    /// `--color`'s.
    pub(crate) fn color(&self) -> style::Choice {
        match self {
            Parsed::Nami(c) => c.color,
            Parsed::NamiOs(m) => m.get_one::<style::Choice>("color").copied().unwrap_or_default(),
        }
    }
}

/// The command line as data, to compare: every command (hidden ones too), with its
/// aliases, docs, examples and each argument as clap holds it. Keyed by name, so it's the
/// same whichever part adds a command (`""`: nami's own options).
#[cfg(test)]
pub(crate) fn described(cmd: &clap::Command) -> std::collections::BTreeMap<String, serde_json::Value> {
    fn args(c: &clap::Command) -> Vec<String> {
        c.get_arguments()
            .map(|a| {
                let text = format!("{a:?} conflicts: {:?}", c.get_arg_conflicts_with(a).iter().map(|b| b.get_id().as_str()).collect::<Vec<_>>());
                // (Not the type's id: that's the compiler's, not the command line's.)
                let mut out = String::new();
                let mut rest = text.as_str();
                while let Some(i) = rest.find("TypeId(") {
                    out += &rest[..i];
                    rest = &rest[i..];
                    rest = &rest[rest.find(')').map_or(rest.len(), |j| j + 1)..];
                    out += "TypeId";
                }
                let mut out = out + rest;
                // (Nor the type a value is read as: a debug build names it, a release build doesn't.)
                while let Some(i) = out.find("ValueParser::other(") {
                    let from = i + "ValueParser::other".len();
                    let to = out[from..].find(')').map_or(out.len(), |j| from + j + 1);
                    out.replace_range(from..to, "");
                }
                // (Not where `--color` comes among nami's commands: nami's own help is help.rs's.)
                if a.get_id() == "color" {
                    if let Some((head, tail)) = out.split_once(", disp_ord: Some(").map(|(h, r)| (h.to_string(), r.split_once(')').map_or(String::new(), |(_, t)| t.to_string()))) {
                        out = head + &tail;
                    }
                }
                // (Nor clap's extensions: only types, named in a debug build and not in a release one.)
                while let Some(i) = out.find(", ext: Extensions {") {
                    let mut depth = 0;
                    let mut end = out.len();
                    for (j, ch) in out[i..].char_indices() {
                        match ch {
                            '{' => depth += 1,
                            '}' => {
                                depth -= 1;
                                if depth == 0 {
                                    end = i + j + 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    out.replace_range(i..end, "");
                }
                for f in ["help", "long_help", "short", "long", "disp_ord", "val_delim", "terminator", "index"] {
                    out = out.replace(&format!(", {f}: None"), "");
                }
                for f in ["conflicts", "overrides", "groups", "requires", "r_ifs", "r_unless", "aliases", "short_aliases", "default_vals", "default_vals_ifs", "default_missing_vals"] {
                    out = out.replace(&format!(", {f}: []"), "");
                }
                out
            })
            .collect()
    }
    fn one(c: &clap::Command) -> serde_json::Value {
        serde_json::json!({
            "aliases": c.get_all_aliases().collect::<Vec<_>>(),
            "hidden": c.is_hide_set(),
            "about": c.get_about().map(|s| s.to_string()),
            "long_about": c.get_long_about().map(|s| s.to_string()),
            "after_help": c.get_after_help().map(|s| s.to_string()),
            "args": args(c),
            // (`help`'s are clap's, one for each command.)
            "subcommands": c.get_subcommands().filter(|_| c.get_name() != "help").map(|s| (s.get_name().to_string(), one(s))).collect::<std::collections::BTreeMap<_, _>>(),
        })
    }
    let mut cmd = cmd.clone();
    cmd.build();
    let mut all: std::collections::BTreeMap<String, serde_json::Value> = cmd.get_subcommands().map(|s| (s.get_name().to_string(), one(s))).collect();
    all.insert(String::new(), serde_json::json!({ "args": args(&cmd) }));
    all
}

/// The commands `which` says, as `file` keeps them (NAMI_GOLDEN=write: written anew).
#[cfg(test)]
pub(crate) fn same_as_kept(file: &std::path::Path, which: fn(&str) -> bool) {
    // (clap's building of it is deep: more stack than a test's thread has.)
    let now = std::thread::Builder::new().stack_size(64 << 20).spawn(|| described(&crate::help::command(crate::style::Choice::Never))).unwrap().join().unwrap();
    let now: std::collections::BTreeMap<String, serde_json::Value> = now.into_iter().filter(|(n, _)| which(n)).collect();
    if std::env::var("NAMI_GOLDEN").as_deref() == Ok("write") {
        std::fs::write(file, serde_json::to_string_pretty(&now).unwrap() + "\n").unwrap();
    }
    let was: std::collections::BTreeMap<String, serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    let names = |m: &std::collections::BTreeMap<String, serde_json::Value>| m.keys().cloned().collect::<Vec<_>>();
    assert_eq!(names(&now), names(&was));
    for (name, spec) in &was {
        assert_eq!(&now[name], spec, "`nami {name}` changed");
    }
}

#[cfg(test)]
mod tests {
    /// The command line stays as it was: installed computers' units and activation, nami
    /// itself and nami-gui call these commands, the hidden ones too, by name and arguments.
    /// (namiOS's are its own to keep.)
    #[test]
    fn the_command_line_is_as_it_was() {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cli.golden.json");
        super::same_as_kept(&file, |name| !crate::nami_os::plugin().is_some_and(|os| os.owns(name)));
    }
}
