//! `gecko`: the remote control of the Gecko web terminal. A line typed here runs in the
//! bound browser tab, and the result comes back (docs/rust-migration.md).
//!
//! Step 4b in progress: options, help, version, the dev-hub switch, login, logout,
//! attach, detach, status and the line path (command-console 1.7.0). `gecko help`
//! follows; until step 5 the Node build is the product.

mod args;
mod auth;
mod binding;
mod browser;
mod config;
mod interrupt;
mod line;
mod output;
mod own;
mod run;
mod session;

use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

use args::{Action, ArgError, Options};
use config::Config;
use interrupt::Interrupts;
use output::{Failure, Io};

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = r#"gecko: remote control of the Gecko web terminal

Usage:
  gecko [options] <command line>     runs the line in the bound Gecko tab
  gecko [options] --line "<line>"    sends the line as is
  gecko [options] --line -           reads the line from stdin

Own commands:
  login [--env <env>]     device sign-in; remembers the environment
  logout                  revokes the grant and deletes the token
  attach [--new]          binds to a Gecko tab (approved in the browser)
  detach                  removes the binding
  help [namespace|cmd]    the bound tab's commands (filtered, paged)
  status                  sign-in, bound tab and hub state
  dev-hub [options]       starts the development hub (localhost only)

Options (before the command name):
  --json            one JSON object; non-ASCII characters as \uXXXX
  --env <env>       environment (default: the last login)
  -h, --help        this text
  -v, --version     the version

Exit codes: 0 ok, 1 failed, 2 usage, 3 denied, 4 not run, 5 unknown outcome,
6 not signed in, 130 interrupted.
"#;

fn main() -> ExitCode {
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let code = run(
        std::env::args_os().skip(1),
        &mut stdout.lock(),
        &mut stderr.lock(),
    );
    ExitCode::from(code)
}

fn run(args: impl IntoIterator<Item = OsString>, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let (options, action) = match args::parse(args) {
        Ok(parsed) => parsed,
        Err(error) => return usage_error(err, &error),
    };
    match action {
        Action::Help => {
            let _ = out.write_all(USAGE.as_bytes());
            0
        }
        Action::Version => {
            let _ = writeln!(out, "{VERSION}");
            0
        }
        Action::DevHub(rest) => dev_hub(&rest, err),
        Action::Command(argv) => command(&argv, &options, out, err),
        Action::Line(line) => tab_line(None, Some(line), &options, out, err),
    }
}

/// An own command or a line for the tab.
fn command(argv: &[String], options: &Options, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let name = argv[0].as_str();
    let rest = &argv[1..];
    if !own::NAMES.contains(&name) {
        return tab_line(Some(argv.to_vec()), None, options, out, err);
    }
    if rest.iter().any(|a| a == "--help" || a == "-h") {
        let _ = out.write_all(own::usage(name).unwrap_or_default().as_bytes());
        return 0;
    }
    if name == "help" {
        let _ = writeln!(
            err,
            "gecko {VERSION}: `help` is not available in the Rust build yet (step 4b); use the Node build."
        );
        return 2;
    }
    execute(options, out, err, Job::Own { name, rest })
}

/// A line for the bound tab, from argv or `--line`.
fn tab_line(
    argv: Option<Vec<String>>,
    line: Option<String>,
    options: &Options,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u8 {
    let auto_pair = line::may_auto_pair(options.json, line.is_some());
    let input = line::input_of(argv, line);
    execute(options, out, err, Job::Line { input, auto_pair })
}

/// What one call does once the runtime is up.
enum Job<'a> {
    Own {
        name: &'a str,
        rest: &'a [String],
    },
    Line {
        input: Result<gecko_wire::request::Input, Failure>,
        auto_pair: bool,
    },
}

/// Runs one job on a single-thread runtime and prints its result.
fn execute(options: &Options, out: &mut dyn Write, err: &mut dyn Write, job: Job<'_>) -> u8 {
    let mut io = Io {
        out,
        err,
        color: Io::stderr_color(),
        json: options.json,
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return io.fail(&Failure::new("failed", format!("cannot start: {error}"))),
    };
    let config = Config::from_env();
    let result = runtime.block_on(async {
        let mut interrupts = Interrupts::listen();
        let (io, config, interrupts) = (&mut io, &config, &mut interrupts);
        match job {
            Job::Own { name, rest } => match name {
                "login" => own::login(rest, options, config, io, interrupts).await,
                "logout" => own::logout(rest, options, config).await,
                "attach" => binding::attach(rest, options, config, io, interrupts).await,
                "detach" => binding::detach(rest, options, config).await,
                _ => binding::status(rest, options, config).await,
            },
            Job::Line { input, auto_pair } => {
                let this = line::LineRun {
                    options,
                    config,
                    auto_pair,
                };
                match input {
                    Ok(input) => line::run(this, input, io, interrupts).await,
                    Err(failure) => Err(failure),
                }
            }
        }
    });
    // A blocking stdin read (the test interrupt hook) must not hold the process open.
    runtime.shutdown_background();
    match result {
        Ok(outcome) => io.emit(&outcome),
        Err(failure) => io.fail(&failure),
    }
}

fn usage_error(err: &mut dyn Write, error: &ArgError) -> u8 {
    let _ = writeln!(err, "gecko: {error}\nSee: gecko --help");
    2
}

#[cfg(feature = "dev-hub")]
fn dev_hub(rest: &[String], err: &mut dyn Write) -> u8 {
    gecko_dev_hub::run(rest, err)
}

#[cfg(not(feature = "dev-hub"))]
fn dev_hub(_rest: &[String], err: &mut dyn Write) -> u8 {
    let _ = writeln!(err, "gecko: this build has no dev hub");
    2
}
