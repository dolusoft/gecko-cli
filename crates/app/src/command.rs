//! The binary's side of the command context: the line's input (argv, `--line`, or
//! `--line -` from stdin), and the hub connection the context's slices run on. The
//! rules and the slices are `gecko-command-domain` and `gecko-command-module`; opening
//! and closing the connection is the binary's.

use gecko_binding_contract::TokenStore;
use gecko_command_domain::line::precheck;
use gecko_hub_protocol::HubConnection;
use gecko_kernel::{Clock, Failure, IdSource, Interrupts, Outcome, Reporter};
use gecko_wire::Input;

use crate::args::Options;
use crate::ports::Ports;
use crate::session;

/// The input of a line: argv, `--line`, or `--line -` from stdin.
pub fn input_of(argv: Option<Vec<String>>, line: Option<String>) -> Result<Input, Failure> {
    match (argv, line) {
        (Some(argv), _) => Ok(Input::Argv(argv)),
        (None, Some(line)) if line == "-" => {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).map_err(|error| {
                Failure::new("usage", format!("cannot read the line from stdin: {error}"))
            })?;
            let text = text.strip_suffix('\n').unwrap_or(&text);
            let text = text.strip_suffix('\r').unwrap_or(text);
            Ok(Input::Line(text.to_owned()))
        }
        (None, Some(line)) => Ok(Input::Line(line)),
        (None, None) => Err(Failure::new("usage", "no command")),
    }
}

/// Sends a line to the bound tab. Without a binding it ends with `no-binding` at once:
/// a binding starts only from a code copied out of a tab (attach spec §3.1).
pub async fn line(
    options: &Options,
    store: &impl TokenStore,
    ports: &Ports<impl Clock, impl IdSource>,
    input: Input,
    io: &mut impl Reporter,
    interrupts: &mut impl Interrupts,
) -> Result<Outcome, Failure> {
    let name = precheck(&input)?;
    let mut session = session::open(store, ports).await?;
    let result = gecko_command_module::run_line(
        &mut session,
        &ports.command(),
        &input,
        &name,
        options.view.as_deref(),
        io,
        interrupts,
    )
    .await;
    session.hub.close().await;
    result
}

/// `gecko help …`: the bound tab's commands.
pub async fn help(
    args: &[String],
    store: &impl TokenStore,
    ports: &Ports<impl Clock, impl IdSource>,
    io: &mut impl Reporter,
    interrupts: &mut impl Interrupts,
) -> Result<Outcome, Failure> {
    let mut session = session::open(store, ports).await?;
    let result = gecko_command_module::help(
        &mut session.hub,
        &session.status,
        &ports.command(),
        args,
        io,
        interrupts,
    )
    .await;
    session.hub.close().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_of_a_line() {
        let words = vec!["tasks".to_owned(), "list".to_owned()];
        assert_eq!(
            input_of(Some(words.clone()), None).unwrap(),
            Input::Argv(words)
        );
        assert_eq!(
            input_of(None, Some("tasks list".to_owned())).unwrap(),
            Input::Line("tasks list".to_owned())
        );
        let none = input_of(None, None).unwrap_err();
        assert_eq!(
            (none.code.as_str(), none.message.as_str()),
            ("usage", "no command")
        );
    }
}
