//! The hub's HTTP side: an attach code's secret turned into tokens (`POST /cli/attach`,
//! attach spec §10.2) and the token's refresh. Requests follow the CLI's proxy rule;
//! the token never goes into an address.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gecko_hub_client::net;
use gecko_hub_client::proxy::ProxyRule;
use gecko_kernel::{BIND_AGAIN, Failure};
use serde_json::{Map, Value};
use url::Url;

use crate::code;
use crate::config::{Config, StoredToken, User};

const CLIENT_ID: &str = "gecko-cli";

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// One hub's HTTP routes.
#[derive(Debug, Clone)]
pub struct Auth {
    hub: String,
    base: Url,
    proxy: ProxyRule,
}

struct Answer {
    status: u16,
    body: Map<String, Value>,
}

impl Auth {
    /// `hub` is an origin that passed `code::hub_origin`.
    pub fn new(hub: &str, proxy: ProxyRule) -> Result<Auth, Failure> {
        let base = Url::parse(hub)
            .map_err(|error| Failure::new("failed", format!("{hub} is not an address: {error}")))?;
        Ok(Auth {
            hub: hub.to_owned(),
            base,
            proxy,
        })
    }

    fn endpoint(&self, path: &str) -> Url {
        let mut url = self.base.clone();
        url.set_path(path);
        url
    }

    /// POSTs a body and reads a JSON object back; a body that is not one reads as `{}`.
    async fn post(&self, path: &str, content_type: &str, body: String) -> Result<Answer, Failure> {
        let url = self.endpoint(path);
        let unreachable =
            |detail: String| Failure::new("unreachable", format!("cannot reach {url}: {detail}"));
        let client = net::http_client(&url, &self.proxy).map_err(unreachable)?;
        let response = client
            .post(url.clone())
            .header("content-type", content_type)
            .body(body)
            .send()
            .await
            .map_err(|error| unreachable(error.to_string()))?;
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        let body = match serde_json::from_str(&text) {
            Ok(Value::Object(map)) => map,
            _ => Map::new(),
        };
        Ok(Answer { status, body })
    }

    async fn post_form(&self, path: &str, form: &[(&str, &str)]) -> Result<Answer, Failure> {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form)
            .finish();
        self.post(path, "application/x-www-form-urlencoded", body)
            .await
    }

    /// Spends the code's secret: the hub binds the tab that made it and answers with
    /// the binding's tokens.
    pub async fn attach(&self, secret: &str) -> Result<StoredToken, Failure> {
        let body = serde_json::json!({ "secret": secret }).to_string();
        let answer = self.post("/cli/attach", "application/json", body).await?;
        if answer.status != 200 {
            let code = answer
                .body
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("");
            return Err(match code {
                "invalid-code" => code::invalid(),
                "no-terminal-access" => {
                    Failure::new("no-terminal-access", "this account has no terminal access")
                }
                "too-many-attempts" => Failure::new(
                    "too-many-attempts",
                    "too many wrong codes from this address; wait a minute, then copy a new code from the tab",
                ),
                "" => Failure::new(
                    "failed",
                    format!("the hub refused the code (HTTP {})", answer.status),
                ),
                other => Failure::new("failed", format!("the hub refused the code: {other}")),
            });
        }
        let text = |key: &str| {
            answer
                .body
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let malformed = || Failure::new("failed", "the hub's attach answer is incomplete");
        let user: User = answer
            .body
            .get("user")
            .and_then(|user| serde_json::from_value(user.clone()).ok())
            .ok_or_else(malformed)?;
        Ok(StoredToken {
            hub: self.hub.clone(),
            access_token: text("access_token").ok_or_else(malformed)?,
            access_expires_at: expires_at(&answer.body),
            refresh_token: text("refresh_token").ok_or_else(malformed)?,
            grant_id: text("grantId").ok_or_else(malformed)?,
            binding_id: text("bindingId").ok_or_else(malformed)?,
            level: text("level").ok_or_else(malformed)?,
            user,
        })
    }

    /// The binding's token, refreshed when it expires within 30 s (or always, with
    /// `force`). A refused refresh means the binding ended: the token is deleted.
    pub async fn current_token(
        &self,
        config: &Config,
        token: StoredToken,
        force: bool,
    ) -> Result<StoredToken, Failure> {
        if !force && token.access_expires_at.saturating_sub(now_ms()) > 30_000 {
            return Ok(token);
        }
        let answer = self
            .post_form(
                "/oauth/token",
                &[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", &token.refresh_token),
                    ("client_id", CLIENT_ID),
                ],
            )
            .await?;
        if (400..500).contains(&answer.status) {
            // `invalid_grant`: the grant ended with its binding (attach spec §6).
            let _ = config.delete_token();
            return Err(Failure::new(
                "gone",
                format!("the binding ended; {BIND_AGAIN}"),
            ));
        }
        if answer.status != 200 {
            return Err(Failure::new(
                "unreachable",
                format!("the hub could not renew the token (HTTP {})", answer.status),
            ));
        }
        let access_token = answer
            .body
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(|| Failure::new("failed", "the hub's token answer is incomplete"))?;
        let next = StoredToken {
            access_token: access_token.to_owned(),
            access_expires_at: expires_at(&answer.body),
            refresh_token: answer
                .body
                .get("refresh_token")
                .and_then(Value::as_str)
                .map_or(token.refresh_token.clone(), str::to_owned),
            ..token
        };
        config
            .write_token(&next)
            .map_err(|error| Failure::new("failed", format!("cannot save the token: {error}")))?;
        Ok(next)
    }
}

fn expires_at(body: &Map<String, Value>) -> u64 {
    let seconds = body
        .get("expires_in")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    now_ms() + u64::try_from(bounded(seconds).as_millis()).unwrap_or(0)
}

/// Seconds from the server, bounded: negative or absurd values do not stall the CLI.
fn bounded(value: f64) -> Duration {
    if value.is_nan() {
        return Duration::ZERO;
    }
    Duration::from_secs_f64(value.clamp(0.0, 24.0 * 3600.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_seconds_are_bounded() {
        assert_eq!(bounded(0.25), Duration::from_millis(250));
        assert_eq!(bounded(-3.0), Duration::ZERO);
        assert_eq!(bounded(1e12), Duration::from_secs(24 * 3600));
        assert_eq!(bounded(f64::NAN), Duration::ZERO);
    }

    #[test]
    fn endpoints_keep_the_origin() {
        let auth = Auth::new("https://hub.example:8443", ProxyRule::default()).unwrap();
        assert_eq!(
            auth.endpoint("/cli/attach").as_str(),
            "https://hub.example:8443/cli/attach"
        );
    }
}
