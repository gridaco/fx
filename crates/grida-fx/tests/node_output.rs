//! A TypeScript body's output, run by the real TypeScript node host through the `grida-fx`
//! command: whatever the body and the processes it starts print reaches the engine's stderr whole,
//! the protocol on the host's stdout stays intact, and the engine's own output is never lost or
//! overwritten, whatever the engine's stderr is (spec/protocol.md §1 "Output streams";
//! gridaco/fx#68). The engine gives a TypeScript host a stderr pipe of its own and copies it
//! (`grida_fx_runtime::host::process`), which these runs hold to the cases that need it: a socket
//! (Node starts its children with sockets, and Linux cannot open one anew) and a pipe (`2>&1 |
//! less`), each left blocking while the host runs (the flags are read with `python3`, when it is
//! on `PATH`), and a regular file opened without `O_APPEND` (a shell's `2> log` or `> log 2>&1`,
//! which Linux would open anew with an offset of its own). A reader that cannot keep up, or that
//! starts reading long after the host has ended (`2>&1 | less`), still gets every line, whole,
//! before the engine's last: the engine waits for the copy to write what the host left, however
//! slowly that is read, and writes whole lines, so hosts that print at once do not splice them; so
//! does one whose stderr a parent made non-blocking, while a body reports progress too (the
//! engine's own lines wait for such a stream as the copy does). A command interrupted while a copy
//! is blocked on a stderr nobody reads still ends within seconds, and a forced exit says so when
//! its stderr is read, however slowly.
//!
//! Each test skips unless Node 24 or later is on `PATH` and the repository's
//! `js/fx/dist/host-entry.js` has been built; the project's `@grida/fx` is a copy of `js/fx`'s
//! `package.json` and `dist`.
#![cfg(unix)]

use std::io::Read;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The repository's `js/fx`.
fn js_fx() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../js/fx")
}

/// Whether Node 24 or later is on `PATH` and `js/fx/dist/host-entry.js` is built; prints why not.
fn real_node() -> bool {
    if !js_fx().join("dist/host-entry.js").is_file() {
        eprintln!("skipped: js/fx/dist/host-entry.js is not built");
        return false;
    }
    let version = Command::new("node")
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string());
    let major = version
        .as_deref()
        .and_then(|v| v.strip_prefix('v'))
        .and_then(|v| v.split('.').next())
        .and_then(|major| major.parse::<u32>().ok());
    if major.is_some_and(|major| major >= 24) {
        true
    } else {
        eprintln!("skipped: no Node 24 or later on PATH ({version:?})");
        false
    }
}

/// The lines the body and the processes it starts print, each once.
const MARKERS: [&str; 6] = [
    "marker console.log",
    "marker process.stdout.write",
    "marker console.error",
    "marker spawnSync inherit",
    "marker async spawn inherit",
    "marker grandchild",
];

/// The body: the markers Node itself writes, then, when the project holds `hold`, `printed`
/// written and a wait for `checked` ([`Held`]), then the markers of the programs it starts. The
/// wait comes first: a program started with inherited streams has them made blocking again.
const NOISY: &str = r#"import { spawn, spawnSync } from "node:child_process";
import { existsSync, writeFileSync } from "node:fs";
import { node } from "@grida/fx";

export const noisy = node({ name: "noisy", outputs: { text: "text" } }, async (ctx) => {
  console.log("marker console.log");
  process.stdout.write("marker process.stdout.write\n");
  console.error("marker console.error");
  if (existsSync("hold")) {
    writeFileSync("printed", "");
    for (let waited = 0; !existsSync("checked") && waited < 60000; waited += 20) {
      await new Promise((done) => setTimeout(done, 20));
    }
  }
  spawnSync("sh", ["-c", "echo marker spawnSync inherit"], { stdio: "inherit" });
  await new Promise((done, fail) => {
    const child = spawn("sh", ["-c", "echo marker async spawn inherit"], { stdio: "inherit" });
    child.on("error", fail);
    child.on("exit", done);
  });
  spawnSync("sh", ["-c", "sh -c \"echo marker grandchild\""], { stdio: "inherit" });
  return { text: ctx.out.text("ok") };
});
"#;

