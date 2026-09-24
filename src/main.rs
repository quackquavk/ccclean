//! ccclean — close Claude Code sessions in cmux that have been idle too long,
//! and keep a log (title, project dir, resume command) so you can revisit them.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, exit};
use std::time::{SystemTime, UNIX_EPOCH};

const CMUX_FALLBACK: &str = "/Applications/cmux.app/Contents/Resources/bin/cmux";

struct Session {
    pid: i32,
    session_id: String,
    name: String,
    cwd: String,
    status: String,
    last_active: u64, // epoch secs
    tty: String,
    surface: Option<Surface>,
    working: bool, // has a running shell/caffeinate child
}

#[derive(Clone)]
struct Surface {
    surface_id: String,
    workspace_id: String,
    surface_ref: String,
    workspace_title: String,
    title: String,
    active: bool,
    only_in_workspace: bool, // cmux refuses to close a workspace's last surface
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let rest = &args[args.len().min(1)..];
    match cmd {
        "sweep" => sweep(rest),
        "status" | "ls" => status(),
        "list" | "log" => list(rest),
        "install" => install(rest),
        "watch" => watch(rest),
        "uninstall" => uninstall(),
        "--version" | "-V" | "version" => println!("ccclean {}", env!("CARGO_PKG_VERSION")),
        _ => help(),
    }
}

fn help() {
    println!(
        "ccclean — close idle Claude Code sessions in cmux and log them

USAGE:
  ccclean status                      show running sessions and idle time
  ccclean sweep [--idle 2h] [--dry-run]
                                      close sessions idle longer than --idle
  ccclean list [-n 20] [--clear]      show closed sessions (newest first)
  ccclean watch [--every 15m] [--idle 2h]
                                      sweep forever (must run inside cmux)
  ccclean install [--every 15m] [--idle 2h]
                                      start `watch` in its own cmux workspace
  ccclean uninstall                   close the watcher workspace

Durations: 90s, 30m, 2h, 1d. Only sessions whose status is 'idle' are closed;
busy ones, ones still running a background command (STATUS 'bg'), and the
surface you're currently focused on are never touched.
Log: {}",
        log_path().display()
    );
}

// ---------- args ----------

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn parse_dur(s: &str) -> u64 {
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: u64 = num.parse().unwrap_or_else(|_| die(&format!("bad duration: {s}")));
    n * match unit {
        "" | "m" => 60,
        "s" => 1,
        "h" => 3600,
        "d" => 86400,
        _ => die(&format!("bad duration unit: {s}")),
    }
}

fn die(msg: &str) -> ! {
    eprintln!("ccclean: {msg}");
    exit(1)
}

// ---------- paths / time ----------

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| die("HOME not set")))
}

fn log_path() -> PathBuf {
    home().join(".local/share/ccclean/closed.jsonl")
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

/// Timestamp of the last real message (`user` or `assistant` entry) in a transcript.
/// File mtime is useless here: idle sessions keep appending untimestamped
/// bookkeeping lines (cost-state, last-prompt, file-history-snapshot).
fn last_message_at(p: &Path) -> Option<u64> {
    use std::io::{Read, Seek, SeekFrom};
    const TAIL: u64 = 1 << 20;
    let mut f = fs::File::open(p).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.lines();
    if start > 0 {
        lines.next(); // partial line
    }
    lines.rev().find_map(|l| {
        let v: Value = serde_json::from_str(l).ok()?;
        matches!(v["type"].as_str(), Some("user" | "assistant")).then_some(())?;
        parse_iso(v["timestamp"].as_str()?)
    })
}

/// "2026-09-22T03:59:34.959Z" -> epoch seconds (UTC only, which is what Claude writes).
fn parse_iso(s: &str) -> Option<u64> {
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (hh, mm, ss) = (n(11..13)?, n(14..16)?, n(17..19)?);
    // days from civil (Howard Hinnant)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    u64::try_from(days * 86400 + hh * 3600 + mm * 60 + ss).ok()
}

fn ago(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86400 => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
        s => format!("{}d{}h", s / 86400, s % 86400 / 3600),
    }
}

fn local_time(epoch: u64) -> String {
    unsafe {
        let t = epoch as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min
        )
    }
}

fn tilde(p: &str) -> String {
    let h = home();
    let h = h.to_string_lossy();
    p.strip_prefix(h.as_ref()).map(|r| format!("~{r}")).unwrap_or_else(|| p.to_string())
}

