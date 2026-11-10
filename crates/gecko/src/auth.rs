//! Device sign-in (RFC 8628), token refresh and revocation (RFC 7009) over HTTP
//! (Node: src/cli/auth.ts). Requests follow the CLI's proxy rule; the token never goes
//! into an address.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gecko_hub_client::net;
use gecko_hub_client::proxy::ProxyRule;
use gecko_hub_client::select::is_local_host;
use serde_json::{Map, Value};
use url::Url;

use crate::config::{Config, StoredToken, User};
use crate::interrupt::Interrupts;
use crate::output::Failure;

const CLIENT_ID: &str = "gecko-cli";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// The host of an environment: an address as given (it has a `.` or `:`, or is
/// `localhost`), or a name defined by `GECKO_ENV_<NAME>`.
pub fn resolve_env(name: &str) -> Result<String, Failure> {
    resolve_env_with(name, |key| std::env::var(key).ok())
}

pub fn resolve_env_with(
    name: &str,
    var: impl Fn(&str) -> Option<String>,
) -> Result<String, Failure> {
    if name.contains('.') || name.contains(':') || name == "localhost" {
        return Ok(name.to_owned());
    }
    let key = format!("GECKO_ENV_{}", name.to_uppercase());
    match var(&key).filter(|alias| !alias.is_empty()) {
        Some(alias) => Ok(alias),
        None => Err(Failure::new(
            "usage",
            format!(
                "unknown environment: {name}. Give its address (--env develop.example.com) or define {key}"
            ),
        )),
    }
}

/// `http://` for localhost, `https://` otherwise.
pub fn base_url(host: &str) -> Result<Url, Failure> {
    let scheme = if is_local_host(host) { "http" } else { "https" };
    Url::parse(&format!("{scheme}://{host}"))
        .map_err(|error| Failure::new("usage", format!("{host} is not a valid address: {error}")))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// The HTTP side of sign-in: one environment, one proxy rule.
#[derive(Debug, Clone)]
pub struct Auth {
    env: String,
    base: Url,
    proxy: ProxyRule,
}

/// What the user is shown while the browser approves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceStart {
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
}

struct Answer {
    status: u16,
    body: Map<String, Value>,
}

impl Auth {
    pub fn new(env: &str, proxy: ProxyRule) -> Result<Auth, Failure> {
        let host = resolve_env(env)?;
        Ok(Auth {
            env: env.to_owned(),
            base: base_url(&host)?,
            proxy,
        })
    }

    fn endpoint(&self, path: &str) -> Url {
        let mut url = self.base.clone();
        url.set_path(path);
        url
    }

    /// POSTs a form and reads a JSON object back; a body that is not one reads as `{}`.
    async fn post(&self, path: &str, form: &[(&str, &str)]) -> Result<Answer, Failure> {
        let url = self.endpoint(path);
        let unreachable =
            |detail: String| Failure::new("unreachable", format!("cannot reach {url}: {detail}"));
        let client = net::http_client(&url, &self.proxy).map_err(unreachable)?;
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form)
            .finish();
        let response = client
            .post(url.clone())
            .header("content-type", "application/x-www-form-urlencoded")
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

    fn stored(&self, body: &Map<String, Value>) -> Result<StoredToken, Failure> {
        let text = |key: &str| body.get(key).and_then(Value::as_str).map(str::to_owned);
        let malformed = || Failure::new("failed", "the server's token answer is incomplete");
        let user: User = body
            .get("user")
            .and_then(|user| serde_json::from_value(user.clone()).ok())
            .ok_or_else(malformed)?;
        let expires_in = body
            .get("expires_in")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        Ok(StoredToken {
            env: self.env.clone(),
            access_token: text("access_token").ok_or_else(malformed)?,
            access_expires_at: now_ms() + seconds(expires_in).as_millis() as u64,
            refresh_token: text("refresh_token").ok_or_else(malformed)?,
            grant_id: text("grant_id").ok_or_else(malformed)?,
            user,
        })
    }

