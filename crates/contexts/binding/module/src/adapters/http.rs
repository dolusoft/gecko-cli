//! The hub's HTTP side: an attach code's secret turned into tokens (`POST /cli/attach`,
//! attach spec §10.2) and the token's refresh. Requests follow the CLI's proxy rule;
//! the token never goes into an address. When to refresh and what a refusal means are
//! the domain's (`gecko_binding_domain::token`).

use gecko_binding_contract::{StoredToken, TokenStore, User};
use gecko_binding_domain::token::{
    attach_refused, binding_ended, expires_at, needs_refresh, refresh_refused,
};
use gecko_hub_client::net;
use gecko_hub_client::proxy::ProxyRule;
use gecko_kernel::{Clock, Failure};
use serde_json::{Map, Value};
use url::Url;

const CLIENT_ID: &str = "gecko-cli";

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
    pub async fn attach(&self, secret: &str, clock: &impl Clock) -> Result<StoredToken, Failure> {
        let body = serde_json::json!({ "secret": secret }).to_string();
        let answer = self.post("/cli/attach", "application/json", body).await?;
        if answer.status != 200 {
            let code = answer
                .body
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("");
            return Err(attach_refused(answer.status, code));
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
            access_expires_at: expires_at(&answer.body, clock.now_ms()),
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
        store: &impl TokenStore,
        token: StoredToken,
        force: bool,
        clock: &impl Clock,
    ) -> Result<StoredToken, Failure> {
        if !needs_refresh(token.access_expires_at, clock.now_ms(), force) {
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
        if refresh_refused(answer.status) {
            let _ = store.delete_token();
            return Err(binding_ended());
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
            access_expires_at: expires_at(&answer.body, clock.now_ms()),
            refresh_token: answer
                .body
                .get("refresh_token")
                .and_then(Value::as_str)
                .map_or(token.refresh_token.clone(), str::to_owned),
            ..token
        };
        store
            .write_token(&next)
            .map_err(|error| Failure::new("failed", format!("cannot save the token: {error}")))?;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_keep_the_origin() {
        let auth = Auth::new("https://hub.example:8443", ProxyRule::default()).unwrap();
        assert_eq!(
            auth.endpoint("/cli/attach").as_str(),
            "https://hub.example:8443/cli/attach"
        );
    }
}