// ---------- discovery ----------

fn cmux_bin() -> String {
    if let Ok(p) = std::env::var("CMUX_BIN") {
        return p;
    }
    if Path::new(CMUX_FALLBACK).exists() { CMUX_FALLBACK.into() } else { "cmux".into() }
}

fn cmux(args: &[&str]) -> Result<String, String> {
    let out = Command::new(cmux_bin()).args(args).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// tty name (e.g. "ttys004") -> surface
fn surfaces_by_tty() -> Result<HashMap<String, Surface>, String> {
    let raw = cmux(&["--json", "--id-format", "both", "tree", "--all"]).map_err(|e| format!("cmux tree failed: {e}"))?;
    let v: Value = serde_json::from_str(&raw).map_err(|e| format!("bad cmux json: {e}"))?;
    let mut map = HashMap::new();
    let arr = |v: &Value, k: &str| v[k].as_array().cloned().unwrap_or_default();
    for win in arr(&v, "windows") {
        for ws in arr(&win, "workspaces") {
            let surface_count: usize = arr(&ws, "panes").iter().map(|p| arr(p, "surfaces").len()).sum();
            for pane in arr(&ws, "panes") {
                for s in arr(&pane, "surfaces") {
                    let Some(tty) = s["tty"].as_str() else { continue };
                    map.insert(
                        tty.to_string(),
                        Surface {
                            surface_id: s["id"].as_str().unwrap_or_default().into(),
                            workspace_id: ws["id"].as_str().unwrap_or_default().into(),
                            surface_ref: s["ref"].as_str().unwrap_or_default().into(),
                            workspace_title: ws["title"].as_str().unwrap_or_default().into(),
                            title: s["title"].as_str().unwrap_or_default().into(),
                            active: s["active"].as_bool().unwrap_or(false)
                                && ws["active"].as_bool().unwrap_or(false),
                            only_in_workspace: surface_count == 1,
                        },
                    );
                }
            }
        }
    }
    Ok(map)
}

/// Child processes that mean a session is still doing work even if its status
/// says idle: shells running (background) commands, and `caffeinate`, which
/// Claude Code holds while working. MCP servers (node, uv, python…) don't count.
const WORK_CHILDREN: &[&str] = &["zsh", "bash", "sh", "fish", "dash", "caffeinate"];

struct Procs {
    tty: HashMap<i32, String>,       // pid -> tty, for processes attached to one
    working: std::collections::HashSet<i32>, // pids with a WORK_CHILDREN child
}

fn procs() -> Procs {
    let out = Command::new("ps").args(["-axo", "pid=,ppid=,tty=,comm="]).output().unwrap_or_else(|e| die(&e.to_string()));
    let mut p = Procs { tty: HashMap::new(), working: Default::default() };
    for l in String::from_utf8_lossy(&out.stdout).lines() {
        let mut it = l.split_whitespace();
        let (Some(pid), Some(ppid), Some(tty)) = (it.next(), it.next(), it.next()) else { continue };
        let (Ok(pid), Ok(ppid)) = (pid.parse::<i32>(), ppid.parse::<i32>()) else { continue };
        let comm = it.collect::<Vec<_>>().join(" ");
        let base = comm.rsplit('/').next().unwrap_or(&comm).trim_start_matches('-');
        if WORK_CHILDREN.contains(&base) {
            p.working.insert(ppid);
        }
        if tty != "??" {
            p.tty.insert(pid, tty.to_string());
        }
    }
    p
}

fn transcript(cwd: &str, session_id: &str) -> PathBuf {
    let enc: String = cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    home().join(".claude/projects").join(enc).join(format!("{session_id}.jsonl"))
}

fn sessions() -> Result<Vec<Session>, String> {
    let procs = procs();
    let ttys = &procs.tty;
    let surfaces = surfaces_by_tty()?;
    let dir = home().join(".claude/sessions");
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(v) = fs::read_to_string(&path).map_err(drop).and_then(|s| serde_json::from_str::<Value>(&s).map_err(drop))
        else {
            continue;
        };
        let pid = v["pid"].as_i64().unwrap_or(0) as i32;
        let Some(tty) = ttys.get(&pid) else { continue }; // dead or not in a terminal
        let session_id = v["sessionId"].as_str().unwrap_or_default().to_string();
        let cwd = v["cwd"].as_str().unwrap_or_default().to_string();
        let status_at = v["statusUpdatedAt"].as_u64().or(v["updatedAt"].as_u64()).unwrap_or(0) / 1000;
        let last_active = status_at.max(last_message_at(&transcript(&cwd, &session_id)).unwrap_or(0));
        out.push(Session {
            pid,
            name: v["name"].as_str().unwrap_or_default().into(),
            status: v["status"].as_str().unwrap_or("?").into(),
            surface: surfaces.get(tty).cloned(),
            working: procs.working.contains(&pid),
            tty: tty.clone(),
            session_id,
            cwd,
            last_active,
        });
    }
    out.sort_by_key(|s| s.last_active);
    Ok(out)
}

