//! The CLI binary as a process: argv, stdin, environment in; exit code, stdout and
//! stderr out.
//!
//! The environment is the test's own, less every `GECKO_*` variable; `USERNAME`,
//! `PATH`, the proxy variables and coverage variables such as `LLVM_PROFILE_FILE` pass
//! through. Each [`Cli`] has its own config directory (`GECKO_CONFIG_DIR`) and sets
//! `NO_COLOR=1`.
//!
//! stdin has no single owner: the Ctrl+C hook (`GECKO_TEST_INTERRUPT_STDIN`),
//! `--line -` and `attach -` all read it, so one call uses one of them.

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::Value;
use tokio::sync::watch;
use tokio::time::Instant;

use crate::{EXIT_WAIT, WAIT};

/// The CLI with its own config directory.
#[derive(Debug)]
pub struct Cli {
    bin: PathBuf,
    dir: PathBuf,
    json: bool,
}

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

impl Cli {
    /// The binary at `bin` (a test passes `env!("CARGO_BIN_EXE_gecko")`), with a new,
    /// empty config directory that goes with the `Cli`.
    pub fn new(bin: impl Into<PathBuf>) -> Cli {
        let n = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join("gecko-testkit")
            .join(format!("{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_else(|error| panic!("{}: {error}", dir.display()));
        Cli {
            bin: bin.into(),
            dir,
            json: false,
        }
    }

    /// Every call of this `Cli` starts with `--json`.
    pub fn json(mut self) -> Cli {
        self.json = true;
        self
    }

    pub fn is_json(&self) -> bool {
        self.json
    }

    /// The config directory (`GECKO_CONFIG_DIR`).
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `token.json`, when there is one.
    pub fn token(&self) -> Option<Value> {
        let text = std::fs::read_to_string(self.dir.join("token.json")).ok()?;
        Some(serde_json::from_str(&text).expect("token.json is JSON"))
    }

    /// A call with `args`; [`Call::output`] runs it to the end.
    pub fn call<I, S>(&self, args: I) -> Call
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new(&self.bin);
        for (key, _) in std::env::vars_os() {
            if key
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("GECKO_")
            {
                command.env_remove(&key);
            }
        }
        command
            .env("GECKO_CONFIG_DIR", &self.dir)
            .env("NO_COLOR", "1");
        let mut shown = Vec::new();
        if self.json {
            command.arg("--json");
            shown.push("--json".to_owned());
        }
        for arg in args {
            let text = arg.as_ref().to_string_lossy();
            // A long argument is cut in messages, never in the call.
            shown.push(match text.char_indices().nth(60) {
                Some((at, _)) => format!("{}...", &text[..at]),
                None => text.into_owned(),
            });
            command.arg(arg);
        }
        Call {
            command,
            stdin: Input::Null,
            shown: shown.join(" "),
        }
    }

    /// Runs `args` to the end.
    pub async fn run<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.call(args).output().await
    }
}

impl Drop for Cli {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[derive(Debug)]
enum Input {
    Null,
    Text(String),
    /// Kept open: each `x` written is a Ctrl+C.
    Interrupts,
}

/// One CLI call, before it starts.
#[derive(Debug)]
pub struct Call {
    command: Command,
    stdin: Input,
    shown: String,
}

impl Call {
    pub fn env(mut self, key: &str, value: &str) -> Call {
        self.command.env(key, value);
        self
    }

    /// stdin is `text`, then closed (`attach -`, `--line -`).
    pub fn stdin(mut self, text: &str) -> Call {
        self.stdin = Input::Text(text.to_owned());
        self
    }

    /// `GECKO_TEST_INTERRUPT_STDIN=1`: [`Process::interrupt`] is a Ctrl+C.
    pub fn interrupts(mut self) -> Call {
        self.command.env("GECKO_TEST_INTERRUPT_STDIN", "1");
        self.stdin = Input::Interrupts;
        self
    }

    /// `GECKO_TEST_TTY=1`: the streams count as terminals.
    pub fn tty(self) -> Call {
        self.env("GECKO_TEST_TTY", "1")
    }

