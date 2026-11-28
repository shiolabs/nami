# Run anything once

Type a command you don't have, and the terminal tells you where it is:

```
› cowsay hi
cowsay isn't installed. These packages have it:
  1  cowsay      Program which generates ASCII pictures of a cow with a message
  2  neo-cowsay  Cowsay reborn, written in Go
  Run it once:  , cowsay hi
  Keep it:      nami install cowsay
Run which? [1-2, Enter for 1, n for none]
```

Press Enter and it runs right away. Nothing gets installed. When only one
package has the command, the question is just `Run it now? [Y/n]`.

**The comma.** Put `,` in front of any command to run it once: `, cowsay hi`,
`, sl`, `, jq . file.json`. This is the same as `nami try cowsay hi`. Use
`--package` to pick the package yourself: `nami try --package vim:xxd xxd`.
If you've run a command once before, `,` uses the same package again without
asking.

**The launcher.** When you search for a command-line tool, Ctrl+Enter runs it
once in a terminal ("Run once in Terminal") instead of installing it.

## How it works

- **Which package has which command.** namiOS makes a table from
  [nix-index-database](https://github.com/nix-community/nix-index-database)'s
  small database (every program in every package's `bin/`, about 1.8 MB,
  published weekly) plus the catalog's own main programs. It keeps only
  packages in this computer's catalog, with the best match first: a package
  named after the command, then one whose main program it is, then the others.
  The table is about 4.7 MB and covers about 70,000 commands. A lookup takes
  a few milliseconds and works offline.
- **Keeping it fresh.** The computer's daily update makes a new table for
  everyone once a week (`/nami/programs.tsv`). If it can't, each
  person gets their own in `~/.cache/nami/programs.tsv`, made the first time
  it's needed and refreshed in the background once it's a week old. To refresh
  it yourself, run `nami try --update-table`.
- **What runs.** The table only decides which package to suggest. What
  actually runs is the catalog's package, which comes from the signed catalog
  and the binary cache it trusts, just as with `nami install`.
- **Nothing installed.** The package is fetched into the store if it isn't
  there yet, then run with its `bin/` first on `PATH`. It isn't added to
  anyone's apps. The second time it starts at once, with no catalog read and
  no download.
- **Cleanup.** A package you ran once stays in the store until the next
  cleanup (`nami gc`, or Settings → Storage → Free up space), then it goes.
  It is never kept forever. While it's running, it's held, so a cleanup in
  the meantime doesn't pull it out from under you.
- **No admin needed.** Anyone can run things once, without sudo. nami's
  daemon fetches only what the catalog names, from caches root trusts, with
  signatures checked. A package the cache doesn't have is built from the
  catalog's own build recipe. Children on the computer can't, because an
  admin chooses their apps.
