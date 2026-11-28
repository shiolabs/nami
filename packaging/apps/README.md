# Add your app to nami

nami installs nearly everything from nixpkgs. An app that isn't there can
still be in nami: send a pull request with a recipe here, and each release
builds it, signs it and serves it from nami's own cache. People find it in
the app and install it with `nami install <name>`; nothing is compiled on
their machine.

## 1. The recipe

`packaging/apps/<name>/package.nami`. `<name>` is what people type, so
lowercase, and not a name nixpkgs already has. A prebuilt Linux release is
the easiest:

```yaml
name: myapp
version: 1.2.0
source:
  url: https://github.com/me/myapp/releases/download/v1.2.0/myapp-1.2.0-linux-x86_64.tar.gz
  sha256: <sha256sum of that file>
build-deps: [autoPatchelfHook]   # points your binaries at nixpkgs' libraries
deps: [libgcc.lib]               # what they link to (the build tells you what's missing)
runtime-deps: [libglvnd, wayland, libxkbcommon]   # what they dlopen at runtime
install: |
  mkdir -p $out
  cp -r bin share $out/
```

Building from source works too (`build-deps: [cmake]`, `cargo`, ...; see
the [recipes](../../docs/guide/recipes.md) and [examples](../../examples/)).

Only a `.deb`? Then the source can be the `.deb` itself: nami unpacks it,
lays it out like a nixpkgs package and points its programs at nixpkgs'
libraries, so no `install:` is needed. `nami recipe from-deb <url>` (or an
Ubuntu package's name) writes the recipe, with the libraries it found, the
app's other packages (its data) and the Ubuntu libraries it bundles (in
`bundle:`, when nixpkgs has no fitting version); check what it marks
**NOT FOUND**, and rename it if nixpkgs has that name.
See [Package a .deb](../../docs/guide/recipes.md#package-a-deb).

Just for you, or not for everyone? A recipe works in your own config repo
too: list its folder by path, `packages: [./packages/myapp]` (see
[Keep your recipes](../../docs/guide/recipes.md#keep-your-recipes)).

## 2. What people see

nami reads it from your package, like a desktop does, so ship:

- `share/applications/<name>.desktop`: makes it an app; its `Exec=` should
  be a program in `bin/`
- an icon in `share/icons/hicolor/<size>/apps/`, named like the desktop
  file's `Icon=` (128px or bigger PNG, or SVG)
- `share/metainfo/<id>.metainfo.xml` (AppStream): `summary`,
  `project_license`, `developer/name`, `url type="homepage"`

An optional `app.nami` next to the recipe overrides `description`,
`homepage`, `license`, `publisher` or `mainProgram`.

## 3. Check it

```sh
./build.sh                                  # nami itself
packaging/check-app.sh packaging/apps/myapp
```

builds it the way a release does and prints what nami would show. The pull
request runs the same check; it has to pass with nothing marked
**missing**.

## Review

Every app is signed with nami's key, so a maintainer reads each new recipe
and each URL it downloads (the check lists them) before merging. A new
version from the same place is a quicker review: bump `version`, the URL
and `sha256`.
