use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

/// Where history is stored: `$XDG_DATA_HOME/cchistory/history` (or the platform equivalent
/// via `dirs::data_dir()` — e.g. `~/Library/Application Support` on macOS, `%APPDATA%` on
/// Windows), defaulting to `~/.local/share/cchistory/history` on Linux.
fn history_path() -> PathBuf {
    dirs::data_dir()
        .expect("无法确定数据目录（HOME 未设）")
        .join("cchistory")
        .join("history")
}

fn ensure_parent(path: &std::path::Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub command: String,
    pub when: i64,
    pub cwd: Option<String>,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum SearchMode {
    #[default]
    Contains,
    Exact,
    Prefix,
}

pub struct History {
    path: PathBuf,
}

impl History {
    pub fn new() -> io::Result<Self> {
        let path = history_path();
        ensure_parent(&path)?;
        // Atomically create with restricted permissions if it doesn't exist
        let mut opts = OpenOptions::new();
        opts.write(true).create(true);
        #[cfg(unix)]
        {
            opts.mode(0o600);
        }
        opts.open(&path)?;
        Ok(Self { path })
    }

    /// Open the history file for reading with a shared lock.
    fn open_read(&self) -> io::Result<File> {
        let file = File::open(&self.path)?;
        file.lock_shared()?;
        Ok(file)
    }

    /// Open the history file for writing with an exclusive lock.
    fn open_write(&self) -> io::Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.path)?;
        file.lock()?;
        Ok(file)
    }

    /// Append a single entry to the history file. Uses exclusive lock.
    pub fn append(&self, entry: &Entry) -> io::Result<()> {
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.path)?;
        file.lock()?;
        write_entry(&mut file, entry)?;
        file.flush()?;
        Ok(())
    }

    /// Read all entries from the history file. Uses shared lock.
    pub fn read_all(&self) -> io::Result<Vec<Entry>> {
        let file = self.open_read()?;
        parse_all(BufReader::new(file))
    }

    /// Search for entries matching the given pattern.
    pub fn search(
        &self,
        pattern: &str,
        mode: SearchMode,
        case_sensitive: bool,
    ) -> io::Result<Vec<Entry>> {
        let entries = self.read_all()?;
        let pred = make_predicate(pattern, mode, case_sensitive);
        Ok(entries.into_iter().filter(|e| pred(e)).collect())
    }

    /// Delete entries matching the given pattern. Returns count of deleted entries.
    /// Holds an exclusive lock from read through write to prevent TOCTOU races.
    pub fn delete(
        &self,
        pattern: &str,
        mode: SearchMode,
        case_sensitive: bool,
    ) -> io::Result<usize> {
        let mut file = self.open_write()?;
        file.seek(SeekFrom::Start(0))?;
        let entries = parse_all(BufReader::new(&mut file))?;

        let pred = make_predicate(pattern, mode, case_sensitive);
        let (keep, removed): (Vec<_>, Vec<_>) = entries.into_iter().partition(|e| !pred(e));

        // Serialize to memory before truncating to avoid data loss on write error
        let mut buf = Vec::new();
        for entry in &keep {
            write_entry(&mut buf, entry)?;
        }
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&buf)?;
        file.flush()?;
        Ok(removed.len())
    }

    /// Clear all history entries.
    pub fn clear(&self) -> io::Result<()> {
        let file = self.open_write()?;
        file.set_len(0)?;
        Ok(())
    }

    /// Merge entries from a reader (e.g., another history file).
    pub fn merge<R: Read>(&self, reader: R) -> io::Result<usize> {
        let incoming = parse_all(BufReader::new(reader))?;
        let count = incoming.len();
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.path)?;
        file.lock()?;
        for entry in &incoming {
            write_entry(&mut file, entry)?;
        }
        file.flush()?;
        Ok(count)
    }

    /// Merge entries from stdin (fish history format or another cchistory file).
    pub fn merge_stdin(&self) -> io::Result<usize> {
        let stdin = io::stdin();
        self.merge(stdin.lock())
    }

    /// Load entries from a specific file (for merge from file).
    pub fn merge_file(&self, file_path: &str) -> io::Result<usize> {
        let src = std::fs::canonicalize(file_path)?;
        let dst = std::fs::canonicalize(&self.path)?;
        if src == dst {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot merge history file into itself",
            ));
        }
        let file = File::open(&src)?;
        self.merge(file)
    }
}

// ---- Parsing ----

