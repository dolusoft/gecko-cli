//! `gecko`: the remote control of the Gecko web terminal. A line typed here runs in the
//! bound browser tab, and the result comes back.
//!
//! Options, help, version, the dev-hub switch, attach, detach, status, `gecko help`
//! and the line path (command-console 1.7).
//!
//! This file is the composition root: the environment is read here and nowhere else,
//! and the ports (`ports.rs`, `interrupt.rs`, `output.rs`) are built here.

mod args;
mod binding;
mod command;
mod format;
mod interrupt;
mod output;
mod own;
mod ports;
mod session;

use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

use args::{Action, ArgError, Options};
use gecko_binding_module::adapters::fs::Config;
use gecko_hub_client::ProxyRule;
use gecko_kernel::{Failure, Reporter, Settings};
use interrupt::CtrlC;
use output::Io;
use ports::{Ports, RandomIds, SystemClock};

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = r#"gecko: remote control of the Gecko web terminal

Usage:
  gecko [options] <command line>     runs the line in the bound Gecko tab
  gecko [options] --line "<line>"    sends the line as is
  gecko [options] --line -           reads the line from stdin

Own commands:
  attach <code>           binds to the Gecko tab the code came from ("Attach CLI"
                          in the tab's terminal panel); `attach -` reads it from stdin
  detach                  ends the binding and deletes its token
  help [namespace|cmd]    the bound tab's commands (filtered, paged)
  status                  bound tab, approval level and hub state
  dev-hub [options]       starts the development hub (localhost only)

Options (before the command name):
  --json            one JSON object; non-ASCII characters as \uXXXX
  --out <file>      writes the result to the file as UTF-8
  --view <id>       runs the command in this Data View
  -h, --help        this text
  -v, --version     the version

Exit codes: 0 ok, 1 failed, 2 usage, 3 denied, 4 not run, 5 unknown outcome,
6 invalid code or ended binding, 130 interrupted.
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
        Action::DevHub(rest) => dev_hub(&rest, out, err),
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
        return execute(
            options,
            out,
            err,
            Job::Help {
                args: rest.to_vec(),
            },
        );
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
    let input = command::input_of(argv, line);
    // The tab's `help` command is not open to the CLI: a `help …` line asks as
    // `gecko help …` does.
    if let Ok(gecko_wire::Input::Line(text)) = &input
        && let Some(args) = gecko_command_module::line_args(text)
    {
        return execute(options, out, err, Job::Help { args });
    }
    execute(options, out, err, Job::Line { input })
}

/// What one call does once the runtime is up.
enum Job<'a> {
    Own {
        name: &'a str,
        rest: &'a [String],
    },
    Line {
        input: Result<gecko_wire::Input, Failure>,
    },
    Help {
        args: Vec<String>,
    },
}

/// Runs one job and prints its result: to stdout, or with `--out` to the file.
fn execute(options: &Options, out: &mut dyn Write, err: &mut dyn Write, job: Job<'_>) -> u8 {
    let Some(path) = &options.out else {
        return execute_to(options, out, err, job, false);
    };
    let mut buffer = Vec::new();
    let code = execute_to(options, &mut buffer, err, job, true);
    if let Err(error) = std::fs::write(path, &buffer) {
        let _ = writeln!(err, "error (failed): cannot write {path}: {error}");
        return if code == 0 { 1 } else { code };
    }
    code
}

/// What the CLI reads from its environment (docs/test-hooks.md), read once per call.
struct Environment {
    settings: Settings,
    proxy: ProxyRule,
    config: Config,
    /// `NO_COLOR` is set.
    no_color: bool,
    /// `GECKO_TEST_INTERRUPT_STDIN=1`.
    interrupt_stdin: bool,
}

/// The process environment; the only place the CLI reads it.
fn environment() -> Environment {
    let var = |name: &str| std::env::var(name).ok();
    let var_os = |name: &str| std::env::var_os(name);
    Environment {
        settings: Settings::from_lookup(var),
        proxy: ProxyRule::from_lookup(var),
        config: Config::from_lookup(var_os),
        no_color: var_os("NO_COLOR").is_some(),
        interrupt_stdin: var_os("GECKO_TEST_INTERRUPT_STDIN").is_some_and(|v| v == "1"),
    }
}

/// Runs one job on a single-thread runtime and prints its result.
fn execute_to(
    options: &Options,
    out: &mut dyn Write,
    err: &mut dyn Write,
    job: Job<'_>,
    to_file: bool,
) -> u8 {
    let Environment {
        settings,
        proxy,
        config,
        no_color,
        interrupt_stdin,
    } = environment();
    let mut io = Io::new(out, err, options.json, to_file, settings.test_tty, no_color);
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return io.fail(&Failure::new("failed", format!("cannot start: {error}"))),
    };
    let ports = Ports {
        settings,
        clock: SystemClock,
        ids: RandomIds,
        cli_version: VERSION,
        proxy,
    };
    let is_help = matches!(job, Job::Help { .. });
    let result = runtime.block_on(async {
        let mut interrupts = CtrlC::listen(interrupt_stdin);
        let (io, config, ports, interrupts) = (&mut io, &config, &ports, &mut interrupts);
        match job {
            Job::Own { name, rest } => match name {
                "attach" => binding::attach(rest, options, config, ports, io, interrupts).await,
                "detach" => binding::detach(config, ports).await,
                _ => binding::status(options, config, ports).await,
            },
            Job::Line { input } => match input {
                Ok(input) => command::line(options, config, ports, input, io, interrupts).await,
                Err(failure) => Err(failure),
            },
            Job::Help { args } => command::help(&args, config, ports, io, interrupts).await,
        }
    });
    // A blocking stdin read (the test interrupt hook) must not hold the process open.
    runtime.shutdown_background();
    match result {
        Ok(outcome) => io.emit(&outcome),
        // Without a binding there is no tab to ask: the CLI's own usage (an agent
        // reading JSON gets the `no-binding` error, UX U9).
        Err(failure) if is_help && failure.code == "no-binding" && !options.json => {
            let _ = io.out.write_all(USAGE.as_bytes());
            io.info(
                "For the Gecko commands, first: gecko attach <code> (copy the line from the Gecko tab)",
            );
            0
        }
        Err(failure) => io.fail(&failure),
    }
}

fn usage_error(err: &mut dyn Write, error: &ArgError) -> u8 {
    let _ = writeln!(err, "gecko: {error}\nSee: gecko --help");
    2
}

#[cfg(feature = "dev-hub")]
fn dev_hub(rest: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    gecko_dev_hub::run(rest, out, err)
}

#[cfg(not(feature = "dev-hub"))]
fn dev_hub(_rest: &[String], _out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let _ = writeln!(err, "gecko: this build has no dev hub");
    2
}
