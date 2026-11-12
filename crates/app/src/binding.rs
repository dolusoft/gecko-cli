//! The binary's side of the binding context: `attach`'s arguments and its code from
//! stdin (`gecko attach -`), whether the streams are a terminal, the trust question's
//! answer from stdin and the hub connection the context's slices open. The rules and
//! the slices are `gecko-binding-domain` and `gecko-binding-module`.

use std::io::IsTerminal;

use gecko_binding_contract::{AttachRequest, HubOpener, StoredToken, TokenStore, TrustPrompt};
use gecko_hub_client::Hub;
use gecko_hub_protocol::Session;
use gecko_kernel::{Clock, Failure, IdSource, Interrupts, Outcome, Reporter};

use crate::args::Options;
use crate::output::interactive;
use crate::ports::Ports;
use crate::session;

/// `gecko attach <code>`, `gecko attach -`, either with `--trust-hub`.
#[derive(Debug, PartialEq, Eq)]
pub struct AttachArgs {
    /// The code, or `-` for stdin.
    pub code: String,
    pub trust_hub: bool,
}

pub fn attach_args(args: &[String]) -> Result<AttachArgs, Failure> {
    let usage = |why: &str| Failure::new("usage", format!("{why}; usage: gecko attach <code>"));
    let mut code = None;
    let mut trust_hub = false;
    for word in args {
        if word == "--trust-hub" {
            trust_hub = true;
            continue;
        }
        if word.starts_with('-') && word != "-" {
            return Err(usage(&format!("unknown option {word}")));
        }
        if code.is_some() {
            return Err(usage("one code at a time"));
        }
        code = Some(word.clone());
    }
    let code = code.ok_or_else(|| usage("no code: copy the line from the Gecko tab"))?;
    Ok(AttachArgs { code, trust_hub })
}

fn read_stdin() -> Result<String, Failure> {
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).map_err(|error| {
        Failure::new("usage", format!("cannot read the code from stdin: {error}"))
    })?;
    Ok(text.trim().to_owned())
}

/// The trust question's answer: one line from stdin, read on a blocking thread.
#[derive(Debug, Clone, Copy, Default)]
pub struct StdinPrompt;

impl TrustPrompt for StdinPrompt {
    async fn answer(&mut self) -> String {
        let answer = tokio::task::spawn_blocking(|| {
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).map(|_| line)
        });
        answer.await.ok().and_then(Result::ok).unwrap_or_default()
    }
}

/// Opens the call's hub connection with a token (`session::open_with`).
#[derive(Debug)]
pub struct Opener<'a, S, C, D> {
    pub store: &'a S,
    pub ports: &'a Ports<C, D>,
}

impl<S: TokenStore, C: Clock, D: IdSource> HubOpener for Opener<'_, S, C, D> {
    type Hub = Hub;

    async fn open_with(&self, token: StoredToken) -> Result<Session<Hub>, Failure> {
        session::open_with(self.store, token, self.ports).await
    }
}

pub async fn attach(
    args: &[String],
    options: &Options,
    store: &impl TokenStore,
    ports: &Ports<impl Clock, impl IdSource>,
    io: &mut impl Reporter,
    interrupts: &mut impl Interrupts,
) -> Result<Outcome, Failure> {
    let args = attach_args(args)?;
    let from_stdin = args.code == "-";
    let text = if from_stdin {
        read_stdin()?
    } else {
        args.code.clone()
    };
    let test_tty = ports.settings.test_tty;
    let request = AttachRequest {
        code: &text,
        from_stdin,
        trust_hub: args.trust_hub,
        json: options.json,
        terminal: interactive(test_tty, std::io::stdin().is_terminal())
            && interactive(test_tty, std::io::stderr().is_terminal()),
    };
    let opener = Opener { store, ports };
    let binding = ports.binding(store, &opener);
    gecko_binding_module::attach(
        &request,
        &binding,
        &ports.proxy,
        io,
        &mut StdinPrompt,
        interrupts,
    )
    .await
}

pub async fn detach(
    store: &impl TokenStore,
    ports: &Ports<impl Clock, impl IdSource>,
) -> Result<Outcome, Failure> {
    let opener = Opener { store, ports };
    gecko_binding_module::detach(&ports.binding(store, &opener)).await
}

pub async fn status(
    options: &Options,
    store: &impl TokenStore,
    ports: &Ports<impl Clock, impl IdSource>,
) -> Result<Outcome, Failure> {
    let opener = Opener { store, ports };
    gecko_binding_module::status(options.json, &ports.binding(store, &opener)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|w| (*w).to_owned()).collect()
    }

    #[test]
    fn attach_arguments() {
        assert_eq!(
            attach_args(&words(&["gk1_x"])).unwrap(),
            AttachArgs {
                code: "gk1_x".into(),
                trust_hub: false
            }
        );
        assert_eq!(
            attach_args(&words(&["--trust-hub", "-"])).unwrap(),
            AttachArgs {
                code: "-".into(),
                trust_hub: true
            }
        );
        assert!(
            attach_args(&words(&["gk1_x", "--trust-hub"]))
                .unwrap()
                .trust_hub
        );
        for bad in [&[][..], &["--new"], &["a", "b"], &["--trust-hub"]] {
            assert_eq!(
                attach_args(&words(bad)).unwrap_err().code,
                "usage",
                "{bad:?}"
            );
        }
    }
}
