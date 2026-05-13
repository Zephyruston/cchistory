# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## About

`cchistory` — records every Bash command Claude Code executes into a fish-compatible history file (`~/.local/share/cchistory/history`). It provides a fish `history`-style CLI for viewing, searching, and managing that history. Multiple concurrent CC instances are safe via `flock` file locking.

## Install

When the user says "install" or asks to set up cchistory globally, run these two steps:

1. **Build and install the binary to `~/.cargo/bin/`:**

```bash
cargo install --path .
```

2. **Add the PostToolUse hook to global settings.** Read `~/.claude/settings.json` (create if missing), merge the following hook into the existing `hooks.PostToolUse` array (if `PostToolUse` already has entries, append; otherwise create the key):

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

Use `jq` to merge the hook config programmatically — do not overwrite unrelated settings. If `~/.claude/settings.json` doesn't exist, create it with this content.

After both steps, verify with `cchistory --version` and confirm the hook appears in `~/.claude/settings.json`.

## Build & test

```bash
cargo build              # dev build → target/debug/cchistory
cargo build --release    # release build → target/release/cchistory
cargo test               # run tests (if any)
```

20 unit tests covering parse/format round-trip, search modes (contains/exact/prefix), case sensitivity, history file operations (append/delete/clear/merge), and thread-safe temp file isolation. Manual smoke tests: `cchistory append -m "test cmd" && cchistory show`.

## Architecture

Two source files:

- **`src/main.rs`** — CLI layer. Clap derive with subcommands (`show`, `search`, `delete`, `clear`, `append`, `merge`). Default subcommand is `show`. Display pipes through `less -R -F -X` when stdout is a terminal. The `append --stdin` mode reads hook JSON from stdin (the `PostToolUse` payload), deserializes `tool_input.command` and `cwd`, then calls `History::append()`.

- **`src/history.rs`** — Storage layer. Fish-compatible YAML-like format (`- cmd: ...` / `  when: ...` / `  cwd: ...` / `  exit_code: ...`). All read operations use `flock` shared locks, all write operations use exclusive locks. `delete` holds an exclusive lock from read through write to prevent TOCTOU races. History file path: `$XDG_DATA_HOME/cchistory/history` (defaults to `~/.local/share/cchistory/history`).
