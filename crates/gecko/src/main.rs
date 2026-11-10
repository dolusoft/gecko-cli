//! `gecko`: the remote control of the Gecko web terminal. A line typed here runs in the
//! bound browser tab, and the result comes back (docs/rust-migration.md).
//!
//! Step 4b in progress: options, help, version, the dev-hub switch, login and logout.
//! The binding commands and the line path follow; until step 5 the Node build is the
//! product.

mod args;
mod auth;
mod browser;
mod config;
mod interrupt;
mod output;
mod own;

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
    }
}

/// An own command or a line for the tab.
fn command(argv: &[String], options: &Options, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let name = argv[0].as_str();
    let rest = &argv[1..];
    if own::NAMES.contains(&name) && rest.iter().any(|a| a == "--help" || a == "-h") {
        let _ = out.write_all(own::usage(name).unwrap_or_default().as_bytes());
        return 0;
    }
    if !matches!(name, "login" | "logout") {
        let _ = writeln!(
            err,
            "gecko {VERSION}: `{name}` is not available in the Rust build yet (step 4b); use the Node build."
        );
        return 2;
    }
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
        match name {
            "login" => own::login(rest, options, &config, &mut io, &mut interrupts).await,
            _ => own::logout(rest, options, &config).await,
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