#[allow(clippy::while_let_on_iterator)]
fn parse_all<R: BufRead>(reader: R) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let mut current = EntryBuilder::new();
    let mut lines = reader.lines();
    let mut in_block = false;

    while let Some(line) = lines.next() {
        let line = line?;

        if in_block {
            if let Some(content) = line.strip_prefix("    ") {
                current.block_lines.push(content.to_string());
                continue;
            } else if line.trim().is_empty() {
                current.block_lines.push(String::new());
                continue;
            }
            // Non-empty line without 4-space indent ends the block
            in_block = false;
        }

        if line == "- cmd: |" {
            if let Some(entry) = current.build() {
                entries.push(entry);
            }
            current = EntryBuilder::new();
            in_block = true;
        } else if let Some(stripped) = line.strip_prefix("- cmd: ") {
            if let Some(entry) = current.build() {
                entries.push(entry);
            }
            current = EntryBuilder::new();
            current.command = Some(stripped.to_string());
        } else if let Some(stripped) = line.strip_prefix("  when: ") {
            current.when = stripped.parse().ok();
        } else if let Some(stripped) = line.strip_prefix("  cwd: ") {
            current.cwd = Some(stripped.to_string());
        } else if let Some(stripped) = line.strip_prefix("  exit_code: ") {
            current.exit_code = stripped.parse().ok();
        } else if line.starts_with("  paths:") {
            // fish compat: skip paths block; re-process the first non-path line
            while let Some(Ok(next)) = lines.next() {
                if !next.starts_with("    - ") && !next.trim().is_empty() {
                    // Process the first non-path line as a top-level directive
                    if let Some(stripped) = next.strip_prefix("- cmd: ") {
                        if let Some(entry) = current.build() {
                            entries.push(entry);
                        }
                        current = EntryBuilder::new();
                        current.command = Some(stripped.to_string());
                    } else if next == "- cmd: |" {
                        if let Some(entry) = current.build() {
                            entries.push(entry);
                        }
                        current = EntryBuilder::new();
                        in_block = true;
                    } else if let Some(stripped) = next.strip_prefix("  when: ") {
                        current.when = stripped.parse().ok();
                    } else if let Some(stripped) = next.strip_prefix("  cwd: ") {
                        current.cwd = Some(stripped.to_string());
                    } else if let Some(stripped) = next.strip_prefix("  exit_code: ") {
                        current.exit_code = stripped.parse().ok();
                    }
                    break;
                }
            }
        }
    }

    if let Some(entry) = current.build() {
        entries.push(entry);
    }

    Ok(entries)
}

struct EntryBuilder {
    command: Option<String>,
    when: Option<i64>,
    cwd: Option<String>,
    exit_code: Option<i32>,
    block_lines: Vec<String>,
}

impl EntryBuilder {
    fn new() -> Self {
        Self {
            command: None,
            when: None,
            cwd: None,
            exit_code: None,
            block_lines: Vec::new(),
        }
    }

    fn build(mut self) -> Option<Entry> {
        if self.command.is_none() && !self.block_lines.is_empty() {
            self.command = Some(self.block_lines.join("\n"));
        }
        self.command.map(|cmd| Entry {
            command: cmd,
            when: self.when.unwrap_or_else(|| chrono::Utc::now().timestamp()),
            cwd: self.cwd,
            exit_code: self.exit_code,
        })
    }
}

// ---- Serialization ----

fn write_entry<W: Write>(writer: &mut W, entry: &Entry) -> io::Result<()> {
    if entry.command.contains('\n') {
        writeln!(writer, "- cmd: |")?;
        for line in entry.command.lines() {
            writeln!(writer, "    {}", line)?;
        }
        // Preserve trailing newline so round-trip is byte-for-byte
        if entry.command.ends_with('\n') {
            writeln!(writer, "    ")?;
        }
    } else {
        writeln!(writer, "- cmd: {}", entry.command)?;
    }
    writeln!(writer, "  when: {}", entry.when)?;
    if let Some(ref cwd) = entry.cwd {
        writeln!(writer, "  cwd: {}", cwd)?;
    }
    if let Some(code) = entry.exit_code {
        writeln!(writer, "  exit_code: {}", code)?;
    }
    Ok(())
}

// ---- Search ----

fn make_predicate(
    pattern: &str,
    mode: SearchMode,
    case_sensitive: bool,
) -> Box<dyn Fn(&Entry) -> bool> {
    let pattern = if case_sensitive {
        pattern.to_string()
    } else {
        pattern.to_lowercase()
    };

    Box::new(move |entry: &Entry| {
        let target = entry.command.clone();
        let target = if case_sensitive {
            target
        } else {
            target.to_lowercase()
        };

        match mode {
            SearchMode::Contains => target.contains(&pattern),
            SearchMode::Exact => target == pattern,
            SearchMode::Prefix => target.starts_with(&pattern),
        }
    })
}

