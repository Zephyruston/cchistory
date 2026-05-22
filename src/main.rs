mod history;

use std::io::{self, IsTerminal, Read, Write};
use std::process::{Command, Stdio};

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};

use history::{Entry, History, SearchMode, format_entry};

#[derive(Parser)]
#[command(
    name = "cchistory",
    about = "Claude Code bash command history — fish-style viewer and recorder",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Show command history (default, with pager)
    Show {
        /// Max entries to show
        #[arg(short = 'n', long = "max")]
        max: Option<usize>,
        /// Show timestamps
        #[arg(short = 't', long = "show-time")]
        show_time: bool,
        /// Reverse order (oldest first)
        #[arg(short = 'R', long = "reverse")]
        reverse: bool,
    },
    /// Search for commands matching a pattern (default: contains, like fish)
    Search {
        /// Search pattern
        pattern: String,
        /// Max entries to show
        #[arg(short = 'n', long = "max")]
        max: Option<usize>,
        /// Show timestamps
        #[arg(short = 't', long = "show-time")]
        show_time: bool,
        /// Reverse order (oldest first)
        #[arg(short = 'R', long = "reverse")]
        reverse: bool,
        /// Case sensitive matching
        #[arg(short = 'C', long = "case-sensitive")]
        case_sensitive: bool,
        /// Exact match (default: contains)
        #[arg(short = 'e', long = "exact", conflicts_with = "prefix")]
        exact: bool,
        /// Prefix match (default: contains)
        #[arg(short = 'p', long = "prefix", conflicts_with = "exact")]
        prefix: bool,
    },
    /// Delete commands matching a pattern (default: exact match, like fish)
    Delete {
        /// Delete pattern
        pattern: String,
        /// Case sensitive matching
        #[arg(short = 'C', long = "case-sensitive")]
        case_sensitive: bool,
        /// Substring / contains match (opt-in; default is exact)
        #[arg(short = 'c', long = "contains", conflicts_with = "prefix")]
        contains: bool,
        /// Prefix match (opt-in; default is exact)
        #[arg(short = 'p', long = "prefix", conflicts_with = "contains")]
        prefix: bool,
    },
    /// Clear all command history
    Clear,
    /// Append a command to history (for hook use)
    Append {
        /// Command string to append
        #[arg(short = 'm', long = "command")]
        command_str: Option<String>,
        /// Working directory
        #[arg(short = 'w', long = "cwd")]
        cwd: Option<String>,
        /// Exit code
        #[arg(short = 'x', long = "exit-code")]
        exit_code: Option<i32>,
        /// Read hook JSON from stdin instead of flags
        #[arg(long = "stdin")]
        from_stdin: bool,
    },
    /// Merge history entries from another file or stdin
    Merge {
        /// Source file (reads from stdin if not provided)
        file: Option<String>,
    },
    /// Generate shell completion script
    #[command(hide = true)]
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command.unwrap_or(Commands::Show {
        max: None,
        show_time: false,
        reverse: false,
    }) {
        Commands::Show {
            max,
            show_time,
            reverse,
        } => {
            let hist = History::new()?;
            let entries = hist.read_all()?;
            let displayed = apply_limit_and_reverse(entries, reverse, max);
            display_with_pager(&displayed, show_time)?;
        }
        Commands::Search {
            pattern,
            max,
            show_time,
            reverse,
            case_sensitive,
            exact,
            prefix,
        } => {
            let hist = History::new()?;
            let mode = search_mode(exact, prefix);
            let entries = hist.search(&pattern, mode, case_sensitive)?;
            let displayed = apply_limit_and_reverse(entries, reverse, max);
            display_with_pager(&displayed, show_time)?;
        }
        Commands::Delete {
            pattern,
            case_sensitive,
            contains,
            prefix,
        } => {
            let hist = History::new()?;
            let mode = if prefix {
                SearchMode::Prefix
            } else if contains {
                SearchMode::Contains
            } else {
                SearchMode::Exact
            };
            let count = hist.delete(&pattern, mode, case_sensitive)?;
            println!("Deleted {} entries", count);
        }
        Commands::Clear => {
            let hist = History::new()?;
            hist.clear()?;
            println!("History cleared");
        }
        Commands::Append {
            command_str,
            cwd,
            exit_code,
            from_stdin,
        } => {
            if from_stdin {
                append_from_stdin()?;
            } else if let Some(cmd) = command_str {
                if cmd.is_empty() {
                    return Ok(());
                }
                let entry = Entry {
                    command: cmd,
                    when: chrono::Utc::now().timestamp(),
                    cwd,
                    exit_code,
                };
                let hist = History::new()?;
                hist.append(&entry)?;
            } else {
                anyhow::bail!("either --stdin or --command is required for append");
            }
        }
        Commands::Merge { file } => {
            let hist = History::new()?;
            let count = if let Some(ref path) = file {
                hist.merge_file(path)?
            } else if io::stdin().is_terminal() {
                anyhow::bail!(
                    "merge requires a file argument or piped input from stdin\n\
                     Usage: cchistory merge <FILE>\n\
                            some_command | cchistory merge"
                );
            } else {
                hist.merge_stdin()?
            };
            println!("Merged {} entries", count);
        }
        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            let name = cmd.get_name().to_string();
            generate(shell, &mut cmd, &name, &mut io::stdout());
        }
    }

    Ok(())
}