    /// The device flow: asks for codes, lets the caller show them, then polls at the
    /// server's interval, slowing down when told (RFC 8628 §3.5). Ctrl+C ends it.
    pub async fn device_login(
        &self,
        config: &Config,
        show: impl FnOnce(&DeviceStart),
        interrupts: &mut Interrupts,
    ) -> Result<StoredToken, Failure> {
        let start = self
            .post("/oauth/device_authorization", &[("client_id", CLIENT_ID)])
            .await?;
        if start.status != 200 {
            return Err(Failure::new(
                "failed",
                format!("could not start the sign-in (HTTP {})", start.status),
            ));
        }
        let text = |key: &str| {
            start
                .body
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let device_code = text("device_code")
            .ok_or_else(|| Failure::new("failed", "the server sent no device code"))?;
        let mut interval = seconds(number(&start.body, "interval").unwrap_or(5.0));
        let lifetime = seconds(number(&start.body, "expires_in").unwrap_or(600.0));
        let verification_uri = text("verification_uri").unwrap_or_default();
        show(&DeviceStart {
            user_code: text("user_code").unwrap_or_default(),
            verification_uri_complete: text("verification_uri_complete")
                .unwrap_or_else(|| verification_uri.clone()),
            verification_uri,
        });

        let deadline = tokio::time::Instant::now() + lifetime;
        let cancelled = || Failure::new("cancelled", "interrupted");
        let form = [
            ("grant_type", DEVICE_GRANT),
            ("device_code", device_code.as_str()),
            ("client_id", CLIENT_ID),
        ];
        while tokio::time::Instant::now() < deadline {
            tokio::select! {
                _ = tokio::time::sleep(interval) => {}
                _ = interrupts.next() => return Err(cancelled()),
            }
            let poll = tokio::select! {
                poll = self.post("/oauth/token", &form) => poll?,
                _ = interrupts.next() => return Err(cancelled()),
            };
            if poll.status == 200 {
                let token = self.stored(&poll.body)?;
                config.write_token(&token).map_err(write_failure)?;
                return Ok(token);
            }
            match poll.body.get("error").and_then(Value::as_str).unwrap_or("") {
                "authorization_pending" => {}
                "slow_down" => {
                    interval = match number(&poll.body, "interval") {
                        Some(next) => seconds(next),
                        None => interval + Duration::from_secs(5),
                    };
                }
                "access_denied" => {
                    return Err(Failure::new(
                        "not-allowed",
                        "the sign-in was refused in the browser",
                    ));
                }
                "expired_token" => return Err(expired()),
                other => {
                    let detail = if other.is_empty() {
                        format!("HTTP {}", poll.status)
                    } else {
                        other.to_owned()
                    };
                    return Err(Failure::new("failed", format!("sign-in failed: {detail}")));
                }
            }
        }
        Err(expired())
    }

    /// The stored token of the environment, refreshed when it expires within 30 s (or
    /// always, with `force`).
    #[allow(dead_code, reason = "the binding commands use it in step 4b-4")]
    pub async fn current_token(
        &self,
        config: &Config,
        force: bool,
    ) -> Result<StoredToken, Failure> {
        let Some(token) = config.read_token(&self.env) else {
            return Err(Failure::new(
                "not-logged-in",
                format!("not signed in: run gecko login --env {}", self.env),
            ));
        };
        if !force && token.access_expires_at.saturating_sub(now_ms()) > 30_000 {
            return Ok(token);
        }
        let answer = self
            .post(
                "/oauth/token",
                &[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", &token.refresh_token),
                    ("client_id", CLIENT_ID),
                ],
            )
            .await?;
        if answer.status != 200 {
            return Err(Failure::new(
                "token-expired",
                format!(
                    "the sign-in expired or was revoked: run gecko login --env {}",
                    self.env
                ),
            ));
        }
        let next = self.stored(&answer.body)?;
        config.write_token(&next).map_err(write_failure)?;
        Ok(next)
    }

    /// Revokes the grant on the server; a network error is ignored, as the token file
    /// is deleted anyway and the grant can be revoked from the user's profile.
    pub async fn revoke(&self, token: &StoredToken) {
        let _ = self
            .post(
                "/oauth/revoke",
                &[
                    ("token", &token.refresh_token),
                    ("token_type_hint", "refresh_token"),
                ],
            )
            .await;
    }
}

fn expired() -> Failure {
    Failure::new("token-expired", "the sign-in code expired")
}

fn write_failure(error: std::io::Error) -> Failure {
    Failure::new("failed", format!("cannot save the token: {error}"))
}

fn number(body: &Map<String, Value>, key: &str) -> Option<f64> {
    body.get(key).and_then(Value::as_f64)
}

/// Seconds from the server, bounded: negative or absurd values do not stall the CLI.
fn seconds(value: f64) -> Duration {
    if value.is_nan() {
        return Duration::ZERO;
    }
    Duration::from_secs_f64(value.clamp(0.0, 3600.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environments() {
        let var = |key: &str| (key == "GECKO_ENV_STAGE").then(|| "stage.example".to_owned());
        assert_eq!(
            resolve_env_with("develop.example", var).unwrap(),
            "develop.example"
        );
        assert_eq!(
            resolve_env_with("127.0.0.1:4400", var).unwrap(),
            "127.0.0.1:4400"
        );
        assert_eq!(resolve_env_with("localhost", var).unwrap(), "localhost");
        assert_eq!(resolve_env_with("stage", var).unwrap(), "stage.example");
        let error = resolve_env_with("prod", var).unwrap_err();
        assert_eq!(error.code, "usage");
        assert!(
            error.message.contains("GECKO_ENV_PROD"),
            "{}",
            error.message
        );
    }

    #[test]
    fn base_urls() {
        assert_eq!(
            base_url("127.0.0.1:4400").unwrap().as_str(),
            "http://127.0.0.1:4400/"
        );
        assert_eq!(
            base_url("develop.example").unwrap().as_str(),
            "https://develop.example/"
        );
    }

    #[test]
    fn server_seconds_are_bounded() {
        assert_eq!(seconds(0.25), Duration::from_millis(250));
        assert_eq!(seconds(-3.0), Duration::ZERO);
        assert_eq!(seconds(1e12), Duration::from_secs(3600));
        assert_eq!(seconds(f64::NAN), Duration::ZERO);
    }
}