/// Copies the folder `from` to `to`, recursively.
fn copy_folder(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_folder(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// A project `acme` in `dir` whose workflow `w` runs the noisy TypeScript body, with
/// `node_modules/@grida/fx` a copy of `js/fx`'s `package.json` and `dist`.
fn noisy_project(dir: &Path) -> PathBuf {
    let root = dir.join("acme");
    std::fs::create_dir_all(root.join("nodes")).unwrap();
    std::fs::create_dir_all(root.join("workflows")).unwrap();
    std::fs::write(root.join("fx.yaml"), "fx: project/v1\n").unwrap();
    std::fs::write(root.join("package.json"), "{\"type\": \"module\"}\n").unwrap();
    std::fs::write(root.join("nodes/noisy.ts"), NOISY).unwrap();
    std::fs::write(
        root.join("workflows/w.yaml"),
        "fx: workflow/v1\nid: w\ntitle: W\nsteps:\n  s:\n    uses: ./nodes/noisy.ts#noisy\n\
         outputs:\n  text: ${{ steps.s.outputs.text }}\n",
    )
    .unwrap();
    let package = root.join("node_modules/@grida/fx");
    copy_folder(&js_fx().join("dist"), &package.join("dist"));
    std::fs::copy(js_fx().join("package.json"), package.join("package.json")).unwrap();
    root
}

/// `grida-fx run <workflow> --no-view` in `root` with a minimal environment: Node from `PATH`,
/// no provider key, the network and the `.env` file off.
fn run_command(root: &Path, workflow: &str) -> Command {
    fx_command(root, &["run", workflow, "--no-view"])
}

/// `grida-fx <args>` in `root` with the environment of [`run_command`].
fn fx_command(root: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_grida-fx"));
    command
        .args(args)
        .current_dir(root)
        .env_clear()
        .stdin(Stdio::null());
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command
        .env("HOME", root)
        .env("NO_COLOR", "1")
        .env("GRIDA_FX_NETWORK", "off")
        .env("GRIDA_FX_DISABLE_DOTENV", "1")
        .env("GRIDA_FX_TOOL_CODEX", root.join("no-codex"));
    command
}

/// How many lines of `text` are exactly `line`.
fn count(text: &str, line: &str) -> usize {
    text.lines().filter(|l| *l == line).count()
}

/// The run succeeded, its stdout is the engine's summary alone, and its stderr (`captured`) holds
/// every marker whole, once, beside the engine's own lines.
fn assert_kept_apart(output: &Output, captured: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{:?}\n{stdout}\n{captured}",
        output.status
    );
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.first(), Some(&"w  \u{b7}  1 phase"), "{stdout}");
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("run       runs/w/")),
        "{stdout}"
    );
    assert_eq!(
        lines.last(),
        Some(&"result    ok   spent $0.00"),
        "{stdout}"
    );
    assert!(!stdout.contains("marker"), "{stdout}");
    for marker in MARKERS {
        assert_eq!(count(captured, marker), 1, "{marker}:\n{captured}");
    }
    assert!(
        captured.lines().any(|line| line.starts_with("invocation ")),
        "{captured}"
    );
}

/// Whether the open file description of `stream` is non-blocking, as `python3` reads it from its
/// standard input; `None` without `python3`.
fn non_blocking(stream: impl Into<Stdio>) -> Option<bool> {
    let output = Command::new("python3")
        .args([
            "-c",
            "import fcntl, os; print(1 if fcntl.fcntl(0, fcntl.F_GETFL) & os.O_NONBLOCK else 0)",
        ])
        .stdin(stream)
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    match String::from_utf8_lossy(&output.stdout).trim() {
        "1" => Some(true),
        "0" => Some(false),
        other => panic!("python3 answered {other:?}"),
    }
}

/// Reads whether the engine's stderr is non-blocking while the body is held after the output Node
/// writes itself ([`NOISY`]): Node puts back the flags it changed when it exits, so they are read
/// while the host runs.
struct Held {
    ended: Arc<AtomicBool>,
    checked: std::thread::JoinHandle<Option<bool>>,
}

