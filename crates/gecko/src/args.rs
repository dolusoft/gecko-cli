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
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Help,
    Version,
    /// `gecko dev-hub ...`
    DevHub(Vec<String>),
    /// Any other command name and its arguments.
    Command(Vec<String>),
}

#[derive(Debug, PartialEq, Eq)]
pub enum ArgError {
    NotUnicode { position: usize },
    MissingValue(&'static str),
    UnknownOption(String),
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
        }
    }
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
    let mut rest = words.into_iter();
    while let Some(word) = rest.next() {
        match word.as_str() {
            "--help" | "-h" => return Ok((options, Action::Help)),
            "--version" | "-v" => return Ok((options, Action::Version)),
            "--json" => options.json = true,
            "--env" => match rest.next() {
                Some(value) => options.env = Some(value),
                None => return Err(ArgError::MissingValue("--env")),
            },
            _ if word.starts_with("--env=") => {
                options.env = Some(word["--env=".len()..].to_owned());
            }
            _ if word.starts_with('-') && word.len() > 1 => {
                return Err(ArgError::UnknownOption(word));
            }
            _ => {
                let tail: Vec<String> = rest.collect();
                let action = if word == "dev-hub" {
                    Action::DevHub(tail)
                } else {
                    let mut argv = vec![word];
                    argv.extend(tail);
                    Action::Command(argv)
                };
                return Ok((options, action));
            }
        }
    }
    Ok((options, Action::Help))
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
        let (options, action) =
            run(&["--json", "--env", "dev", "tasks.list", "--json", "--help"]).unwrap();
        assert!(options.json);
        assert_eq!(options.env.as_deref(), Some("dev"));
        assert_eq!(
            action,
            Action::Command(vec!["tasks.list".into(), "--json".into(), "--help".into()])
        );
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
