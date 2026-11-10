//! The bound tab's capability list (§5.8; command-console 1.7.0 "Capabilities"): the
//! JSON text of one object, published to the hub in one to three parts.

use serde::Serialize;
use serde_json::{Map, Value};

use crate::class::Danger;
use crate::reply::ENVELOPE_MAX_BYTES;

/// The most parts of one list.
pub const PARTS_MAX: usize = 3;

/// The features this CLI needs (§5.7 rule 2); a tab without one of them is too old.
pub const REQUIRED_FEATURES: [&str; 6] = [
    "argv",
    "received-v2",
    "suggest-argv",
    "help-v2",
    "error-class",
    "capabilities-v2",
];

/// The envelope versions this CLI speaks.
pub const ENVELOPES: [u64; 1] = [1];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Command {
    pub name: String,
    pub aliases: Vec<String>,
    pub danger: Danger,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub envelopes: Vec<u64>,
    pub features: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_cli: Option<String>,
    pub revision: String,
    pub sensitive: Vec<String>,
    pub commands: Vec<Command>,
}

/// ASCII case folding, as both sides compare names (§5.3).
pub fn same_name(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

impl Capabilities {
    /// Whether `name` is a sensitive command or alias.
    pub fn is_sensitive(&self, name: &str) -> bool {
        self.sensitive.iter().any(|s| same_name(s, name))
    }

    /// The command `name` names, by its name or an alias.
    pub fn command(&self, name: &str) -> Option<&Command> {
        self.commands.iter().find(|c| {
            same_name(&c.name, name) || c.aliases.iter().any(|alias| same_name(alias, name))
        })
    }

    /// The features this CLI needs that the tab does not declare.
    pub fn missing_features(&self) -> Vec<&'static str> {
        REQUIRED_FEATURES
            .into_iter()
            .filter(|need| !self.features.iter().any(|have| have == need))
            .collect()
    }

    /// The highest envelope version both sides speak.
    pub fn envelope(&self) -> Option<u64> {
        ENVELOPES
            .into_iter()
            .filter(|v| self.envelopes.contains(v))
            .max()
    }
}

fn strings(fields: &Map<String, Value>, key: &str) -> Result<Vec<String>, String> {
    fields
        .get(key)
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .ok_or_else(|| format!("{key}: expected strings"))
}

/// Joins the parts and reads the object. Any fault makes the whole list unreadable:
/// the CLI then acts as if there were no list.
pub fn read_parts(parts: &[String]) -> Result<Capabilities, String> {
    if parts.is_empty() || parts.len() > PARTS_MAX {
        return Err("expected 1 to 3 parts".to_owned());
    }
    if parts.iter().any(|part| part.len() > ENVELOPE_MAX_BYTES) {
        return Err("a part is over 16384 bytes".to_owned());
    }
    let joined = parts.concat();
    let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(&joined) else {
        return Err("not a JSON object".to_owned());
    };
    let envelopes = fields
        .get("envelopes")
        .and_then(Value::as_array)
        .and_then(|items| items.iter().map(Value::as_u64).collect::<Option<Vec<_>>>())
        .ok_or("envelopes: expected integers")?;
    let revision = match fields.get("revision") {
        Some(Value::String(r)) if !r.is_empty() => r.clone(),
        _ => return Err("revision: expected a non-empty string".to_owned()),
    };
    let min_cli = match fields.get("minCli") {
        None => None,
        Some(Value::String(v)) => Some(v.clone()),
        Some(_) => return Err("minCli: expected a string".to_owned()),
    };
    let commands = fields
        .get("commands")
        .and_then(Value::as_array)
        .ok_or("commands: expected an array")?
        .iter()
        .map(|command| {
            let Value::Object(command) = command else {
                return Err("commands: expected objects".to_owned());
            };
            let name = command
                .get("name")
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty())
                .ok_or("commands: name expected")?;
            let danger = command
                .get("danger")
                .and_then(Value::as_str)
                .ok_or("commands: danger expected")?;
            Ok(Command {
                name: name.to_owned(),
                aliases: strings(command, "aliases")?,
                // An unknown danger is a change (rule 4).
                danger: if danger == "read" {
                    Danger::Read
                } else {
                    Danger::Change
                },
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Capabilities {
        envelopes,
        features: strings(&fields, "features")?,
        min_cli,
        revision,
        sensitive: strings(&fields, "sensitive")?,
        commands,
    })
}

/// "0.3.0" < "0.10.0": dotted numeric parts, compared in turn; anything after a `-`
/// or `+` is ignored. `None` when either is not such a version.
pub fn version_below(version: &str, minimum: &str) -> Option<bool> {
    fn parts(v: &str) -> Option<Vec<u64>> {
        let core = v.split(['-', '+']).next()?;
        core.split('.').map(|p| p.parse().ok()).collect()
    }
    let (a, b) = (parts(version)?, parts(minimum)?);
    let len = a.len().max(b.len());
    let at = |v: &[u64], i: usize| v.get(i).copied().unwrap_or(0);
    for i in 0..len {
        if at(&a, i) != at(&b, i) {
            return Some(at(&a, i) < at(&b, i));
        }
    }
    Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(version_below("0.3.0", "0.10.0"), Some(true));
        assert_eq!(version_below("1.0.0", "0.10.0"), Some(false));
        assert_eq!(version_below("0.3.0", "0.3.0"), Some(false));
        assert_eq!(version_below("0.3", "0.3.1"), Some(true));
        assert_eq!(version_below("0.3.0-rc.1", "0.3.0"), Some(false));
        assert_eq!(version_below("x", "0.3.0"), None);
    }

    #[test]
    fn names_fold_ascii_only() {
        assert!(same_name("SignIn", "signin"));
        assert!(!same_name("İ", "i"));
    }
}