fn apply_limit_and_reverse(
    mut entries: Vec<Entry>,
    reverse: bool,
    max: Option<usize>,
) -> Vec<Entry> {
    // Default: newest first (reverse file order). --reverse: oldest first.
    if !reverse {
        entries.reverse();
    }
    if let Some(n) = max {
        entries.truncate(n);
    }
    entries
}

fn search_mode(exact: bool, prefix: bool) -> SearchMode {
    if exact {
        SearchMode::Exact
    } else if prefix {
        SearchMode::Prefix
    } else {
        SearchMode::Contains
    }
}

/// Read hook JSON from stdin, extract command + cwd, and append to history.
fn append_from_stdin() -> Result<()> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;

    #[derive(serde::Deserialize)]
    struct HookInput {
        #[serde(default)]
        tool_input: ToolInput,
        #[serde(default)]
        cwd: Option<String>,
    }

    #[derive(serde::Deserialize, Default)]
    struct ToolInput {
        command: Option<String>,
    }

    let hook: HookInput = serde_json::from_str(&input)?;

    let command = hook.tool_input.command.unwrap_or_default();
    if command.is_empty() {
        return Ok(());
    }

    let entry = Entry {
        command,
        when: chrono::Utc::now().timestamp(),
        cwd: hook.cwd,
        exit_code: None,
    };

    let hist = History::new()?;
    hist.append(&entry)?;
    Ok(())
}

/// Display entries through a pager if stdout is a terminal.
fn display_with_pager(entries: &[Entry], show_time: bool) -> Result<()> {
    let output = entries
        .iter()
        .enumerate()
        .map(|(i, e)| format_entry(e, i + 1, show_time))
        .collect::<Vec<_>>()
        .join("\n\n");

    let stdout = io::stdout();
    if !stdout.is_terminal() || entries.is_empty() {
        println!("{}", output);
        return Ok(());
    }

    let pager_cmd = std::env::var("PAGER").unwrap_or_else(|_| "less".into());
    // Parse the pager command (may include args, e.g. "less -R")
    let parts: Vec<&str> = pager_cmd.split_whitespace().collect();
    let (prog, args) = parts.split_first().unwrap();

    let mut pager = match Command::new(prog)
        .args(args)
        .args(["-R", "-F", "-X"])
        .stdin(Stdio::piped())
        .spawn()
    {
        Ok(p) => p,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            println!("{}", output);
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };

    if let Some(ref mut stdin) = pager.stdin {
        stdin.write_all(output.as_bytes())?;
    }
    pager.wait()?;
    Ok(())
}