impl Held {
    /// Holds the body of the project at `root`; `kept` shares the open file description of the
    /// engine's stderr.
    fn start(root: &Path, kept: OwnedFd) -> Held {
        std::fs::write(root.join("hold"), "").unwrap();
        let root = root.to_path_buf();
        let ended = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&ended);
        let checked = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(120);
            while !root.join("printed").exists() {
                if seen.load(Ordering::SeqCst) || Instant::now() > deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let flag = non_blocking(kept);
            std::fs::write(root.join("checked"), "").unwrap();
            flag
        });
        Held { ended, checked }
    }

    /// Once the command has exited: the stream was left blocking. Without `python3`, or when the
    /// body never got there (the run then fails), nothing was read.
    fn assert_left_blocking(self) {
        self.ended.store(true, Ordering::SeqCst);
        match self.checked.join().unwrap() {
            Some(flag) => assert!(!flag, "the engine's stderr was made non-blocking"),
            None => eprintln!("skipped: the stream's flags were not read (python3 is needed)"),
        }
    }
}

#[test]
fn a_typescript_bodys_output_reaches_a_socket_stderr() {
    if !real_node() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = noisy_project(&std::fs::canonicalize(dir.path()).unwrap());
    let (mut ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
    ours.set_read_timeout(Some(Duration::from_secs(120)))
        .unwrap();
    let held = Held::start(&root, OwnedFd::from(theirs.try_clone().unwrap()));
    // The command (and with it this process's copy of the socket) is dropped once spawned, so
    // the socket ends when the engine and what it started have closed it.
    let child = run_command(&root, "w")
        .stdout(Stdio::piped())
        .stderr(Stdio::from(OwnedFd::from(theirs)))
        .spawn()
        .unwrap();
    let reader = std::thread::spawn(move || {
        let mut captured = Vec::new();
        ours.read_to_end(&mut captured).unwrap();
        captured
    });
    let output = child.wait_with_output().unwrap();
    held.assert_left_blocking();
    let captured = String::from_utf8(reader.join().unwrap()).unwrap();
    assert_kept_apart(&output, &captured);
}

#[test]
fn a_typescript_bodys_output_reaches_a_pipe_stderr_left_blocking() {
    if !real_node() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = noisy_project(&std::fs::canonicalize(dir.path()).unwrap());
    // `2>&1 | less`: Node makes a pipe it writes to non-blocking, and a non-blocking stderr
    // fails the engine's own writes while its reader is slow.
    let (mut reader, writer) = std::io::pipe().unwrap();
    let held = Held::start(&root, OwnedFd::from(writer.try_clone().unwrap()));
    let child = run_command(&root, "w")
        .stdout(Stdio::piped())
        .stderr(writer)
        .spawn()
        .unwrap();
    let read = std::thread::spawn(move || {
        let mut captured = Vec::new();
        reader.read_to_end(&mut captured).unwrap();
        captured
    });
    let output = child.wait_with_output().unwrap();
    held.assert_left_blocking();
    let captured = String::from_utf8(read.join().unwrap()).unwrap();
    assert_kept_apart(&output, &captured);
}

#[test]
fn a_typescript_bodys_output_reaches_a_file_stderr_whole() {
    if !real_node() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let root = noisy_project(&base);
    // `2> run.log`: written from the start, not appended.
    let log = base.join("run.log");
    let file = std::fs::File::create(&log).unwrap();
    let output = run_command(&root, "w")
        .stdout(Stdio::piped())
        .stderr(Stdio::from(file))
        .output()
        .unwrap();
    let captured = std::fs::read_to_string(&log).unwrap();
    assert_kept_apart(&output, &captured);
}

#[test]
fn a_typescript_bodys_output_and_the_engines_share_a_file_whole() {
    if !real_node() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let root = noisy_project(&base);
    // `> run.log 2>&1`: one description for both streams, written from the start.
    let log = base.join("run.log");
    let file = std::fs::File::create(&log).unwrap();
    let both = file.try_clone().unwrap();
    let status = run_command(&root, "w")
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(both))
        .status()
        .unwrap();
    let captured = std::fs::read_to_string(&log).unwrap();
    assert!(status.success(), "{status:?}\n{captured}");
    for line in MARKERS
        .iter()
        .copied()
        .chain(["w  \u{b7}  1 phase", "result    ok   spent $0.00"])
    {
        assert_eq!(count(&captured, line), 1, "{line}:\n{captured}");
    }
    // The plan was written before the body ran, and the body's output did not overwrite it.
    let at = |line: &str| captured.find(line).unwrap();
    assert!(
        at("w  \u{b7}  1 phase") < at("marker console.log"),
        "{captured}"
    );
}

