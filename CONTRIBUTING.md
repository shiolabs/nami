# Contributing

Thanks for wanting to help. nami is in alpha, so the most useful things right now are:

- **Problems.** Run `nami report`: it shows you everything it would send
  (your message, nami's version, your distribution, `nami doctor`'s output,
  your last ten changes and your nami files, secrets left out) and asks before
  sending. Or open an issue with what you did, what happened and
  `nami --version`.
- **Ideas and questions.** Open an issue, or email hi@shiolabs.dev.
- **Security problems.** Don't open an issue; see [SECURITY.md](SECURITY.md).
- **An app that isn't in nixpkgs.** Send its recipe:
  [packaging/apps/README.md](packaging/apps/README.md) says how.

Everyone taking part follows the [code of conduct](CODE_OF_CONDUCT.md).

## The one rule

nami changes a computer as root. Never run a nami you built with `sudo` on
your own computer: not `sudo target/release/nami ...`, not
`sudo ./install.sh`, not `./uninstall.sh`. Build and run the unit tests on
your computer (they only touch temporary folders), and try everything else in
a container.

## What you need

- Rust (stable), to build and run the unit tests, on a glibc distribution.
- Docker or podman, for `./build.sh` and the test suites. Without Docker,
  `tests/lib/docker` runs rootless podman in its place; the suites put it
  first on `PATH` themselves.
- Or nami itself: `nami shell` in this folder has Rust, podman and Python
  ([project.nami](project.nami)); `nami run build` and `nami run test` run
  cargo behind everything else on your desktop.

## Build and test

| Step | Where | What it does |
|---|---|---|
| `cargo test --release` | your computer, as yourself | Builds nami and runs the unit tests, and `tests/mcp.rs` with the built nami in a scratch root. Not as root. About 8 minutes on a 16-core computer from a clean checkout, nearly all of it compiling; the tests themselves take seconds. |
| `./build.sh` | a container | The static binary, `target/release/nami`, with the bundle runtimes attached. The suites use it. About 8 minutes the first time. |
| `tests/<suite>.sh` | containers | Whole features on real distributions. Each script says what it checks, what it needs and how to run it in its first lines. |
| `cargo test --manifest-path gui/Cargo.toml` | your computer, as yourself | Builds the nami app and runs its unit tests. It needs the libraries its windows use; see below. |

Most suites install real packages, so they take the package catalog. Download
the current one, then run the suites your change touches:

```sh
rev=$(curl -fsSL https://namios.dev/menu/nixos-unstable)
curl -fsSLO "https://namios.dev/menu/$rev/index-x86_64-linux.json.xz"
./build.sh
tests/distros.sh index-x86_64-linux.json.xz   # install, apply, undo on 7 distributions
tests/home.sh index-x86_64-linux.json.xz      # a person's dotfiles
```

Once the container images are downloaded, these two take under a minute
each. Suites that build, bundle or start real apps on several distributions
(`tests/build.sh`, `tests/app-bundle.sh`, `tests/gui.sh`) take longer.

A few suites need neither a container nor root, such as `tests/secrets.sh`,
`tests/recipe-sources.sh` and `tests/deb.sh`; their first lines say so. Some
check things only a maintainer has, such as a signing key or a catalog with
build definitions; those say what they take as arguments.

The app's libraries, on Ubuntu or Debian:

```sh
sudo apt install pkg-config libfontconfig-dev libfreetype-dev libxkbcommon-dev libxkbcommon-x11-dev libxcb1-dev libwayland-dev
```

With Nix, `nix develop .#gui` has them.

`dev/quiet <command>` runs a build behind everything else on your desktop,
with a low CPU share and a memory ceiling. The `nami run` tasks use it.

To get disk space back: `target/`, `~/.config/nami-tests` (podman's images
and volumes, when it stands in for Docker) and the Docker volume `nami-cargo`
(`./build.sh`'s cargo cache).

## Where things are

[ARCHITECTURE.md](ARCHITECTURE.md) explains how nami works and has a map of
every module. In short:

| Folder | What's in it |
|---|---|
| `src/` | nami: the command line, the daemon, the store, profiles, recipes, bundles |
| `src/bin/` | the bundle runtime, the small program at the front of every bundle |
| `gui/` | the nami app (`nami-gui`), a crate of its own |
| `index/` | building the package catalog from a nixpkgs release (the only place Nix runs) |
| `packaging/` | the catalog's trusted keys, the installer, the offline installer, apps that aren't in nixpkgs |
| `action/` | the GitHub Action that bundles your app on every release |
| `examples/` | the projects, services and packages `nami new --from` starts from |
| `tests/` | the container suites, and the unit tests' fixtures |
| `docs/guide/` | the user docs, also at [namios.dev/docs](https://namios.dev/docs/nami/install) |

## Pull requests

Small fixes (a typo, a broken link, a clear bug with a test) are welcome
straight away. For anything bigger, open an issue first, so we can agree on
the approach before you spend time on it. During the alpha a lot changes from
week to week, and a pull request may need reworking or may be declined.

1. Fork the repo and make a branch.
2. Make the change, with a test: a unit test next to the code, or a check in
   the suite that covers it.
3. Run `cargo test --release` and the suites your change touches.
4. Open the pull request. The template asks what it changes and how you
   tested it.

CI ([.github/workflows/ci.yml](.github/workflows/ci.yml)) runs the unit tests,
the static build, the app's build and unit tests, and the scripts' own checks
on GitHub's runners. A pull
request that changes `packaging/apps/` also gets that app built and checked.
A maintainer approves the first run of a first-time contributor.

By sending a pull request, you agree that your contribution is licensed under
the MIT license, as the rest of nami is ([LICENSE](LICENSE)).

### Commit messages

Look at `git log` for the style:

- The first line says what's different now, in plain words, as a person
  using nami would see it: `nami undo says which change it went back to`.
  No `feat:` or `fix:` prefixes. A part's name can lead when it helps:
  `Tests: ...`, `Docs: ...`.
- Then a blank line and a few lines on why, and anything a reviewer
  should know, wrapped at about 72 characters.
- One change per commit.

### Words people read

Messages, `--help`, docs and examples follow
[docs/design-principles.md](docs/design-principles.md#6-words-on-screen):

- Plain, short sentences with concrete nouns. Say what happened and what to
  do next, without blame or jargon.
- An error names the thing that's wrong and the command that fixes it.
  `--json` errors keep their codes ([Scripts and agents](docs/guide/scripts-and-agents.md)).
- Headings in the docs say what you can do: "Pin a version", "Share builds".
- The example person is Ada (`ada`), the example project `myapp`, the example
  repo `github:ada/nami-config`.
- The files are `user.nami`, `system.nami` and `project.nami`; the flags
  `--user` and `--system`.

## In scope, out of scope

In scope: the `nami` command and its daemon, the nami app, the catalog and
how it's built, recipes, bundles and the Action, the installers, the examples
and these docs.

Out of scope:

- **Changing a package itself.** nami's packages are nixpkgs': a broken or
  outdated package is best fixed in [nixpkgs](https://github.com/NixOS/nixpkgs),
  and nami gets the fix with the next catalog.
- **A language for nami files.** They stay plain YAML: no templates,
  conditions or variables.
