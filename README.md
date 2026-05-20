<div align="center">

# cchistory

*Record every Bash command Claude Code runs — fish-style history for your AI agent*

[English](README.md) | [中文](README_CN.md)

[![License](https://img.shields.io/badge/License-MIT-blue?style=flat-square)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust->=1.85-3c873a?style=flat-square)](https://rust-lang.org)

[Features](#features) • [Installation](#installation) • [Usage](#usage) • [How it works](#how-it-works) • [CLI Reference](#cli-reference)

</div>

`cchistory` captures every Bash command executed by Claude Code and stores them in a fish-compatible history file. Browse, search, and manage your agent's command history — just like you would your own shell history.

Multiple concurrent Claude Code sessions are safe via `flock` file locking; each append is an atomic write, each delete holds the lock from read through rewrite.

## Features

- **Automatic recording** — hook into Claude Code's `PostToolUse` event, no manual steps after setup
- **Multi-line commands** — heredocs, backslash continuations stored via YAML block scalar (`|`) format
- **Fish-compatible format** — history stored in the same YAML-like format fish uses (`- cmd: ...` / `  when: ...`)
- **Colored output** — line numbers, timestamps, and commands colorized via `owo-colors`; renders in `less -R`
- **Local timezone display** — timestamps shown in your machine's local time, newest-first by default
- **`less` pager** — output pipes through `less -R -F -X` when stdout is a terminal
- **Search & delete** — contains, exact, and prefix matching; case-sensitive or insensitive
- **Concurrency-safe** — `flock` shared locks for reads, exclusive locks for writes; TOCTOU-free deletes
- **Shell completions** — built-in generation for bash, zsh, and fish

## Installation

```bash
cargo install --path .
```

Then add the hook to your Claude Code settings (`~/.claude/settings.json` for global, or `.claude/settings.local.json` for per-project):

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "cchistory",
            "args": ["append", "--stdin"]
          }
        ]
      }
    ]
  }
}
```

Verify it's working:

```bash
cchistory --version
```

> [!TIP]
> Use `cchistory install` (TODO) to automatically set up both the binary and the global hook.
> For now, follow the [CLAUDE.md](CLAUDE.md) guide — say "install" to Claude Code to have it configured for you.

## Usage

```bash
# Browse all recorded commands (opens in less pager)
cchistory
   1  git status
   2  cargo build --release
   3  gh pr create --title "Fix bug"

# Show the last 20 entries with timestamps
cchistory show -n 20 -t
   1  2024-05-13 12:00:00  git status
   2  2024-05-13 12:00:01  cargo build --release

# Search for git commands
cchistory search git

# Exact-match search
cchistory search -e "git push origin main"

# Case-sensitive prefix search
cchistory search -p "cargo" -C

# Delete matching entries (exact by default, like fish)
cchistory delete "rm -rf /tmp"

# Delete with contains match (opt-in)
cchistory delete -c "npm"

# Merge history from another file
cchistory merge ~/.local/share/cchistory/old-session

# Generate fish shell completions
cchistory completions fish > ~/.config/fish/completions/cchistory.fish
```

## How it works

```
Claude Code runs Bash tool
        │
        ▼
PostToolUse hook fires
        │
        ▼
cchistory append --stdin  ◄── reads hook JSON from stdin
        │                     {"tool_input": {"command": "..."}, "cwd": "..."}
        ▼
exclusive flock → append to ~/.local/share/cchistory/history
```

The hook JSON contains `tool_input.command` and `cwd` from the Claude Code session. `cchistory` deserializes it, builds an entry with the current timestamp, and appends it under a file lock.

## CLI Reference

```
cchistory [COMMAND]

Commands:
  show          Show command history (default, with pager)
  search        Search for commands matching a pattern
  delete        Delete commands matching a pattern
  clear         Clear all command history
  append        Append a command to history (for hook use)
  merge         Merge history entries from another file or stdin
  completions   Generate shell completion script (hidden)
  help          Print help
```

### `show` (default)

| Flag | Description |
|------|-------------|
| `-n, --max <N>` | Max entries to show |
| `-t, --show-time` | Show timestamps |
| `-R, --reverse` | Oldest first |

### `search <PATTERN>`

Default match mode: **contains** (case-insensitive).

| Flag | Description |
|------|-------------|
| `-e, --exact` | Exact match |
| `-p, --prefix` | Prefix match |
| `-C, --case-sensitive` | Case sensitive |
| `-n, --max <N>` | Max entries to show |
| `-t, --show-time` | Show timestamps |
| `-R, --reverse` | Oldest first |

### `delete <PATTERN>`

Default match mode: **exact** (like fish).

| Flag | Description |
|------|-------------|
| `-c, --contains` | Substring match (opt-in) |
| `-p, --prefix` | Prefix match (opt-in) |
| `-C, --case-sensitive` | Case sensitive |

### `append`

| Flag | Description |
|------|-------------|
| `-m, --command <CMD>` | Command string to record |
| `-w, --cwd <DIR>` | Working directory |
| `-x, --exit-code <N>` | Exit code |
| `--stdin` | Read hook JSON from stdin |

### `clear`

Removes all history entries. No confirmation prompt.

### `merge [FILE]`

Merges entries from another cchistory file. Reads from stdin when piped; shows an error if no file is given and stdin is a terminal.

### `completions <SHELL>`

Prints completion script for `bash`, `zsh`, or `fish`. Pipe to your shell's completions directory.

## History file format

Stored at `$XDG_DATA_HOME/cchistory/history` (defaults to `~/.local/share/cchistory/history`):

```
- cmd: git status
  when: 1715600000
  cwd: /home/user/project
  exit_code: 0
- cmd: cargo build --release
  when: 1715600001
  cwd: /home/user/project
```

Multi-line commands (heredocs, backslash continuations) use YAML literal block scalar:

```
- cmd: |
    python3 << 'PYEOF'
    import re
    print("hello")
    PYEOF
  when: 1715600002
  cwd: /home/user/project
```

Compatible with fish's history file format — you can even point fish at it for debugging.

## Build & test

```bash
cargo build              # dev build → target/debug/cchistory
cargo build --release    # release build → target/release/cchistory
cargo test               # run 25 unit tests (parsing, multi-line, search, locking, format)
```