/// A body that prints `count` lines `<tag><number><99 × tag>` through `awk`, which it starts with
/// its streams inherited (so they are written blocking, in blocks that end anywhere in a line),
/// then `marker <tag> end`.
const LINES: &str = r#"import { spawnSync } from "node:child_process";
import { node } from "@grida/fx";

const PRINT =
  'BEGIN { pad = ""; for (i = 0; i < 99; i++) pad = pad tag; ' +
  'for (i = 0; i < count; i++) printf "%s%06d%s\\n", tag, i, pad }';

export const lines = node(
  { name: "lines", params: { tag: "string", count: "integer" }, outputs: { text: "text" } },
  async (ctx) => {
    const { tag, count } = ctx.params;
    spawnSync("awk", ["-v", `tag=${tag}`, "-v", `count=${count}`, PRINT], { stdio: "inherit" });
    spawnSync("sh", ["-c", `echo marker ${tag} end`], { stdio: "inherit" });
    return { text: ctx.out.text(tag) };
  },
);
"#;

/// The project of [`noisy_project`], with workflows `one` (a step printing [`LINES`] tagged `A`)
/// and `two` (steps tagged `A` and `B`, which run at once), each step `count` lines.
fn lines_project(dir: &Path, count: usize) -> PathBuf {
    let root = noisy_project(dir);
    std::fs::write(root.join("nodes/lines.ts"), LINES).unwrap();
    let step = |tag: char| {
        let id = tag.to_ascii_lowercase();
        format!(
            "  {id}:\n    uses: ./nodes/lines.ts#lines\n    with: {{ tag: {tag}, count: {count} }}\n"
        )
    };
    for (id, tags) in [("one", &['A'][..]), ("two", &['A', 'B'][..])] {
        let steps: String = tags.iter().copied().map(step).collect();
        let outputs: String = tags
            .iter()
            .map(|tag| tag.to_ascii_lowercase())
            .map(|id| format!("  {id}: ${{{{ steps.{id}.outputs.text }}}}\n"))
            .collect();
        std::fs::write(
            root.join(format!("workflows/{id}.yaml")),
            format!("fx: workflow/v1\nid: {id}\ntitle: {id}\nsteps:\n{steps}outputs:\n{outputs}"),
        )
        .unwrap();
    }
    root
}

/// Runs `workflow` with the command's stdout and stderr on one pipe (`2>&1 | reader`), read
/// after `first`, `chunk` bytes at a time with `pause` after each read; the command's status and
/// everything read.
fn run_read_slowly(
    root: &Path,
    workflow: &str,
    first: Duration,
    chunk: usize,
    pause: Duration,
) -> (std::process::ExitStatus, String) {
    let (mut reader, writer) = std::io::pipe().unwrap();
    let both = writer.try_clone().unwrap();
    let mut child = run_command(root, workflow)
        .stdout(writer)
        .stderr(both)
        .spawn()
        .unwrap();
    std::thread::sleep(first);
    let mut captured = Vec::new();
    let mut buffer = vec![0u8; chunk];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => captured.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => panic!("{error}"),
        }
        std::thread::sleep(pause);
    }
    let status = child.wait().unwrap();
    (status, String::from_utf8(captured).unwrap())
}

