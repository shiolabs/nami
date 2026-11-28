# Security

## Reporting a vulnerability

Email **hi@shiolabs.dev** with what you found, how to reproduce it, and the
nami version (`nami --version`). Please don't open a public issue or use
`nami report` for a vulnerability: reports are for bugs, and they include your
nami files.

We'll answer within a few days, keep you posted while we fix it, and credit
you when the fix is out, if you'd like. nami is in alpha and made by a small
team, so there's no bug bounty.

Only the newest release gets fixes. `nami self-update` gets it.

## What's signed, and how keys work

- **The catalog** decides what `nami install firefox` means, so nami only
  takes a catalog signed (ed25519) by a key the computer trusts. The trusted
  keys are in `/nami/catalog-keys.json`; the installer adds the
  publisher's key, and `nami update --trust <key>` adds one. The catalog
  carries the sha256 of every build definition and icon, so its one signature
  covers them all. An unsigned catalog is taken only with
  `nami update --unsigned`.
- **Packages** come from cache.nixos.org, and each one's `.narinfo` must be
  signed by a trusted cache key (cache.nixos.org's is built in); each archive
  must match the hash it names. A signed catalog may name other caches, for
  apps that aren't in nixpkgs; that adds no trust beyond the catalog's signer.
- **nami itself:** the installer, fetched over HTTPS, checks the release
  against its `SHA256SUMS` and the signature on it. `nami self-update` and the
  daily update take a release only if its `SHA256SUMS` is signed by a catalog
  key the computer already trusts.
- **Root's changes.** `sudo nami` changes the computer's file,
  `/etc/nami/system.nami`, as root. Without `sudo`, everything that needs
  root goes through `nami-daemon`: it checks who's asking (an admin, for
  `nami install --system`, with their password), and offers only operations
  that can't plant something under a path another person trusts.
- **Bundles** made with `nami bundle` take updates only when they're signed
  with the app maker's key. The first copy you run says whose key that is,
  and it's remembered (`~/.config/nami-bundle/trusted/`); an update or a new
  download signed with another key is refused.

More detail: [How it works](https://namios.dev/docs/reference/how-it-works)
and [Privacy](https://namios.dev/privacy).
