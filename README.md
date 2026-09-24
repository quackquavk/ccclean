# ccclean

Close [Claude Code](https://claude.com/claude-code) sessions you've forgotten about in [cmux](https://cmux.com), and keep a list of what was closed so you can pick them back up later.

If you open a Claude Code session per project, you probably end up with a dozen tabs, most of them sitting idle. `ccclean` runs a small background loop inside cmux. When a session has been idle longer than your threshold, it closes the tab and logs the title, the project directory and a `claude --resume` command.

```
$ ccclean list
2026-09-23 22:40  ✳ Footer paradiso text monochrome  [Kinetico]
    dir:    ~/code/client/paradiso-web
    resume: cd /Users/you/code/client/paradiso-web && claude --resume c4b651fe-…
```

It's a single ~400 KB binary with no runtime. The watcher uses about 2 MB of RAM and sleeps between sweeps.

## Requirements

- macOS
- cmux, with its `cmux` CLI available (the app bundles it)
- Claude Code

## Install

Homebrew:

```sh
brew install quackquavk/tap/ccclean
```

Shell installer:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/quackquavk/ccclean/releases/latest/download/ccclean-installer.sh | sh
```

From source (Rust 1.85+):

```sh
cargo install --git https://github.com/quackquavk/ccclean
```

## Usage

Start the watcher from a cmux terminal:

```sh
ccclean install                        # every 15m, close sessions idle > 2h
ccclean install --every 10m --idle 1h  # custom timing
```

This opens a background cmux workspace named `ccclean` that runs `ccclean watch`. Keep it open. If you restart cmux, run `ccclean install` again; it replaces any existing watcher.

Other commands:

```sh
ccclean status                     # all sessions, idle time, status, tab title, dir
ccclean sweep --idle 2h --dry-run  # preview what would be closed
ccclean sweep --idle 2h            # one-off sweep
ccclean list [-n 50]               # closed sessions, newest first
ccclean list --clear               # empty the log
ccclean uninstall                  # stop the watcher
```

Durations accept `90s`, `30m`, `2h`, `1d`.

The log is stored at `~/.local/share/ccclean/closed.jsonl`, one JSON object per line.

## How inactivity is measured

1. Each running Claude Code process writes `~/.claude/sessions/<pid>.json` with its working directory, session ID, status (`busy` / `idle`) and when that status last changed.
2. Idle time counts from the later of two timestamps: that status change, and the timestamp of the last user or assistant message in the session's transcript (`~/.claude/projects/<dir>/<session>.jsonl`). The file's modification time isn't used, because Claude Code keeps appending bookkeeping lines to idle transcripts. Sending a message resets it.
3. Each Claude process is matched to its cmux tab by tty, using `cmux --json tree`.

A session is closed only if all of these hold:

- its status is `idle`
- its Claude process has no running shell or `caffeinate` child (a background build, dev server or long command keeps it open; `ccclean status` shows these as `bg`)
- it has been idle longer than `--idle`
- it isn't the tab you're focused on
- it isn't the tab running `ccclean`

"Closing" means closing the tab. If the session is the only tab in its workspace, only the Claude process is stopped (SIGHUP, then SIGKILL after 5s); the tab and workspace stay open at a shell prompt, since cmux won't close a workspace's last tab and closing the workspace would be too much.

Busy sessions are never closed, however long Claude works, and neither are ones waiting on a permission prompt.

Things that do **not** count as activity: viewing a tab, scrolling, or typing a prompt you haven't sent. Unsent text in the prompt is lost when the tab closes, but the conversation can still be resumed.

## Why a cmux tab and not launchd?

By default cmux only accepts socket connections from processes started inside cmux (`socketControlMode: cmuxOnly`), so a launchd job gets rejected. Running the loop in a cmux tab works without changing that security setting.

## License

MIT
