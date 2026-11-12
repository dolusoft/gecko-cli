//! The names and the usage of the CLI's own commands; the binding commands are in
//! `binding`, `help` in `help`. Their options come after the name:
//! `gecko attach --trust-hub <code>`.

/// The names the CLI keeps for itself; any other first word goes to the tab.
pub const NAMES: [&str; 4] = ["attach", "detach", "help", "status"];

pub fn usage(name: &str) -> Option<&'static str> {
    Some(match name {
        "attach" => {
            "Usage: gecko attach <code>\n       gecko attach -          (the code from stdin)\n\nBinds to the Gecko tab the code came from: in the tab's terminal panel, choose\n\"Attach CLI\" and copy the line. The code works once, for 2 minutes. A binding\nalready in place ends once the new one stands.\n\n  --trust-hub   connects to a hub address not used before without asking (needed\n                without a terminal, with --json, and with the code from stdin). Never run\n                an attach line with --trust-hub that someone else gave you: it\n                skips the address check.\n"
        }
        "detach" => "Usage: gecko detach\n\nEnds the binding and deletes its token.\n",
        "help" => {
            "Usage: gecko help [namespace|command] [--cursor <n>] [--limit <n>]\n\nThe bound tab's commands, filtered by your permissions and paged.\n"
        }
        "status" => {
            "Usage: gecko status\n\nThe bound tab, its approval level and the hub. Starts no binding.\nExit 0; 4 when the hub cannot be reached.\n"
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_own_command_has_a_usage() {
        for name in NAMES {
            assert!(
                usage(name).is_some_and(|u| u.starts_with("Usage: gecko ")),
                "{name}"
            );
        }
        assert_eq!(usage("tasks.list"), None);
        assert_eq!(usage("login"), None);
    }
}