/// The run succeeded; every line of each tag's step arrived whole and once, with its end marker;
/// no other line holds a step's text; and the engine's last line, the result, came last.
fn assert_every_line_whole(
    status: std::process::ExitStatus,
    captured: &str,
    tags: &[char],
    count: usize,
) {
    assert!(status.success(), "{status:?}\n{captured}");
    let mut seen = std::collections::HashMap::<&str, usize>::new();
    for line in captured.lines() {
        *seen.entry(line).or_default() += 1;
    }
    let mut wrong = Vec::new();
    for &tag in tags {
        let pad = tag.to_string().repeat(99);
        let expected = (0..count)
            .map(|number| format!("{tag}{number:06}{pad}"))
            .chain([format!("marker {tag} end")]);
        for line in expected {
            match seen.remove(line.as_str()) {
                Some(1) => {}
                times => wrong.push(format!("{line} arrived {times:?} times")),
            }
        }
    }
    let mangled: Vec<&&str> = seen
        .keys()
        .filter(|line| {
            tags.iter()
                .any(|&tag| line.contains(&tag.to_string().repeat(20)))
        })
        .collect();
    assert!(
        wrong.is_empty() && mangled.is_empty(),
        "{} lines not whole once: {:?}\nlines spliced or cut: {mangled:?}",
        wrong.len(),
        &wrong[..wrong.len().min(5)]
    );
    assert_eq!(
        captured.lines().last(),
        Some("result    ok   spent $0.00"),
        "{}",
        &captured[captured.len().saturating_sub(2000)..]
    );
}

/// A reader that takes 4 KiB every 100 ms is still reading the body's lines well after the host
/// has ended: the engine waits for the copy while it moves, rather than for a second, so nothing
/// is lost when the command exits right after (`grida_fx_runtime::host::process`).
#[test]
fn a_slow_reader_gets_everything_a_typescript_body_printed() {
    if !real_node() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = lines_project(&std::fs::canonicalize(dir.path()).unwrap(), 1200);
    let (status, captured) = run_read_slowly(
        &root,
        "one",
        Duration::ZERO,
        4096,
        Duration::from_millis(100),
    );
    assert_every_line_whole(status, &captured, &['A'], 1200);
}

/// A reader that starts reading 15 seconds after the run started (a pager whose first screen is
/// read at leisure), well after the host has ended, then reads flat out: it gets every line. What
/// the engine waits for after a host ends is what the host left, not a time.
#[test]
fn a_reader_that_starts_late_gets_everything_a_typescript_body_printed() {
    if !real_node() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = lines_project(&std::fs::canonicalize(dir.path()).unwrap(), 1000);
    let (status, captured) =
        run_read_slowly(&root, "one", Duration::from_secs(15), 65536, Duration::ZERO);
    assert_every_line_whole(status, &captured, &['A'], 1000);
}

/// Two hosts print at once to a reader that cannot keep up: their copies, and the engine's own
/// lines, meet only between whole lines.
#[test]
fn concurrent_typescript_hosts_print_whole_lines() {
    if !real_node() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = lines_project(&std::fs::canonicalize(dir.path()).unwrap(), 2000);
    let (status, captured) = run_read_slowly(
        &root,
        "two",
        Duration::ZERO,
        16384,
        Duration::from_millis(50),
    );
    assert_every_line_whole(status, &captured, &['A', 'B'], 2000);
}

/// The run of [`lines_project`]'s `one`, `count` lines, with the command's stderr a pipe that a
/// parent made non-blocking (as Node leaves a pipe it wrote to) and whose reader falls behind by
/// two seconds: every line still arrives whole, and once.
#[test]
fn a_non_blocking_stderr_whose_reader_falls_behind_gets_everything() {
    if !real_node() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let lines = 3000;
    let root = lines_project(&std::fs::canonicalize(dir.path()).unwrap(), lines);
    let (mut reader, writer) = std::io::pipe().unwrap();
    rustix::io::ioctl_fionbio(&writer, true).unwrap();
    let child = run_command(&root, "one")
        .stdout(Stdio::piped())
        .stderr(writer)
        .spawn()
        .unwrap();
    let read = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        let mut captured = Vec::new();
        reader.read_to_end(&mut captured).unwrap();
        captured
    });
    let output = child.wait_with_output().unwrap();
    let captured = String::from_utf8(read.join().unwrap()).unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{:?}\n{stdout}", output.status);
    let pad = "A".repeat(99);
    let missing: Vec<String> = (0..lines)
        .map(|number| format!("A{number:06}{pad}"))
        .chain(["marker A end".to_string()])
        .filter(|line| count(&captured, line) != 1)
        .collect();
    assert!(
        missing.is_empty(),
        "{} lines not there whole once, the first {:?}",
        missing.len(),
        missing.first()
    );
}

