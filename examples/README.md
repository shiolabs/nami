# Examples

- `user.nami`: an example of a person's own file, ~/user.nami.
- `projects/`: starters for a project (node, python, rust, go, c, empty).
- `services/`: services for a project (postgres, mysql, redis, mailpit): copy one into a project's `services/` folder and list it, `- ./services/postgres.nami`.
- `packages/`: packages (a folder with package.nami each): a whole computer's parts (niri, desktop, base...), add-ons for it (tailscale, docker, steam, nvidia...), and ones nami builds (from source, from a release, a patched one). They list each other by path (`../niri`); `nami new laptop --from niri` copies one, with every package it lists, into your config repo's `shared/packages/` and writes `laptop/system.nami` listing it.

nami has no packages, projects or services of its own: everything is here, to show how you can do it. `nami new` lists what you can start from.
