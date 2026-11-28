---
title: "Your editor"
description: "Get key completion, docs on hover and mistakes marked as you type, in any editor with the YAML language server."
---

## Add a schema to one file

nami publishes a schema for each kind of file. Put a first line like this in a file, and your editor completes keys and marks what nami would refuse.

```yaml
# yaml-language-server: $schema=https://namios.dev/schema/computer.json
hostname: laptop
packages:
  - namios
```

Each kind of file has its own schema:

| File | Schema |
|---|---|
| `system.nami` | `https://namios.dev/schema/computer.json` |
| `user.nami` | `https://namios.dev/schema/user.json` |
| `project.nami` | `https://namios.dev/schema/project.json` |
| `package.nami` | `https://namios.dev/schema/package.json` |
| `checks.nami` | `https://namios.dev/schema/checks.json` |
| A project's service file, like `services/postgres.nami` | `https://namios.dev/schema/service.json` |
| A set, any other `.nami` a file lists | `https://namios.dev/schema/set.json` |

A set can have any name, so give it its first line. `nami options --schema computer` prints the schema your nami has.

## Set up VS Code

Install Red Hat's YAML extension, then add this to your settings.

```json
{
  "files.associations": { "*.nami": "yaml" },
  "yaml.customTags": ["!secret scalar"],
  "yaml.schemas": {
    "https://namios.dev/schema/computer.json": "**/system.nami",
    "https://namios.dev/schema/user.json": "**/user.nami",
    "https://namios.dev/schema/package.json": "**/package.nami",
    "https://namios.dev/schema/project.json": "**/project.nami",
    "https://namios.dev/schema/checks.json": "**/checks.nami",
    "https://namios.dev/schema/service.json": "**/services/*.nami"
  }
}
```

`yaml.customTags` lets [secrets](secrets.md) through.

## Set up Helix or Neovim

- **Helix:** in `languages.toml`, add `{ glob = "*.nami" }` to yaml's `file-types`, and the same map under `[language-server.yaml-language-server.config.yaml.schemas]`.
- **Neovim:** `vim.filetype.add({ extension = { nami = "yaml" } })`, and the same map in `yamlls`'s `settings.yaml.schemas`.

## Check what the schema can't

The schema knows each file's keys and the shape of their values, but not the catalog or your secrets. `nami check` does, so run it before you apply.

```sh
nami check system.nami
```