/// A body that prints `count` lines `P<number><99 × P>` with `console.error`, and reports progress
/// (`round <number>`) every 20 lines.
const PROGRESS: &str = r#"import { node } from "@grida/fx";

export const prog = node(
  { name: "prog", params: { count: "integer" }, outputs: { text: "text" } },
  async (ctx) => {
    for (let i = 0; i < ctx.params.count; i++) {
      console.error(`P${String(i).padStart(6, "0")}` + "P".repeat(99));
      if (i % 20 === 0) {
        ctx.progress(`round ${i}`);
        await new Promise((done) => setImmediate(done));
      }
    }
    return { text: ctx.out.text("ok") };
  },
);
"#;

/// Waits up to `limit` for `child`, whose stdout is piped; its status and stdout. A command still
/// running then is killed with the host of the project at `root`, and the test fails.
fn finishes_within(
    mut child: std::process::Child,
    limit: Duration,
    root: &Path,
) -> (std::process::ExitStatus, String) {
    let mut stdout = child.stdout.take().unwrap();
    let printed = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let start = Instant::now();
    while start.elapsed() < limit {
        if let Some(status) = child.try_wait().unwrap() {
            return (status, printed.join().unwrap());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    // The host leads a process group of its own, which the engine's death does not end.
    let entry = root.join("node_modules/@grida/fx/dist/host-entry.js");
    let _ = Command::new("pkill")
        .args(["-KILL", "-f", &entry.to_string_lossy()])
        .status();
    panic!("the run was still going {limit:?} after it started");
}

/// A body that prints and reports progress while the command's stderr is a pipe that a parent
/// made non-blocking and whose reader falls behind: the engine's progress lines wait for the
/// stream as the copy does, so neither the run nor the connection's reader stops, and every
/// progress line arrives once.
#[test]
fn a_non_blocking_stderr_that_falls_behind_while_a_body_reports_progress() {
    if !real_node() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = noisy_project(&std::fs::canonicalize(dir.path()).unwrap());
    let count = 20_000;
    std::fs::write(root.join("nodes/prog.ts"), PROGRESS).unwrap();
    std::fs::write(
        root.join("workflows/prog.yaml"),
        format!(
            "fx: workflow/v1\nid: prog\ntitle: prog\nsteps:\n  s:\n    uses: ./nodes/prog.ts#prog\n    \
             with: {{ count: {count} }}\noutputs:\n  text: ${{{{ steps.s.outputs.text }}}}\n"
        ),
    )
    .unwrap();
    let (mut reader, writer) = std::io::pipe().unwrap();
    rustix::io::ioctl_fionbio(&writer, true).unwrap();
    let child = run_command(&root, "prog")
        .stdout(Stdio::piped())
        .stderr(writer)
        .spawn()
        .unwrap();
    // A second late, then 4 KiB every millisecond.
    let read = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(1));
        let mut captured = Vec::new();
        let mut buffer = vec![0u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => captured.extend_from_slice(&buffer[..read]),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => panic!("{error}"),
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        captured
    });
    let (status, stdout) = finishes_within(child, Duration::from_secs(90), &root);
    let captured = String::from_utf8(read.join().unwrap()).unwrap();
    assert!(
        status.success(),
        "{status:?}\n{stdout}\n{}",
        &captured[captured.len().saturating_sub(2000)..]
    );
    assert_eq!(
        stdout.lines().last(),
        Some("result    ok   spent $0.00"),
        "{stdout}"
    );
    let mut rounds = std::collections::HashMap::<&str, usize>::new();
    for line in captured.lines() {
        if let Some((_, round)) = line.split_once(": round ") {
            *rounds.entry(round).or_default() += 1;
        }
    }
    let wrong: Vec<usize> = (0..count)
        .step_by(20)
        .filter(|round| rounds.get(round.to_string().as_str()) != Some(&1))
        .collect();
    let printed = captured
        .lines()
        .filter(|line| line.starts_with("P0"))
        .count();
    assert!(
        wrong.is_empty(),
        "{} progress lines not there once, the first {:?} ({printed} lines of the body's)",
        wrong.len(),
        wrong.first()
    );
}

