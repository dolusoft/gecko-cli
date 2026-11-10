//! Command-line parsing (§10.1 of the desktop CLI spec): the CLI's own options come
//! before the command name; everything from the command name on belongs to the tab and
//! is passed on as argv without being read (§5.1).
//!
//! Arguments are read with `args_os`, so text that is not valid Unicode is an error
//! with exit code 2 rather than a crash.

use std::ffi::OsString;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub json: bool,
    pub env: Option<String>,
    /// `--view <id>`: the view the command runs in.
    pub view: Option<String>,
    /// `--out <file>`: the result goes to this file as UTF-8 instead of stdout.
    pub out: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Help,
    Version,
    /// `gecko dev-hub ...`
    DevHub(Vec<String>),
    /// Any other command name and its arguments.
    Command(Vec<String>),
    /// `--line "<line>"`, or `--line -` (from stdin): sent as typed.
    Line(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum ArgError {
    NotUnicode {
        position: usize,
    },
    MissingValue(&'static str),
    UnknownOption(String),
    /// `--line` and a command name together.
    LineAndCommand,
    /// A CLI option right after the command name (B15); carries the fixed order.
    Misplaced(String),
}

impl std::fmt::Display for ArgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArgError::NotUnicode { position } => {
                write!(f, "argument {position} is not valid Unicode")
            }
            ArgError::MissingValue(option) => write!(f, "{option} needs a value"),
            ArgError::UnknownOption(option) => write!(
                f,
                "unknown option {option}; options of a command go after its name"
            ),
            ArgError::LineAndCommand => {
                write!(f, "--line takes the whole line; give no command after it")
            }
            ArgError::Misplaced(fixed) => {
                write!(f, "CLI options come before the command: {fixed}")
            }
        }
    }
}

/// Options the CLI keeps; right after a command name they are a mistake (B15).
const CLI_ONLY: [&str; 3] = ["--json", "--out", "--line"];

/// `gecko tasks.list --json`: the first word after the command name is a CLI option.
/// Only that position is checked: elsewhere it may be a field's value (§5.1).
fn misplaced(argv: &[String]) -> Option<String> {
    let word = argv.get(1)?;
    let flag = word.split('=').next().unwrap_or("");
    if !CLI_ONLY.contains(&flag) {
        return None;
    }
    let takes_value = flag != "--json" && !word.contains('=');
    let mut moved = vec![word.clone()];
    if takes_value {
        moved.push(argv.get(2).cloned().unwrap_or_else(|| "<value>".to_owned()));
    }
    let skip = if takes_value { 3 } else { 2 };
    let mut fixed = vec!["gecko".to_owned()];
    fixed.extend(moved);
    fixed.push(argv[0].clone());
    fixed.extend(argv.iter().skip(skip).cloned());
    Some(fixed.join(" "))
}

/// Parses the arguments after the program name.
pub fn parse<I>(args: I) -> Result<(Options, Action), ArgError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut words = Vec::new();
    for (index, arg) in args.into_iter().enumerate() {
        match arg.into_string() {
            Ok(word) => words.push(word),
            Err(_) => {
                return Err(ArgError::NotUnicode {
                    position: index + 1,
                });
            }
        }
    }

    let mut options = Options::default();
    let mut line = None;
    let mut rest = words.into_iter().peekable();
    while let Some(word) = rest.next() {
        let (name, inline) = match word.split_once('=') {
            Some((name, value)) if name.starts_with("--") => {
                (name.to_owned(), Some(value.to_owned()))
            }
            _ => (word.clone(), None),
        };
        let mut value = |option: &'static str| match inline.clone().or_else(|| rest.next()) {
            Some(value) => Ok(value),
            None => Err(ArgError::MissingValue(option)),
        };
        match name.as_str() {
            "--help" | "-h" if inline.is_none() => return Ok((options, Action::Help)),
            "--version" | "-v" if inline.is_none() => return Ok((options, Action::Version)),
            "--json" if inline.is_none() => options.json = true,
            "--env" => options.env = Some(value("--env")?),
            "--view" => options.view = Some(value("--view")?),
            "--out" => options.out = Some(value("--out")?),
            "--line" => line = Some(value("--line")?),
            "--" => break,
            _ if word.starts_with('-') && word.len() > 1 => {
                return Err(ArgError::UnknownOption(name));
            }
            _ => {
                let mut argv = vec![word];
                argv.extend(rest.by_ref());
                return finish(options, line, argv);
            }
        }
    }
    finish(options, line, rest.collect())
}

