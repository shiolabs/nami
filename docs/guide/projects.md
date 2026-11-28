---
title: "Projects"
description: "Give a project its own tools, tasks and services in a project.nami, so everyone who clones it gets the same setup."
---

## Start a project

A starter writes a project.nami with a language's compiler, tools and a first task.

```sh
nami new shop --from node     # also: c, go, python, rust, empty
```

```yaml file="shop/project.nami"
# JavaScript and TypeScript with Node.js. The tools this project needs; `nami shell` to use them.
packages:
  - nodejs
tasks:
  dev: npm run dev
  test: npm test
```

```sh
cd shop && nami shell
```

`nami new` with no `--from` lists the starters, and [Examples](https://namios.dev/docs/reference/examples) shows each in full. nami uses the `project.nami` in the folder you're in, or the nearest one above it, the way git finds `.git`.

## Give a project its tools

List the tools under `packages:`. They're there only inside the project, never among your apps.

```yaml file="project.nami"
packages:
  - nodejs: 22       # a version
  - ripgrep
env:
  NODE_ENV: development
```

A Python with its packages is one entry, with them under `with:` ([more](first-apps.md#python-with-its-packages)):

```yaml file="project.nami"
packages:
  - python3:
      version: 3.13
      with: [numpy, requests]
```

Commit `project.nami` and the `nami.lock` next to it, so whoever clones the project gets exactly the same tools. Every key a project.nami takes is in [Options](https://namios.dev/docs/reference/options).

## Work in the project

`nami shell` opens your shell with the project's tools first on `PATH`, plus its `env:` and what its services export.

```sh
nami shell
```

```text
nami shell · shop · nodejs 22.12.0, ripgrep 14.1.1 · exit to leave
```

Type `exit` to leave. You can also open a project you haven't cloned, such as `nami shell github:acme/tools@v2`.

## Load tools as you cd

nami's shell hook loads a project's tools when you `cd` into it and puts everything back when you leave.

```sh
# ~/.bashrc or ~/.zshrc
eval "$(nami hook bash)"     # zsh: nami hook zsh
```

In fish, add `nami hook fish | source` to `config.fish`. With direnv instead, put `eval "$(nami env)"` in an `.envrc`.

## Run a task

A task is a named command. Give it a description and the tasks it needs first when it says more than one line.

```yaml file="project.nami"
packages: [cargo, rustc, clippy]
tasks:
  build: cargo build
  lint: { run: cargo clippy, description: Checks the code for common mistakes }
  test:
    run: cargo test
    depends: [build, lint]    # these run first, once each
```

```sh
nami run test                # build, lint, then test
nami run test --release     # extra arguments go to test alone
nami run -- cargo fmt        # any command, with the project's tools
```

If a task fails, the ones after it don't run, and `nami run` exits with its code, so it works in CI and git hooks.

## Build C code with the project's libraries

When a project lists a compiler, a build tool or a library such as `openssl`, nami sets it up for building, like `nix develop`: headers, libraries and `pkg-config` all work.

```yaml file="project.nami"
packages: [gcc, pkg-config, openssl, zlib]
```

```sh
nami run -- sh -c 'cc app.c $(pkg-config --cflags --libs openssl zlib) -o app'
```

Add `develop: false` to turn this off, or `develop: true` to turn it on for a project that lists none of these.

## Pin the project's tools

The first time you open a project, nami writes `nami.lock`, which pins every tool to an exact build.

```sh
nami run --locked test     # in CI: fail instead of changing the lock
nami shell --upgrade       # move to today's catalog
```

[Changes and undo](changes.md) explains the lock.

## Run a service for a project

A project's services are what it needs running while you work, like a database or a mail catcher. nami runs them itself, with no containers, root or systemd.

```sh
nami new . --from postgres
```

```yaml file="project.nami" diff
  tasks:
    dev: npm run dev
    test: npm test
+ services:
+   - ./services/postgres.nami
```

That copies the service file into `services/` and lists it. A service of the project's own goes in the same list:

```yaml file="project.nami"
services:
  - ./services/postgres.nami
  - worker:
      run: npm run worker
      env: { QUEUE: default }
```

The service file gives the project `DATABASE_URL` and the other `PG` variables, and listens only on this computer. Everything services keep is in the project's `.nami` folder, which git ignores.

## Start and stop services

```sh
nami up                 # start them all and show their output, until Ctrl-C
nami up -d              # start them in the background
nami logs               # follow their output
nami project services   # each one's state
nami down               # stop them
```

Services start in the order they're listed, each once the one before is ready, and one that ends is restarted. To start over with an empty database, run `nami down` and delete its folder, such as `.nami/postgres`.

## Write a service file

nami has four service files to copy: `postgres`, `mysql`, `redis` and `mailpit`. A service file of your own needs `run:`, a command that stays in the foreground.

```yaml file="services/redis.nami"
about: Redis, the in-memory store
packages: [redis]
port: 6379                                   # nami waits for it to answer
run: redis-server --port {port} --bind 127.0.0.1 --dir {state} --daemonize no
ready: redis-cli -p {port} ping
exports:
  REDIS_URL: redis://localhost:{port}        # for the whole project
```

nami fills in `{state}` (the service's folder in `.nami`), `{port}`, `{sockets}`, `{project}` and `{project.dir}`. To change a setting where you list it, write `- ./services/redis.nami: { port: 6380 }`. The rest of the keys are in [Options](https://namios.dev/docs/reference/options#services-project-your-own).

## Share a tool from a project

A project in a public repository can offer tools it builds, for anyone to install.

```yaml
# project.nami in github.com/acme/tools
provides:
  greet: ./tools/greet       # a recipe in the repo
```

```sh
nami install github:acme/tools#greet
```

```yaml file="~/user.nami" diff
  packages:
    - firefox
+   - github:acme/tools#greet
```

Any nami file can list it the same way. A repository with a `flake.nix` and no recipe works too: an admin can install its package for everyone, as in `nami install github:numtide/treefmt`.

## Share a base between projects

A team can keep the tools, tasks and services all its projects share in one `.nami` set, and each project lists it.

```yaml file="project.nami"
packages:
  - github:acme/dev/node-service.nami   # the team's set
  - nodejs: 22
```

Where the set and the project both say something, the project wins. The set's commit is pinned in `nami.lock`, and `nami layers --update` moves it on. [Config repos and sets](config-repos.md) covers sets in full.

A project.nami can't add a binary cache, so a repository you clone can't make your computer trust a new key; see [Share builds](share-builds.md). For tokens in `env:`, see [Secrets](secrets.md).