    /// Starts the process.
    pub fn spawn(mut self) -> Process {
        self.command
            .stdin(match self.stdin {
                Input::Null => Stdio::null(),
                _ => Stdio::piped(),
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = self
            .command
            .spawn()
            .unwrap_or_else(|error| panic!("gecko {}: {error}", self.shown));
        let mut stdin = child.stdin.take();
        if let Input::Text(text) = &self.stdin
            && let Some(mut pipe) = stdin.take()
        {
            // The process may end before it reads all of it; that is its answer.
            let _ = pipe.write_all(text.as_bytes());
        }
        let streams = Arc::new(Mutex::new(Streams::default()));
        let (changed_tx, changed) = watch::channel(0u64);
        let changed_tx = Arc::new(changed_tx);
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        reader(stdout, streams.clone(), changed_tx.clone(), false);
        reader(stderr, streams.clone(), changed_tx, true);
        Process {
            child,
            stdin,
            streams,
            changed,
            shown: self.shown,
            done: false,
        }
    }

    /// Runs the call to the end.
    pub async fn output(self) -> Output {
        self.spawn().output().await
    }
}

#[derive(Debug, Default)]
struct Streams {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    ended: u8,
}

fn reader(
    mut pipe: impl Read + Send + 'static,
    streams: Arc<Mutex<Streams>>,
    changed: Arc<watch::Sender<u64>>,
    is_stderr: bool,
) {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            let n = pipe.read(&mut buffer).unwrap_or(0);
            {
                let mut streams = lock(&streams);
                if n == 0 {
                    streams.ended += 1;
                } else if is_stderr {
                    streams.stderr.extend_from_slice(&buffer[..n]);
                } else {
                    streams.stdout.extend_from_slice(&buffer[..n]);
                }
            }
            changed.send_modify(|count| *count += 1);
            if n == 0 {
                break;
            }
        }
    });
}

fn lock(streams: &Mutex<Streams>) -> MutexGuard<'_, Streams> {
    streams
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A running CLI process.
#[derive(Debug)]
pub struct Process {
    child: Child,
    stdin: Option<ChildStdin>,
    streams: Arc<Mutex<Streams>>,
    changed: watch::Receiver<u64>,
    shown: String,
    done: bool,
}

impl Process {
    /// The process id; the only process the testkit ever stops.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// stderr so far.
    pub fn stderr(&self) -> String {
        text(&lock(&self.streams).stderr)
    }

    fn seen(&self) -> String {
        let streams = lock(&self.streams);
        format!(
            "gecko {}\nstdout: {:?}\nstderr: {:?}",
            self.shown,
            text(&streams.stdout),
            text(&streams.stderr)
        )
    }

    /// Waits until `done` holds for the streams, within `limit`; false when they ended
    /// first.
    async fn wait(&mut self, limit: std::time::Duration, done: impl Fn(&Streams) -> bool) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            {
                let streams = lock(&self.streams);
                if done(&streams) {
                    return true;
                }
                if streams.ended == 2 {
                    return false;
                }
            }
            match tokio::time::timeout_at(deadline, self.changed.changed()).await {
                Ok(Ok(())) => {}
                // Both readers are gone; the last look above decides.
                Ok(Err(_)) => {
                    let streams = lock(&self.streams);
                    return done(&streams);
                }
                Err(_) => {
                    let seen = self.seen();
                    self.kill();
                    panic!("waited {limit:?} in vain\n{seen}");
                }
            }
        }
    }

    /// Waits until stderr contains `pattern`, within [`WAIT`]; returns stderr so far.
    pub async fn stderr_until(&mut self, pattern: &str) -> String {
        let found = self
            .wait(WAIT, |streams| text(&streams.stderr).contains(pattern))
            .await;
        if !found {
            let seen = self.seen();
            panic!("the process ended without {pattern:?} on stderr\n{seen}");
        }
        self.stderr()
    }

    /// One Ctrl+C through the test hook (the call was made with [`Call::interrupts`]).
    pub fn interrupt(&mut self) {
        let stdin = self
            .stdin
            .as_mut()
            .expect("an interrupt needs Call::interrupts");
        stdin
            .write_all(b"x")
            .and_then(|()| stdin.flush())
            .expect("the process takes the interrupt");
    }

    fn kill(&mut self) {
        if !self.done {
            self.done = true;
            // Our own child, by its handle: nothing else is touched.
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// Waits for the end, within [`EXIT_WAIT`]; a process still running then is
    /// stopped and the test fails with what it wrote.
    pub async fn output(mut self) -> Output {
        self.stdin = None;
        let _ = self.wait(EXIT_WAIT, |streams| streams.ended == 2).await;
        self.done = true;
        let status = self
            .child
            .wait()
            .unwrap_or_else(|error| panic!("gecko {}: {error}", self.shown));
        let streams = lock(&self.streams);
        Output {
            code: status.code(),
            stdout: text(&streams.stdout),
            stderr: text(&streams.stderr),
            shown: self.shown.clone(),
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.kill();
    }
}

/// How a CLI call ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// `None` when a signal ended it.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    shown: String,
}

impl Output {
    /// Fails the test unless the exit code is `code`.
    pub fn expect_code(&self, code: i32) -> &Output {
        assert_eq!(self.code, Some(code), "{self}");
        self
    }

    /// stdout as one JSON value (`--json`).
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|error| panic!("stdout is not JSON ({error})\n{self}"))
    }
}

impl std::fmt::Display for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "gecko {} -> {:?}\nstdout: {:?}\nstderr: {:?}",
            self.shown, self.code, self.stdout, self.stderr
        )
    }
}
