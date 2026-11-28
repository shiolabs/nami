---
title: "Agents"
description: "Let an AI agent such as Claude install apps and change your setup, and ask before it changes the computer."
---

An agent works with nami the way you do: it runs `nami` as you. Your own setup, `~/user.nami`, is yours to change, so the agent can change it too. The computer's file, `/etc/nami/system.nami`, takes an administrator's password, which stays with you. For that, the agent asks, and you decide.

## Add nami to Claude Code

`nami mcp` serves nami's tools to an agent over MCP, on its standard input and output. It opens no network port.

```sh
claude mcp add nami -- nami mcp
```

Any other app that speaks MCP over stdio takes the same command. In Claude Desktop and Cursor, it's an entry in the app's MCP settings file:

```json file="mcp.json"
{
  "mcpServers": {
    "nami": { "command": "nami", "args": ["mcp"] }
  }
}
```

## What the agent can do

| Tool | What it does |
|---|---|
| `search`, `info` | Find apps and tools in the catalog |
| `get`, `check`, `show` | Read your file or the computer's, find mistakes, see the whole computer |
| `history`, `diff` | Look back at earlier changes |
| `install`, `remove` | Install or remove your apps, in `~/user.nami` |
| `config`, `apply`, `undo` | Change your `~/user.nami`, apply it, go back |
| `propose`, `proposal` | Ask to change `system.nami`, then see the decision |

Changes to your own file need no approval, as when you run `nami install` yourself. Each apply is a [change](changes.md) you can undo.

## When the agent asks to change the computer

Apps for everyone, services, drivers, the firewall and the people on the computer are in `system.nami`. The agent proposes a change, with a reason:

```sh
nami propose '[
  {"add": {"path": ["packages"], "value": "docker"}},
  {"add": {"path": ["services"], "value": "docker.socket"}}
]' --by Claude --reason "You asked me to set up Docker for your projects."
```

```text
proposal 3f9c2a71: waiting for approval
system.nami
  Apps
    + docker
  Services
    + docker.socket
Review it in Settings, or:
  nami proposals show 3f9c2a71
```

nami checks the change first, as `nami check` does, and refuses one that leaves a mistake in the file. Nothing changes yet.

`nami proposals show 3f9c2a71` gives the reason, the change in words, and the lines it changes:

```yaml file="/etc/nami/system.nami" diff
  packages:
    - firefox
+   - docker
+ services:
+   - docker.socket
```

`nami proposals apply` asks for an administrator's password, then changes the file and applies it. `nami proposals reject` discards the request. Either way, the agent is told.

## Decide in a terminal

```sh
nami proposals list            # what's waiting
nami proposals show 3f9c2a71   # the reason, the words, the lines
nami proposals apply 3f9c2a71  # asks for the password, then applies
nami proposals reject 3f9c2a71
```

An agent that wants the answer before going on adds `--wait`, and gets it as JSON:

```sh
nami propose '{"add": {"path": ["packages"], "value": "docker"}}' \
  --reason "Docker runs the project's test database." --by Claude --wait
```

```json
{"id": "3f9c2a71", "status": "applied", "by": "Claude", "message": "system.nami was changed and applied."}
```

The status is `applied`, `rejected`, `stale`, `failed`, or `pending` if `--timeout` ran out first.

## What keeps it safe

- **Only you approve.** A request is only a note in your `~/.local/state/nami/proposals`. Applying it asks for an administrator's password, there and then.
- **It's checked twice.** At apply time, nami checks the change again, against the file as it is then.
- **Stale requests are refused.** If `system.nami` changed after the agent asked, Apply refuses: "system.nami changed since Claude asked: nothing was changed. Ask for the change again."
- **No secrets.** A request can't carry a secret. Add one yourself, as in [Secrets](secrets.md).
- **You apply what you saw.** If the request changes after you looked at it, nami refuses it.