// ---- Formatting for display ----

use owo_colors::OwoColorize as _;

pub fn format_entry(entry: &Entry, index: usize, show_time: bool) -> String {
    let num = format!("{:>4}", index).dimmed().to_string();
    let cmd = entry.command.default_color().to_string();

    if show_time {
        let dt = chrono::DateTime::from_timestamp(entry.when, 0)
            .map(|dt| {
                dt.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()
            })
            .unwrap_or_else(|| entry.when.to_string());
        let ts = dt.cyan().to_string();
        format!("{}  {}  {}", num, ts, cmd)
    } else {
        format!("{}  {}", num, cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(s: &str) -> Vec<Entry> {
        parse_all(BufReader::new(s.as_bytes())).unwrap()
    }

    fn write_to_string(entries: &[Entry]) -> String {
        let mut buf = Vec::new();
        for e in entries {
            write_entry(&mut buf, e).unwrap();
        }
        String::from_utf8(buf).unwrap()
    }

    const SAMPLE_HISTORY: &str = "\
- cmd: git status
  when: 1715600000
  cwd: /home/user/project
  exit_code: 0
- cmd: Cargo Build
  when: 1715600001
  cwd: /home/user/project
- cmd: ls -la
  when: 1715600002
  exit_code: 0
- cmd: git push origin main
  when: 1715600003
  cwd: /home/user/project
  exit_code: 0
";

    #[test]
    fn parse_fish_format() {
        let entries = parse_str(SAMPLE_HISTORY);
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].command, "git status");
        assert_eq!(entries[0].when, 1715600000);
        assert_eq!(entries[0].cwd.as_deref(), Some("/home/user/project"));
        assert_eq!(entries[0].exit_code, Some(0));
        assert_eq!(entries[2].command, "ls -la");
        assert_eq!(entries[2].cwd, None);
    }

    #[test]
    fn parse_empty() {
        let entries = parse_str("");
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn parse_missing_when() {
        let entries = parse_str("- cmd: echo hello\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].command, "echo hello");
        // when defaults to current timestamp, so just verify it's > 0
        assert!(entries[0].when > 0);
    }

    #[test]
    fn write_round_trip() {
        let original = vec![
            Entry {
                command: "git status".into(),
                when: 1715600000,
                cwd: Some("/home/user/project".into()),
                exit_code: Some(0),
            },
            Entry {
                command: "ls -la".into(),
                when: 1715600001,
                cwd: None,
                exit_code: None,
            },
        ];
        let serialized = write_to_string(&original);
        let parsed = parse_str(&serialized);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].command, "git status");
        assert_eq!(parsed[0].when, 1715600000);
        assert_eq!(parsed[0].cwd.as_deref(), Some("/home/user/project"));
        assert_eq!(parsed[0].exit_code, Some(0));
        assert_eq!(parsed[1].command, "ls -la");
        assert_eq!(parsed[1].cwd, None);
        assert_eq!(parsed[1].exit_code, None);
    }

    #[test]
    fn write_round_trip_multiline_heredoc() {
        let original = vec![Entry {
            command: "python3 << 'PYEOF'\nimport re\nprint('hello')\nPYEOF".into(),
            when: 1715600000,
            cwd: Some("/home/user/project".into()),
            exit_code: Some(0),
        }];
        let serialized = write_to_string(&original);
        let parsed = parse_str(&serialized);
        assert_eq!(parsed.len(), 1);
        assert_eq!(
            parsed[0].command,
            "python3 << 'PYEOF'\nimport re\nprint('hello')\nPYEOF"
        );
        assert_eq!(parsed[0].when, 1715600000);
        assert_eq!(parsed[0].cwd.as_deref(), Some("/home/user/project"));
        assert_eq!(parsed[0].exit_code, Some(0));
    }

    #[test]
    fn write_round_trip_multiline_backslash() {
        let original = vec![Entry {
            command: "sed -i \\\n  -e 's/foo/bar/g' \\\n  output.md".into(),
            when: 1715600000,
            cwd: None,
            exit_code: None,
        }];
        let serialized = write_to_string(&original);
        let parsed = parse_str(&serialized);
        assert_eq!(parsed.len(), 1);
        assert_eq!(
            parsed[0].command,
            "sed -i \\\n  -e 's/foo/bar/g' \\\n  output.md"
        );
        assert_eq!(parsed[0].when, 1715600000);
    }

    #[test]
    fn parse_mixed_single_and_multiline() {
        let input = "\
- cmd: git status
  when: 1
- cmd: |
    echo hello
    echo world
  when: 2
- cmd: ls -la
  when: 3
";
        let entries = parse_str(input);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].command, "git status");
        assert_eq!(entries[1].command, "echo hello\necho world");
        assert_eq!(entries[2].command, "ls -la");
    }

    #[test]
    fn write_round_trip_multiline_with_empty_lines() {
        let original = vec![Entry {
            command: "python3 << 'EOF'\n\nprint('hello')\n\nEOF".into(),
            when: 1715600000,
            cwd: None,
            exit_code: None,
        }];
        let serialized = write_to_string(&original);
        let parsed = parse_str(&serialized);
        assert_eq!(parsed.len(), 1);
        assert_eq!(
            parsed[0].command,
            "python3 << 'EOF'\n\nprint('hello')\n\nEOF"
        );
    }

    #[test]
    fn single_line_pipe_character_not_confused_with_block() {
        // A command containing a pipe should NOT trigger block mode
        let input = "\
- cmd: echo foo | bar
  when: 1
";
        let entries = parse_str(input);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].command, "echo foo | bar");
    }

    // -- Search mode tests --

    #[test]
    fn search_contains_default() {
        let pred = make_predicate("git", SearchMode::Contains, false);
        let entry = Entry {
            command: "git status".into(),
            when: 1,
            cwd: None,
            exit_code: None,
        };
        assert!(pred(&entry));
        let entry2 = Entry {
            command: "cargo build".into(),
            when: 1,
            cwd: None,
            exit_code: None,
        };
        assert!(!pred(&entry2));
    }

    #[test]
    fn search_exact() {
        let pred = make_predicate("git status", SearchMode::Exact, false);
        assert!(pred(&Entry {
            command: "git status".into(),
            when: 1,
            cwd: None,
            exit_code: None
        }));
        assert!(pred(&Entry {
            command: "GIT STATUS".into(),
            when: 1,
            cwd: None,
            exit_code: None
        }));
        assert!(!pred(&Entry {
            command: "git status -v".into(),
            when: 1,
            cwd: None,
            exit_code: None
        }));
    }

    #[test]
    fn search_exact_case_sensitive() {
        let pred = make_predicate("git status", SearchMode::Exact, true);
        assert!(pred(&Entry {
            command: "git status".into(),
            when: 1,
            cwd: None,
            exit_code: None
        }));
        assert!(!pred(&Entry {
            command: "GIT STATUS".into(),
            when: 1,
            cwd: None,
            exit_code: None
        }));
    }

    #[test]
    fn search_prefix() {
        let pred = make_predicate("git", SearchMode::Prefix, false);
        assert!(pred(&Entry {
            command: "git status".into(),
            when: 1,
            cwd: None,
            exit_code: None
        }));
        assert!(pred(&Entry {
            command: "git push".into(),
            when: 1,
            cwd: None,
            exit_code: None
        }));
        assert!(!pred(&Entry {
            command: "cargo git".into(),
            when: 1,
            cwd: None,
            exit_code: None
        }));
    }

    #[test]
    fn search_contains_case_insensitive() {
        let pred = make_predicate("BUILD", SearchMode::Contains, false);
        let entries = parse_str(SAMPLE_HISTORY);
        let matches: Vec<_> = entries.iter().filter(|e| pred(e)).collect();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].command, "Cargo Build");
    }

    #[test]
    fn search_contains_case_sensitive() {
        let pred = make_predicate("BUILD", SearchMode::Contains, true);
        let entries = parse_str(SAMPLE_HISTORY);
        let matches: Vec<_> = entries.iter().filter(|e| pred(e)).collect();
        assert_eq!(matches.len(), 0);

        let pred = make_predicate("Build", SearchMode::Contains, true);
        let matches: Vec<_> = entries.iter().filter(|e| pred(e)).collect();
        assert_eq!(matches.len(), 1);
    }

    // -- Formatting tests --

    #[test]
    fn format_entry_without_time() {
        let entry = Entry {
            command: "git status".into(),
            when: 1715600000,
            cwd: None,
            exit_code: None,
        };
        let formatted = format_entry(&entry, 1, false);
        assert!(formatted.contains("git status"));
        assert!(formatted.contains("1"));
    }

    #[test]
    fn format_entry_with_time() {
        let entry = Entry {
            command: "git status".into(),
            when: 1715600000,
            cwd: None,
            exit_code: None,
        };
        let formatted = format_entry(&entry, 1, true);
        assert!(formatted.contains("git status"));
        assert!(
            formatted.contains("2024-05-13"),
            "expected datetime, got: {}",
            formatted
        );
    }

    // -- History file operation tests (use temp file) --

    fn temp_history() -> (History, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history");
        let hist = History::with_path(path).unwrap();
        (hist, dir)
    }

    // We need a way to create History with a specific path for testing.
    // Add a test-only constructor.

    impl History {
        #[cfg(test)]
        pub fn with_path(path: std::path::PathBuf) -> io::Result<Self> {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut opts = OpenOptions::new();
            opts.write(true).create(true);
            #[cfg(unix)]
            {
                opts.mode(0o600);
            }
            opts.open(&path)?;
            Ok(Self { path })
        }
    }

    #[test]
    fn append_and_read() {
        let (hist, _dir) = temp_history();
        let entry = Entry {
            command: "echo test".into(),
            when: 1715600000,
            cwd: Some("/tmp".into()),
            exit_code: Some(0),
        };
        hist.append(&entry).unwrap();
        let entries = hist.read_all().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].command, "echo test");
        assert_eq!(entries[0].cwd.as_deref(), Some("/tmp"));
    }

    #[test]
    fn search_in_history() {
        let (hist, _dir) = temp_history();
        for cmd in &["git status", "cargo build", "git push"] {
            hist.append(&Entry {
                command: cmd.to_string(),
                when: 1715600000,
                cwd: None,
                exit_code: None,
            })
            .unwrap();
        }
        let results = hist.search("git", SearchMode::Contains, false).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_exact_in_history() {
        let (hist, _dir) = temp_history();
        for cmd in &["git status", "git status -v", "cargo build"] {
            hist.append(&Entry {
                command: cmd.to_string(),
                when: 1715600000,
                cwd: None,
                exit_code: None,
            })
            .unwrap();
        }
        let results = hist.search("git status", SearchMode::Exact, false).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].command, "git status");
    }

    #[test]
    fn delete_entries() {
        let (hist, _dir) = temp_history();
        for cmd in &["git status", "npm install", "git push"] {
            hist.append(&Entry {
                command: cmd.to_string(),
                when: 1715600000,
                cwd: None,
                exit_code: None,
            })
            .unwrap();
        }
        let count = hist.delete("git", SearchMode::Contains, false).unwrap();
        assert_eq!(count, 2);
        let remaining = hist.read_all().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].command, "npm install");
    }

    #[test]
    fn delete_default_is_exact() {
        let (hist, _dir) = temp_history();
        for cmd in &["git status", "git status -v", "cargo build"] {
            hist.append(&Entry {
                command: cmd.to_string(),
                when: 1715600000,
                cwd: None,
                exit_code: None,
            })
            .unwrap();
        }
        // Exact match (default for delete)
        let count = hist.delete("git status", SearchMode::Exact, false).unwrap();
        assert_eq!(count, 1);
        let remaining = hist.read_all().unwrap();
        assert_eq!(remaining.len(), 2);
    }

    #[test]
    fn clear_history() {
        let (hist, _dir) = temp_history();
        for cmd in &["cmd1", "cmd2", "cmd3"] {
            hist.append(&Entry {
                command: cmd.to_string(),
                when: 1715600000,
                cwd: None,
                exit_code: None,
            })
            .unwrap();
        }
        hist.clear().unwrap();
        let entries = hist.read_all().unwrap();
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn merge_entries() {
        let (hist, _dir) = temp_history();
        // Add one entry directly
        hist.append(&Entry {
            command: "existing".into(),
            when: 1,
            cwd: None,
            exit_code: None,
        })
        .unwrap();
        // Merge two more from a reader
        let incoming = "- cmd: merged_one\n  when: 2\n- cmd: merged_two\n  when: 3\n";
        let count = hist.merge(std::io::Cursor::new(incoming)).unwrap();
        assert_eq!(count, 2);
        let entries = hist.read_all().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].command, "existing");
        assert_eq!(entries[1].command, "merged_one");
        assert_eq!(entries[2].command, "merged_two");
    }

    #[test]
    fn history_path_resolves() {
        let path = history_path();
        assert!(path.to_string_lossy().contains("cchistory"));
        assert!(path.to_string_lossy().contains("history"));
    }
}