/// A module that prints 3000 lines when it is imported, more than two pipes hold.
const LOUD: &str = r#"import { node } from "@grida/fx";

for (let i = 0; i < 3000; i++) console.error(`loud line ${i} ` + "y".repeat(80));

export const loud = node({ name: "loud", outputs: { text: "text" } }, async (ctx) => ({
  text: ctx.out.text("ok"),
}));
"#;

/// A body that prints 3000 lines, then waits two minutes.
const STUCK: &str = r#"import { node } from "@grida/fx";

export const stuck = node({ name: "stuck", outputs: { text: "text" } }, async (ctx) => {
  for (let i = 0; i < 3000; i++) console.error(`stuck line ${i} ` + "y".repeat(80));
  await new Promise((done) => setTimeout(done, 120000));
  return { text: ctx.out.text("ok") };
});
"#;

/// A body that prints for good.
const FLOOD: &str = r#"import { node } from "@grida/fx";

export const flood = node({ name: "flood", outputs: { text: "text" } }, async (ctx) => {
  for (let i = 0; ; i++) {
    console.error(`flood line ${i} ` + "f".repeat(90));
    if (i % 1000 === 0) await new Promise((done) => setImmediate(done));
  }
  return { text: ctx.out.text("never") };
});
"#;

/// The project of [`noisy_project`], with workflows `loud`, `stuck` and `flood` running [`LOUD`],
/// [`STUCK`] and [`FLOOD`].
fn stalling_project(dir: &Path) -> PathBuf {
    let root = noisy_project(dir);
    for (id, module) in [("loud", LOUD), ("stuck", STUCK), ("flood", FLOOD)] {
        std::fs::write(root.join(format!("nodes/{id}.ts")), module).unwrap();
        std::fs::write(
            root.join(format!("workflows/{id}.yaml")),
            format!(
                "fx: workflow/v1\nid: {id}\ntitle: {id}\nsteps:\n  s:\n    uses: ./nodes/{id}.ts#{id}\n\
                 outputs:\n  text: ${{{{ steps.s.outputs.text }}}}\n"
            ),
        )
        .unwrap();
    }
    root
}

