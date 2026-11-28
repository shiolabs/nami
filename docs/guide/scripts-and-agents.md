---
title: "Scripts and agents"
description: "Read nami's output as JSON, handle its errors by code, and run it with nobody to answer questions."
---

## Get JSON

Add `--json` to any command whose output a program reads. It prints one JSON value on stdout, and nothing else there.

```sh
nami search ripgrep --json        # [{"attr": "ripgrep", "version": "14.1.1", ...}]
nami list --json                  # what's installed
nami status --json                # who you are to nami, your change, the catalog
nami history --json               # every change, to go back to
nami check user.nami --json       # problems in a file, by line
nami apply --dry-run --json       # what would change, without changing it
```

Commands that only change something, such as `install`, `remove`, `apply`, `undo` and `update`, print `{"ok": true}` when they're done. What they say along the way goes to stderr.

These also take `--json`: `info`, `diff`, `doctor`, `upgrade --dry-run`, `gc --dry-run`, `layers`, `show`, `options`, `config get`, `project list`, `project tasks`, `project services`, `backup list`, `backup status`, `sync status`, `home diff`, `package-info`, `file-apps`, `new`, and every `nami desktop` command.

## Handle errors

With `--json`, a command that fails prints an error object on stdout, and its plain message on stderr as always:

```json
{"error": {"code": "not_found", "message": "unknown package \"rigrep\"; did you mean: ripgrep", "hint": "`nami search rigrep` looks for it."}}
```

`hint` is there only when there's something to suggest. Match on `code`; the message is for people and may change. The codes stay the same from one nami to the next:

| Code | What happened |
|---|---|
| `invalid_args` | The command line or a value on it is wrong. |
| `not_found` | A package, file, change, window, network or page isn't there. |
| `invalid_file` | A nami file has an error in it. `nami check` lists them by line. |
| `needs_admin` | It changes the computer for everyone: an administrator's password is needed. |
| `needs_input` | It would ask a question and nobody can answer. Pass the answer as a flag. |
| `network` | No connection, or a download failed. Trying again later may work. |
| `conflict` | Something else is changing it, it changed meanwhile, or it already exists. |
| `denied` | Refused: a wrong password, or not allowed. |
| `not_running` | The namiOS desktop isn't running in this session. |
| `unsupported` | Not on this computer: no hardware for it, or not namiOS. |
| `failed` | Anything else. |

The exit code says the same, with or without `--json`:

| Exit code | Meaning |
|---|---|
| `0` | It worked. |
| `1` | It failed (`failed`, `not_found`, `invalid_file` and the rest). |
| `2` | The command line is wrong (`invalid_args`). |
| `75` | The network (`network`): try again later. |
| `77` | An administrator is needed (`needs_admin`). |

## Run with nobody to answer

Set `NAMI_NONINTERACTIVE=1` and nothing waits for an answer, in the terminal or in a window. A command that would ask fails with `needs_input`, and its hint names the flag that answers it:

```sh
export NAMI_NONINTERACTIVE=1
nami report                       # needs_input: pass --message "..."
nami report --message "It froze"  # needs_input: pass --yes
```

Without a terminal, nami asks nothing in one either, but a change to the computer's file can still bring up a password box on the desktop. With `NAMI_NONINTERACTIVE` set, no box comes up: the command fails with `needs_admin`, unless an administrator's password was given in the last few minutes.


## Tell your agent about nami

`nami new <folder> --from <starter>` writes an `AGENTS.md` next to `project.nami`. It tells coding agents how to run the project's tasks, add a tool, and read nami's JSON. A file that's already there is kept.

For an overview of nami to give a language model, see [/llms.txt](https://namios.dev/llms.txt).