fn own_tty() -> Option<String> {
    unsafe {
        let p = libc::ttyname(0);
        if p.is_null() {
            return None;
        }
        let s = std::ffi::CStr::from_ptr(p).to_string_lossy();
        s.strip_prefix("/dev/").map(String::from)
    }
}

// ---------- commands ----------

fn status() {
    let t = now();
    let list = sessions().unwrap_or_else(|e| die(&e));
    if list.is_empty() {
        println!("no running Claude Code sessions found");
        return;
    }
    println!("{:<8} {:<6} {:<14} {:<34} {}", "IDLE", "STATUS", "WHERE", "TITLE", "DIR");
    for s in &list {
        let (loc, title) = match &s.surface {
            Some(sf) => (sf.surface_ref.clone(), clip(&sf.title, 34)),
            None => (s.tty.clone(), "(not in cmux)".into()),
        };
        println!("{:<8} {:<6} {:<14} {:<34} {}", ago(t.saturating_sub(s.last_active)), if s.working && s.status == "idle" { "bg" } else { &s.status }, loc, title, tilde(&s.cwd));
    }
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.into() } else { s.chars().take(n - 1).collect::<String>() + "…" }
}

fn sweep(args: &[String]) {
    let idle = parse_dur(&flag(args, "--idle").unwrap_or_else(|| "2h".into()));
    let dry = has(args, "--dry-run");
    match sweep_once(idle, dry) {
        Err(e) => die(&e),
        Ok(0) if !dry => println!("nothing idle for longer than {}", ago(idle)),
        Ok(_) => {}
    }
}

/// Runs forever inside a cmux tab (cmux only accepts socket clients started from cmux).
fn watch(args: &[String]) {
    let every = parse_dur(&flag(args, "--every").unwrap_or_else(|| "15m".into())).max(10);
    let idle = parse_dur(&flag(args, "--idle").unwrap_or_else(|| "2h".into()));
    println!("ccclean watching: every {}, closing sessions idle > {}\nclosed sessions: ccclean list\n", ago(every), ago(idle));
    loop {
        let stamp = local_time(now());
        match sweep_once(idle, false) {
            Ok(n) => println!("{stamp}  swept, closed {n}"),
            Err(e) => eprintln!("{stamp}  sweep failed: {e}"),
        }
        std::thread::sleep(std::time::Duration::from_secs(every));
    }
}

fn sweep_once(idle: u64, dry: bool) -> Result<usize, String> {
    let t = now();
    let me = own_tty();
    let mut closed = 0;
    for s in sessions()? {
        let Some(sf) = &s.surface else { continue };
        if sf.surface_id.is_empty() || sf.workspace_id.is_empty() {
            continue; // never fall back to cmux's "current" surface
        }
        let idle_for = t.saturating_sub(s.last_active);
        if s.status != "idle" || s.working || idle_for < idle || sf.active || me.as_deref() == Some(s.tty.as_str()) {
            continue;
        }
        let resume = format!("cd {} && claude --resume {}", shell_quote(&s.cwd), s.session_id);
        if dry {
            println!("would close {} [{}] idle {} — {} — {}", sf.surface_ref, sf.workspace_title, ago(idle_for), sf.title, tilde(&s.cwd));
            continue;
        }
        let res = if sf.only_in_workspace {
            cmux(&["close-workspace", "--workspace", &sf.workspace_id])
        } else {
            cmux(&["close-surface", "--surface", &sf.surface_id, "--workspace", &sf.workspace_id])
        };
        if let Err(e) = res {
            eprintln!("ccclean: failed to close {}: {e}", sf.surface_ref);
            continue;
        }
        // make sure claude itself is gone even if the surface close didn't HUP it
        unsafe { libc::kill(s.pid, libc::SIGTERM) };
        let rec = json!({
            "closed_at": t,
            "title": sf.title,
            "workspace": sf.workspace_title,
            "name": s.name,
            "cwd": s.cwd,
            "session_id": s.session_id,
            "idle_secs": idle_for,
            "resume": resume,
        });
        append_log(&rec);
        println!("closed {} idle {} — {} — {}", sf.surface_ref, ago(idle_for), sf.title, tilde(&s.cwd));
        closed += 1;
    }
    Ok(closed)
}