/// Starts `command` with its stdout discarded and its stderr a pipe nobody reads (a pager left
/// unscrolled, a terminal paused with Ctrl-S), and returns once the pipe is full and the copy of
/// the host's stderr is blocked on it: the pipe holds text and has taken no more for half a
/// second. The pipe's reading end is returned with the command, to keep it open.
fn start_stalled(mut command: Command) -> (std::process::Child, std::io::PipeReader) {
    let (reader, writer) = std::io::pipe().unwrap();
    let mut child = command
        .stdout(Stdio::null())
        .stderr(writer)
        .spawn()
        .unwrap();
    let start = Instant::now();
    let (mut held, mut since) = (0, Instant::now());
    while start.elapsed() < Duration::from_secs(60) {
        let now = rustix::io::ioctl_fionread(&reader).unwrap();
        if now != held {
            (held, since) = (now, Instant::now());
        } else if held >= 8192 && since.elapsed() >= Duration::from_millis(500) {
            return (child, reader);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("the command's stderr never filled ({held} bytes)");
}

/// Sends the signal `name` to `child`.
fn signal(child: &std::process::Child, name: &str) {
    let sent = Command::new("kill")
        .args(["-s", name, "--", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(sent.success(), "kill -s {name} failed");
}

/// Waits up to 15 seconds for `child` to exit; its status and how long that took. A command
/// still running then is killed, and the test fails.
fn ends_within(mut child: std::process::Child) -> (std::process::ExitStatus, Duration) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(15) {
        if let Some(status) = child.try_wait().unwrap() {
            return (status, start.elapsed());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("the command was still running 15 seconds after it was interrupted");
}

/// Planning, interrupted while the copy of a host's stderr is blocked on a stderr nobody reads:
/// the command ends at once (the copy is waited for two seconds at most) and exits 130.
#[test]
fn an_interrupted_plan_ends_though_nobody_reads_its_stderr() {
    if !real_node() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = stalling_project(&std::fs::canonicalize(dir.path()).unwrap());
    let (child, _stalled) = start_stalled(fx_command(&root, &["plan", "loud"]));
    signal(&child, "INT");
    let (status, took) = ends_within(child);
    assert_eq!(status.code(), Some(130), "{status:?}");
    assert!(took < Duration::from_secs(6), "{took:?}");
}

/// A run, interrupted twice while the copy of a host's stderr is blocked on a stderr nobody
/// reads: the second Ctrl-C forces the exit at once, though the copy holds the stderr lock.
#[test]
fn a_run_interrupted_twice_ends_though_nobody_reads_its_stderr() {
    if !real_node() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = stalling_project(&std::fs::canonicalize(dir.path()).unwrap());
    let (child, _stalled) = start_stalled(run_command(&root, "stuck"));
    signal(&child, "INT");
    std::thread::sleep(Duration::from_millis(500));
    signal(&child, "INT");
    let (status, took) = ends_within(child);
    assert_eq!(status.code(), Some(130), "{status:?}");
    // The run's own cleanup would wait for the host's cancellation (5 seconds) and its copy.
    assert!(took < Duration::from_secs(4), "{took:?}");
}

/// The same run with its stderr a file, which has room: the forced exit says so.
#[test]
fn a_run_interrupted_twice_says_it_forces_the_exit() {
    if !real_node() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let root = stalling_project(&base);
    let log = base.join("run.log");
    let child = run_command(&root, "stuck")
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();
    // The body has printed everything and waits.
    let start = Instant::now();
    while !std::fs::read_to_string(&log)
        .unwrap()
        .contains(&format!("stuck line 2999 {}\n", "y".repeat(80)))
    {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "the body never ran"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    signal(&child, "INT");
    std::thread::sleep(Duration::from_millis(500));
    signal(&child, "INT");
    let (status, _) = ends_within(child);
    assert_eq!(status.code(), Some(130), "{status:?}");
    let captured = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        count(
            &captured,
            "interrupted again: forcing exit; run completion and cleanup are unverified"
        ),
        1,
        "{}",
        &captured[captured.len().saturating_sub(2000)..]
    );
}

/// A run whose body prints for good, interrupted twice while its stderr is a pipe read slowly (4
/// KiB every 10 ms), and so is full most of the time: the forced exit's message still arrives,
/// after what the host wrote, since it waits for the stream to take it once the copies are done.
#[test]
fn a_run_interrupted_twice_while_its_stderr_is_read_slowly_says_it_forces_the_exit() {
    if !real_node() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = stalling_project(&std::fs::canonicalize(dir.path()).unwrap());
    let (mut reader, writer) = std::io::pipe().unwrap();
    let child = run_command(&root, "flood")
        .stdout(Stdio::null())
        .stderr(writer)
        .spawn()
        .unwrap();
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let into = Arc::clone(&captured);
    let read = std::thread::spawn(move || {
        let mut buffer = vec![0u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => into.lock().unwrap().extend_from_slice(&buffer[..read]),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => panic!("{error}"),
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    // The body floods.
    let start = Instant::now();
    while captured.lock().unwrap().len() < 400_000 {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "the body never ran"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    signal(&child, "INT");
    std::thread::sleep(Duration::from_millis(500));
    signal(&child, "INT");
    let (status, took) = ends_within(child);
    read.join().unwrap();
    assert_eq!(status.code(), Some(130), "{status:?}");
    assert!(took < Duration::from_secs(4), "{took:?}");
    let captured = String::from_utf8_lossy(&captured.lock().unwrap()).into_owned();
    let message = "interrupted again: forcing exit; run completion and cleanup are unverified";
    assert_eq!(
        count(&captured, message),
        1,
        "{}",
        &captured[captured.len().saturating_sub(2000)..]
    );
}