fn finish(
    options: Options,
    line: Option<String>,
    argv: Vec<String>,
) -> Result<(Options, Action), ArgError> {
    if let Some(line) = line {
        if !argv.is_empty() {
            return Err(ArgError::LineAndCommand);
        }
        return Ok((options, Action::Line(line)));
    }
    let Some(first) = argv.first() else {
        return Ok((options, Action::Help));
    };
    if first == "dev-hub" {
        return Ok((options, Action::DevHub(argv[1..].to_vec())));
    }
    if let Some(fixed) = misplaced(&argv) {
        return Err(ArgError::Misplaced(fixed));
    }
    Ok((options, Action::Command(argv)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<(Options, Action), ArgError> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn no_arguments_is_help() {
        assert_eq!(run(&[]).unwrap().1, Action::Help);
    }

    #[test]
    fn help_and_version() {
        assert_eq!(run(&["--help"]).unwrap().1, Action::Help);
        assert_eq!(run(&["-h"]).unwrap().1, Action::Help);
        assert_eq!(run(&["--version"]).unwrap().1, Action::Version);
        assert_eq!(run(&["-v"]).unwrap().1, Action::Version);
    }

    #[test]
    fn options_stop_at_the_command_name() {
        let (options, action) = run(&[
            "--json",
            "--env",
            "dev",
            "tasks.list",
            "--q",
            "--json",
            "--help",
        ])
        .unwrap();
        assert!(options.json);
        assert_eq!(options.env.as_deref(), Some("dev"));
        assert_eq!(
            action,
            Action::Command(vec![
                "tasks.list".into(),
                "--q".into(),
                "--json".into(),
                "--help".into()
            ])
        );
    }

    #[test]
    fn line_and_view() {
        let (options, action) =
            run(&["--view=v-1", "--out", "r.txt", "--line", "tasks.list --q a"]).unwrap();
        assert_eq!(options.view.as_deref(), Some("v-1"));
        assert_eq!(options.out.as_deref(), Some("r.txt"));
        assert_eq!(action, Action::Line("tasks.list --q a".into()));
        assert_eq!(run(&["--line", "-"]).unwrap().1, Action::Line("-".into()));
        assert_eq!(
            run(&["--line", "x", "tasks.list"]).unwrap_err(),
            ArgError::LineAndCommand
        );
        assert_eq!(
            run(&["--", "--weird"]).unwrap().1,
            Action::Command(vec!["--weird".into()])
        );
    }

    #[test]
    fn cli_option_right_after_the_name() {
        assert_eq!(
            run(&["tasks.list", "--json", "--limit", "5"]).unwrap_err(),
            ArgError::Misplaced("gecko --json tasks.list --limit 5".into())
        );
        assert_eq!(
            run(&["tasks.list", "--out", "a.json"]).unwrap_err(),
            ArgError::Misplaced("gecko --out a.json tasks.list".into())
        );
        assert!(run(&["search", "--q", "--json"]).is_ok());
    }

    #[test]
    fn env_with_equals() {
        let (options, _) = run(&["--env=prod", "status"]).unwrap();
        assert_eq!(options.env.as_deref(), Some("prod"));
    }

    #[test]
    fn dev_hub_takes_the_rest() {
        assert_eq!(
            run(&["dev-hub", "--port", "0"]).unwrap().1,
            Action::DevHub(vec!["--port".into(), "0".into()])
        );
    }

    #[test]
    fn errors() {
        assert_eq!(
            run(&["--env"]).unwrap_err(),
            ArgError::MissingValue("--env")
        );
        assert_eq!(
            run(&["--nope", "x"]).unwrap_err(),
            ArgError::UnknownOption("--nope".into())
        );
    }

    #[cfg(windows)]
    #[test]
    fn lone_surrogate_is_not_unicode() {
        use std::os::windows::ffi::OsStringExt;
        let bad = OsString::from_wide(&[0x61, 0xd800]);
        let result = parse([OsString::from("x"), bad]);
        assert_eq!(result.unwrap_err(), ArgError::NotUnicode { position: 2 });
    }

    #[cfg(unix)]
    #[test]
    fn invalid_utf8_is_not_unicode() {
        use std::os::unix::ffi::OsStringExt;
        let bad = OsString::from_vec(vec![0x61, 0xff]);
        let result = parse([bad]);
        assert_eq!(result.unwrap_err(), ArgError::NotUnicode { position: 1 });
    }
}