fn shell_quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-~".contains(c)) {
        s.into()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

fn append_log(rec: &Value) {
    let p = log_path();
    fs::create_dir_all(p.parent().unwrap()).ok();
    let mut f = OpenOptions::new().create(true).append(true).open(&p).unwrap_or_else(|e| die(&e.to_string()));
    writeln!(f, "{rec}").ok();
}

fn list(args: &[String]) {
    let p = log_path();
    if has(args, "--clear") {
        fs::remove_file(&p).ok();
        println!("cleared {}", p.display());
        return;
    }
    let n: usize = flag(args, "-n").and_then(|v| v.parse().ok()).unwrap_or(20);
    let raw = fs::read_to_string(&p).unwrap_or_default();
    let recs: Vec<Value> = raw.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    if recs.is_empty() {
        println!("no closed sessions logged yet");
        return;
    }
    for r in recs.iter().rev().take(n) {
        let s = |k: &str| r[k].as_str().unwrap_or_default().to_string();
        println!("{}  {}  [{}]", local_time(r["closed_at"].as_u64().unwrap_or(0)), s("title"), s("workspace"));
        println!("    dir:    {}", tilde(&s("cwd")));
        println!("    resume: {}\n", s("resume"));
    }
    if recs.len() > n {
        println!("({} older — use -n {})", recs.len() - n, recs.len());
    }
}

const WATCH_WS: &str = "ccclean";
/// Workspace description used to recognise our own watcher (titles aren't unique).
const WATCH_MARKER: &str = "ccclean watcher — closes idle Claude Code sessions";

fn watcher_workspace() -> Option<String> {
    let raw = cmux(&["--json", "--id-format", "both", "tree", "--all"]).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    v["windows"].as_array()?.iter().flat_map(|w| w["workspaces"].as_array().cloned().unwrap_or_default())
        .find(|ws| ws["description"].as_str() == Some(WATCH_MARKER))
        .and_then(|ws| ws["id"].as_str().map(String::from))
}

/// Path to launch the watcher with. For Homebrew installs, prefer the
/// `<prefix>/bin` link over the versioned Cellar path, which `brew upgrade` deletes.
fn stable_exe() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|e| die(&e.to_string()));
    let exe = fs::canonicalize(&exe).unwrap_or(exe);
    let s = exe.to_string_lossy();
    if let Some(i) = s.find("/Cellar/") {
        let link = PathBuf::from(format!("{}/bin/ccclean", &s[..i]));
        if fs::canonicalize(&link).map(|t| t == exe).unwrap_or(false) {
            return link;
        }
    }
    exe
}

fn install(args: &[String]) {
    let every = flag(args, "--every").unwrap_or_else(|| "15m".into());
    let idle = flag(args, "--idle").unwrap_or_else(|| "2h".into());
    parse_dur(&every);
    parse_dur(&idle);
    if let Some(id) = watcher_workspace() {
        cmux(&["close-workspace", "--workspace", &id]).unwrap_or_else(|e| die(&e));
    }
    let exe = stable_exe();
    let command = format!("{} watch --every {every} --idle {idle}", shell_quote(&exe.to_string_lossy()));
    cmux(&["new-workspace", "--name", WATCH_WS, "--description", WATCH_MARKER, "--command", &command, "--focus", "false"]).unwrap_or_else(|e| die(&e));
    println!("started watcher in cmux workspace \"{WATCH_WS}\": sweep every {every}, close sessions idle > {idle}");
    println!("keep that workspace open; rerun `ccclean install` after restarting cmux");
}

fn uninstall() {
    match watcher_workspace() {
        Some(id) => {
            cmux(&["close-workspace", "--workspace", &id]).unwrap_or_else(|e| die(&e));
            println!("closed watcher workspace \"{WATCH_WS}\"");
        }
        None => println!("no watcher workspace running"),
    }
}
