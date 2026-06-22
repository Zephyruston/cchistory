<div align="center">

# cchistory

*记录 Claude Code 执行的每一条 Bash 命令 — 给你的 AI 代理配上 fish 风格的历史记录*

[English](README.md) | [中文](README_CN.md)

[![License](https://img.shields.io/badge/License-MIT-blue?style=flat-square)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust->=1.96-3c873a?style=flat-square)](https://rust-lang.org)

[特性](#特性) • [安装](#安装) • [使用](#使用) • [工作原理](#工作原理) • [CLI 参考](#cli-参考)

</div>

`cchistory` 捕获 Claude Code 执行的每一条 Bash 命令，存入 fish 兼容的历史文件。浏览、搜索、管理你的代理命令历史 — 和你自己的 shell 历史一样自然。

多个 Claude Code 会话并发写入安全，基于 `flock` 文件锁：追加原子写入，删除操作从读取到重写全程持锁。

## 特性

- **自动记录** — 挂载到 Claude Code 的 `PostToolUse` 事件，一次配置，永久生效
- **多行命令** — heredoc、反斜杠续行等通过 YAML 字面量块标量（`|`）格式存储
- **Fish 兼容格式** — 使用与 fish 相同的 YAML 风格格式（`- cmd: ...` / `  when: ...`）
- **彩色输出** — 序号、时间戳、命令通过 `owo-colors` 着色，`less -R` 直接渲染
- **本地时区显示** — 时间戳按本机时区显示，默认最新在前
- **`less` 分页器** — stdout 是终端时自动 pipe 到 `less -R -F -X`
- **搜索与删除** — 支持包含、精确、前缀三种匹配模式，区分/忽略大小写
- **并发安全** — `flock` 读共享锁 + 写互斥锁，删除操作消除 TOCTOU 竞态窗口
- **Shell 补全** — 内置 bash、zsh、fish 补全脚本生成

## 安装

```bash
cargo install --path .
```

然后将 hook 添加到 Claude Code 设置中（全局用 `~/.claude/settings.json`，单项目用 `.claude/settings.local.json`）：

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

验证安装：

```bash
cchistory --version
```

> [!TIP]
> 也可以直接对 Claude Code 说"install"，让它通过 [CLAUDE.md](CLAUDE.md) 里的指南自动配置。

## 使用

```bash
# 浏览所有记录的命令（通过 less 分页）
cchistory
   1  git status
   2  cargo build --release
   3  gh pr create --title "Fix bug"

# 最近 20 条，带时间戳
cchistory show -n 20 -t
   1  2024-05-13 12:00:00  git status
   2  2024-05-13 12:00:01  cargo build --release

# 搜索 git 相关命令
cchistory search git

# 精确匹配
cchistory search -e "git push origin main"

# 区分大小写的前缀搜索
cchistory search -p "cargo" -C

# 删除匹配条目（默认精确匹配，与 fish 一致）
cchistory delete "rm -rf /tmp"

# 模糊匹配删除（显式开启）
cchistory delete -c "npm"

# 合并其他历史文件
cchistory merge ~/.local/share/cchistory/old-session

# 生成 fish 补全脚本
cchistory completions fish > ~/.config/fish/completions/cchistory.fish
```

## 工作原理

```
Claude Code 执行 Bash 工具
        │
        ▼
PostToolUse hook 触发
        │
        ▼
cchistory append --stdin  ◄── 从 stdin 读取 hook JSON
        │                     {"tool_input": {"command": "..."}, "cwd": "..."}
        ▼
互斥锁 → 追加写入 ~/.local/share/cchistory/history
```

Hook JSON 中包含 `tool_input.command` 和 `cwd`。`cchistory` 反序列化后补充当前时间戳，加锁追加到历史文件。

## CLI 参考

```
cchistory [COMMAND]

命令:
  show          显示命令历史（默认，通过 less 分页）
  search        搜索匹配的命令
  delete        删除匹配的命令
  clear         清空所有历史
  append        追加一条命令到历史（供 hook 使用）
  merge         从其他文件或 stdin 合并历史条目
  completions   生成 shell 补全脚本（隐藏命令）
  help          打印帮助
```

### `show`（默认）

| 标志 | 说明 |
|------|------|
| `-n, --max <N>` | 最多显示条数 |
| `-t, --show-time` | 显示时间戳 |
| `-R, --reverse` | 从旧到新排序 |

### `search <关键词>`

默认匹配模式：**包含**（不区分大小写）。

| 标志 | 说明 |
|------|------|
| `-e, --exact` | 精确匹配 |
| `-p, --prefix` | 前缀匹配 |
| `-C, --case-sensitive` | 区分大小写 |
| `-n, --max <N>` | 最多显示条数 |
| `-t, --show-time` | 显示时间戳 |
| `-R, --reverse` | 从旧到新排序 |

### `delete <关键词>`

默认匹配模式：**精确**（与 fish 一致）。

| 标志 | 说明 |
|------|------|
| `-c, --contains` | 包含/模糊匹配（显式开启） |
| `-p, --prefix` | 前缀匹配（显式开启） |
| `-C, --case-sensitive` | 区分大小写 |

### `append`

| 标志 | 说明 |
|------|------|
| `-m, --command <CMD>` | 要记录的命令 |
| `-w, --cwd <DIR>` | 工作目录 |
| `-x, --exit-code <N>` | 退出码 |
| `--stdin` | 从 stdin 读取 hook JSON |

### `clear`

删除所有历史条目，无确认提示。

### `merge [文件]`

从其他 cchistory 历史文件合并条目。支持管道输入；stdin 是终端且未指定文件时会报错。

### `completions <SHELL>`

输出 `bash`、`zsh` 或 `fish` 的补全脚本，pipe 到对应 shell 的补全目录即可。

## 历史文件格式

存储在 `$XDG_DATA_HOME/cchistory/history`（默认为 `~/.local/share/cchistory/history`）：

```
- cmd: git status
  when: 1715600000
  cwd: /home/user/project
  exit_code: 0
- cmd: cargo build --release
  when: 1715600001
  cwd: /home/user/project
```

多行命令（heredoc、反斜杠续行）使用 YAML 字面量块标量格式：

```
- cmd: |
    python3 << 'PYEOF'
    import re
    print("hello")
    PYEOF
  when: 1715600002
  cwd: /home/user/project
```

与 fish 的历史文件格式兼容 — 必要时候甚至可以直接用 fish 打开查看。

## 构建与测试

```bash
cargo build              # 开发构建 → target/debug/cchistory
cargo build --release    # 发布构建 → target/release/cchistory
cargo test               # 运行 25 个单元测试（解析、多行、搜索、锁、格式）
```
