# ccclean: auto-close idle Claude Code sessions in cmux

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform: macOS](https://img.shields.io/badge/platform-macOS-lightgrey.svg)](#requirements)
[![Written in Rust](https://img.shields.io/badge/written%20in-Rust-orange.svg)](https://www.rust-lang.org)

ccclean is a small command-line tool that finds forgotten [Claude Code](https://claude.com/claude-code) sessions in the [cmux](https://cmux.com) terminal and closes them after a period of inactivity. Every session it closes goes into a log with the tab title, the project directory and a `claude --resume` command, so you can pick the work back up later.

If you run one Claude Code agent per project, you know how this goes. By Friday there are fifteen tabs, twelve of them idle since Tuesday, each holding a few hundred MB of RAM plus its MCP servers. ccclean closes the ones you stopped using and keeps a list of what they were.

```
$ ccclean list
2026-09-23 22:40  ✳ Footer paradiso text monochrome  [Kinetico]
    dir:    ~/code/client/paradiso-web
    resume: cd /Users/you/code/client/paradiso-web && claude --resume c4b651fe-…
```

It's a single 400 KB binary. The background watcher uses about 2 MB of RAM and sleeps between sweeps.

## Features

- Closes Claude Code sessions after a configurable idle timeout (default 2 hours)
- Never closes a session while Claude is working, including long autonomous runs, background builds and dev servers
- Never closes the tab you're looking at
- Logs the title, workspace, project directory and resume command for every closed session
- `ccclean status` shows every running Claude Code session with its idle time
- Leaves cmux's security settings alone. The watcher runs inside cmux, so the default `cmuxOnly` socket mode keeps working.

## Requirements

- macOS (Apple Silicon or Intel)
- [cmux](https://cmux.com). The `cmux` CLI ships inside the app.
- [Claude Code](https://claude.com/claude-code)

## Install

With Homebrew:

```sh
brew install quackquavk/tap/ccclean
```

With the shell installer:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/quackquavk/ccclean/releases/latest/download/ccclean-installer.sh | sh
```

From source (Rust 1.85 or newer):

```sh
cargo install --git https://github.com/quackquavk/ccclean
```

## Quick start

Run this from any cmux terminal:

```sh
ccclean install
```

That opens a cmux workspace named `ccclean` running `ccclean watch`. Every 15 minutes it closes Claude Code sessions that have been idle for more than 2 hours. Keep that workspace open. After restarting cmux, run `ccclean install` again. It replaces the old watcher, so running it twice is harmless.

Change the timing:

```sh
ccclean install --every 10m --idle 1h
```

## Commands

| Command | What it does |
| --- | --- |
| `ccclean status` | Lists running Claude Code sessions with idle time, status, tab title and directory |
| `ccclean sweep --idle 2h --dry-run` | Shows what would be closed |
| `ccclean sweep --idle 2h` | Runs one sweep now |
| `ccclean list [-n 50]` | Shows closed sessions, newest first |
| `ccclean list --clear` | Empties the log |
| `ccclean install [--every 15m] [--idle 2h]` | Starts the watcher in its own cmux workspace |
| `ccclean uninstall` | Stops the watcher |
| `ccclean watch` | Runs the sweep loop in the current terminal |

Durations accept `90s`, `30m`, `2h` and `1d`. The log lives at `~/.local/share/ccclean/closed.jsonl`, one JSON object per line.

## How ccclean detects an idle Claude Code session

1. Each running Claude Code process writes `~/.claude/sessions/<pid>.json` with its working directory, session ID, status (`busy` or `idle`) and the time that status last changed.
2. ccclean reads the timestamp of the last user or assistant message in the session transcript (`~/.claude/projects/<dir>/<session>.jsonl`). It ignores the file's modification time, because Claude Code keeps appending bookkeeping lines to idle transcripts.
3. Idle time counts from whichever of those two timestamps is later.
4. `cmux --json tree` gives each tab's tty, which ccclean matches to the Claude process.

A session gets closed only if all of these are true:

- its status is `idle`
- the Claude process has no running shell or `caffeinate` child. A background build, dev server or long command keeps the session open, and `ccclean status` shows it as `bg`.
- it has been idle longer than `--idle`
- it isn't the tab you're focused on
- it isn't the tab running ccclean

Closing a session closes its tab. When the session is the only tab in its workspace, ccclean stops just the Claude process (SIGHUP, then SIGKILL after 5 seconds) and leaves the tab open at a shell prompt. cmux refuses to close a workspace's last tab, and closing the whole workspace would take out more than you asked for.

Viewing a tab, scrolling and typing a prompt you haven't sent don't count as activity. Unsent prompt text is lost when the tab closes. The conversation itself can still be resumed.

## FAQ

### Will it close Claude Code while it's running a long task?

No. A working session reports status `busy`, and ccclean skips busy sessions however long they run. Background commands started with `run_in_background` also keep a session open.

### How do I get a closed session back?

Run `ccclean list` and paste the `resume` line. It changes into the project directory and runs `claude --resume <session-id>`, which reopens the full conversation.

### Why doesn't it use launchd or cron?

cmux only accepts socket connections from processes started inside cmux (`socketControlMode: cmuxOnly`, the default), so a launchd job gets rejected. Running the loop in a cmux tab avoids opening up that setting.

### Does it work with tmux, iTerm2 or Ghostty?

Not yet. ccclean relies on cmux's CLI to map Claude processes to tabs and close them. Supporting tmux would need a different backend, and pull requests are welcome.

### Does it work with /loop or scheduled tasks?

Only if each run happens before the idle timeout. A loop that wakes Claude every 30 minutes resets the clock each time. For longer schedules, raise the timeout, for example `ccclean install --idle 6h`.

### How much memory does it use?

About 2 MB for the watcher process. A sweep takes around 100 ms and shells out to `cmux` and `ps` once each.

## Contributing

Issues and pull requests are welcome at [github.com/quackquavk/ccclean](https://github.com/quackquavk/ccclean/issues). The whole tool is one file, `src/main.rs`.

## License

MIT. See [LICENSE](LICENSE).
