//! The CLI's own commands: login and logout here, the binding commands in `binding`;
//! `help` in `help`. Their options come after the name: `gecko login --env dev`.

use gecko_hub_client::proxy::ProxyRule;

use crate::args::Options;
use crate::auth::Auth;
use crate::browser;
use crate::config::Config;
use crate::interrupt::Interrupts;
use crate::output::{Failure, Io, Outcome};

/// The names the CLI keeps for itself; any other first word goes to the tab.
pub const NAMES: [&str; 6] = ["login", "logout", "attach", "detach", "help", "status"];

pub fn usage(name: &str) -> Option<&'static str> {
    Some(match name {
        "login" => {
            "Usage: gecko login [--env <env>]\n\nDevice sign-in: prints a code and opens the approval page in the browser.\nOnce approved, the environment becomes the default.\n"
        }
        "logout" => {
            "Usage: gecko logout [--env <env>]\n\nRevokes the CLI's grant and deletes the token.\n"
        }
        "attach" => {
            "Usage: gecko attach [--new]\n\nBinds to a Gecko tab; the browser asks for approval. When already bound it\nopens no new tab; --new makes a new binding.\n"
        }
        "detach" => "Usage: gecko detach\n\nRemoves the binding.\n",
        "help" => {
            "Usage: gecko help [namespace|command] [--cursor <n>] [--limit <n>]\n\nThe bound tab's commands, filtered by your permissions and paged.\n"
        }
        "status" => {
            "Usage: gecko status\n\nSign-in, bound tab and hub state. Starts no binding.\nExit 0; 6 when not signed in, 4 when the hub cannot be reached.\n"
        }
        _ => return None,
    })
}

/// `--name value`, `--name=value`, or `--name` alone (true); as the Node build reads it.
#[derive(Debug, PartialEq, Eq)]
pub enum Flag {
    Absent,
    Set,
    Value(String),
}

pub fn own_option(args: &[String], name: &str) -> Flag {
    let long = format!("--{name}");
    let prefix = format!("--{name}=");
    let mut words = args.iter();
    while let Some(word) = words.next() {
        if *word == long {
            return match words.next() {
                Some(next) if !next.starts_with("--") => Flag::Value(next.clone()),
                _ => Flag::Set,
            };
        }
        if let Some(value) = word.strip_prefix(&prefix) {
            return Flag::Value(value.to_owned());
        }
    }
    Flag::Absent
}

/// The environment of a command: its own `--env`, the global `--env`, or the last login.
pub fn env_of(args: &[String], options: &Options, config: &Config) -> Result<String, Failure> {
    if let Flag::Value(env) = own_option(args, "env") {
        return Ok(env);
    }
    options
        .env
        .clone()
        .or_else(|| config.read_default_env())
        .ok_or_else(|| {
            Failure::new(
                "not-logged-in",
                "not signed in: run gecko login --env <env> first",
            )
        })
}

pub async fn login(
    args: &[String],
    options: &Options,
    config: &Config,
    io: &mut Io<'_>,
    interrupts: &mut Interrupts,
) -> Result<Outcome, Failure> {
    let env = env_of(args, options, config)?;
    let auth = Auth::new(&env, ProxyRule::from_env())?;
    let token = auth
        .device_login(
            config,
            |start| {
                let line = if browser::open(&start.verification_uri_complete) {
                    format!(
                        "Code: {} - the approval page opened in the browser",
                        start.user_code
                    )
                } else {
                    format!(
                        "Code: {} - open {} and enter the code",
                        start.user_code, start.verification_uri
                    )
                };
                io.info(&line);
            },
            interrupts,
        )
        .await?;
    config
        .write_default_env(&env)
        .map_err(|error| Failure::new("failed", format!("cannot save the environment: {error}")))?;
    Ok(Outcome::done(format!(
        "Signed in: {} ({env})",
        token.user.name
    )))
}

pub async fn logout(
    args: &[String],
    options: &Options,
    config: &Config,
) -> Result<Outcome, Failure> {
    let env = env_of(args, options, config)?;
    if let Some(token) = config.read_token(&env) {
        // An unknown alias still lets the file go: revoking needs the address, deleting
        // does not.
        if let Ok(auth) = Auth::new(&env, ProxyRule::from_env()) {
            auth.revoke(&token).await;
        }
    }
    config
        .delete_token(&env)
        .map_err(|error| Failure::new("failed", format!("cannot delete the token: {error}")))?;
    Ok(Outcome::done(format!("Signed out ({env})")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|w| (*w).to_owned()).collect()
    }

    #[test]
    fn own_options() {
        assert_eq!(
            own_option(&words(&["--env", "dev"]), "env"),
            Flag::Value("dev".into())
        );
        assert_eq!(
            own_option(&words(&["--env=dev"]), "env"),
            Flag::Value("dev".into())
        );
        assert_eq!(own_option(&words(&["--env", "--new"]), "env"), Flag::Set);
        assert_eq!(own_option(&words(&["--new"]), "new"), Flag::Set);
        assert_eq!(own_option(&words(&["x"]), "env"), Flag::Absent);
    }

    #[test]
    fn every_own_command_has_a_usage() {
        for name in NAMES {
            assert!(
                usage(name).is_some_and(|u| u.starts_with("Usage: gecko ")),
                "{name}"
            );
        }
        assert_eq!(usage("tasks.list"), None);
    }
}
