//! `gecko help [namespace|command] [--cursor <n>] [--limit <n>]`: the bound tab's
//! commands, filtered by the user's permissions and paged. The tab's own `help`
//! command is not open to the CLI; the CLI sends a `help-request` with the raw
//! arguments and prints the tab's `help-result` (command-console 1.7.0).

use gecko_command_domain::help::{Answer, ended, outcome_of};
use gecko_command_domain::line::{capabilities_of, live_binding};
use gecko_hub_protocol::HubConnection;
use gecko_hub_protocol::hub::HubStatus;
use gecko_kernel::{Clock, Failure, IdSource, Interrupts, Outcome, Reporter};

use crate::Ports;
use crate::exchange::ask;

/// `gecko help …` on an open connection whose `status` is `status`.
pub async fn help<H: HubConnection>(
    hub: &mut H,
    status: &HubStatus,
    ports: &Ports<'_, impl Clock, impl IdSource>,
    args: &[String],
    io: &mut impl Reporter,
    interrupts: &mut impl Interrupts,
) -> Result<Outcome, Failure> {
    let caps = capabilities_of(live_binding(status)?, ports.cli_version)?;
    let v = caps.envelope().unwrap_or(1);
    let width = io.columns().and_then(|c| u16::try_from(c).ok());
    let answer = ask(hub, ports, v, args, io.json(), width, interrupts).await?;
    if let Some(outcome) = ended(&answer) {
        return Ok(outcome);
    }
    let Answer::Result(help) = answer else {
        return Err(Failure::new("failed", "no help result"));
    };
    Ok(outcome_of(&help, io.json()))
}
