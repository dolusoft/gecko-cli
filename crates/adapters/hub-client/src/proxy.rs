//! Which proxy a connection goes through (spec §7.3, F30).
//!
//! One rule for every connection the CLI opens, HTTP and WebSocket alike, read from the
//! usual variables: `https_proxy`/`HTTPS_PROXY` for `https`/`wss` addresses,
//! `http_proxy`/`HTTP_PROXY` for `http`/`ws`, `all_proxy`/`ALL_PROXY` for both, and
//! `no_proxy`/`NO_PROXY` for the exceptions. The lowercase name wins when both are set.
//! Only `http://` proxies are supported; the tunnel is an HTTP `CONNECT`.

use url::{Host, Url};

/// The proxy settings, read once per call of the CLI.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProxyRule {
    https: Option<String>,
    http: Option<String>,
    all: Option<String>,
    no_proxy: Vec<String>,
}

impl ProxyRule {
    /// No proxy for any address.
    pub fn none() -> ProxyRule {
        ProxyRule::default()
    }

    /// The rule from the process environment.
    pub fn from_env() -> ProxyRule {
        ProxyRule::from_lookup(|name| std::env::var(name).ok())
    }

    /// The rule from a variable lookup; tests pass their own.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> ProxyRule {
        let read = |lower: &str, upper: &str| {
            [lower, upper]
                .into_iter()
                .filter_map(&get)
                .map(|value| value.trim().to_owned())
                .find(|value| !value.is_empty())
        };
        ProxyRule {
            https: read("https_proxy", "HTTPS_PROXY"),
            http: read("http_proxy", "HTTP_PROXY"),
            all: read("all_proxy", "ALL_PROXY"),
            no_proxy: read("no_proxy", "NO_PROXY")
                .map(|list| {
                    list.split(',')
                        .map(|entry| entry.trim().to_ascii_lowercase())
                        .filter(|entry| !entry.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// The proxy for `target`, or `None` to connect directly. An unusable proxy setting is
    /// an error rather than a silent direct connection.
    pub fn proxy_for(&self, target: &Url) -> Result<Option<Url>, String> {
        let chosen = match target.scheme() {
            "https" | "wss" => self.https.as_ref().or(self.all.as_ref()),
            _ => self.http.as_ref().or(self.all.as_ref()),
        };
        let Some(chosen) = chosen else {
            return Ok(None);
        };
        if self.bypasses(target) {
            return Ok(None);
        }
        let with_scheme = if chosen.contains("://") {
            chosen.clone()
        } else {
            format!("http://{chosen}")
        };
        let proxy = Url::parse(&with_scheme)
            .map_err(|error| format!("the proxy address {chosen} is not valid: {error}"))?;
        if proxy.scheme() != "http" {
            return Err(format!(
                "the proxy address {chosen} is not supported; use an http:// proxy"
            ));
        }
        if proxy.host().is_none() {
            return Err(format!("the proxy address {chosen} has no host"));
        }
        Ok(Some(proxy))
    }

    fn bypasses(&self, target: &Url) -> bool {
        let Some(host) = target.host() else {
            return true;
        };
        let host = match host {
            Host::Domain(name) => name.trim_end_matches('.').to_ascii_lowercase(),
            Host::Ipv4(address) => address.to_string(),
            Host::Ipv6(address) => address.to_string(),
        };
        let port = target.port_or_known_default();
        self.no_proxy
            .iter()
            .any(|entry| no_proxy_matches(entry, &host, port))
    }
}

/// One `NO_PROXY` entry: `*`, a name (also matching its subdomains, with or without a
/// leading `.` or `*.`), an IP address, and either with an optional `:port`.
/// CIDR ranges are not supported.
fn no_proxy_matches(entry: &str, host: &str, port: Option<u16>) -> bool {
    if entry == "*" {
        return true;
    }
    let (name, entry_port) = split_port(entry);
    if entry_port.is_some() && entry_port != port {
        return false;
    }
    let name = name
        .trim_start_matches("*.")
        .trim_start_matches('.')
        .trim_start_matches('[')
        .trim_end_matches(']');
    if name.is_empty() {
        return false;
    }
    host == name
        || host
            .strip_suffix(name)
            .is_some_and(|rest| rest.ends_with('.'))
}

/// `host:port`, `[v6]:port` or a bare host. A bare IPv6 address has several colons and
/// no port.
fn split_port(entry: &str) -> (&str, Option<u16>) {
    if let Some(rest) = entry.strip_prefix('[') {
        if let Some((address, after)) = rest.split_once(']') {
            return (
                address,
                after.strip_prefix(':').and_then(|p| p.parse().ok()),
            );
        }
        return (entry, None);
    }
    match entry.rsplit_once(':') {
        Some((name, port)) if !name.contains(':') => match port.parse() {
            Ok(port) => (name, Some(port)),
            Err(_) => (entry, None),
        },
        _ => (entry, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(pairs: &[(&str, &str)]) -> ProxyRule {
        ProxyRule::from_lookup(|name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        })
    }

    fn proxy(rule: &ProxyRule, target: &str) -> Option<String> {
        rule.proxy_for(&Url::parse(target).unwrap())
            .unwrap()
            .map(|url| url.to_string())
    }

    #[test]
    fn scheme_picks_the_variable() {
        let rule = rule(&[
            ("HTTPS_PROXY", "http://secure:3128"),
            ("http_proxy", "plain:8080"),
        ]);
        assert_eq!(
            proxy(&rule, "https://hub.example/cli").as_deref(),
            Some("http://secure:3128/")
        );
        assert_eq!(
            proxy(&rule, "wss://hub.example/cli").as_deref(),
            Some("http://secure:3128/")
        );
        assert_eq!(
            proxy(&rule, "http://hub.example/cli").as_deref(),
            Some("http://plain:8080/")
        );
    }

    #[test]
    fn lowercase_wins_and_empty_means_unset() {
        let rule = rule(&[
            ("https_proxy", "http://lower:1"),
            ("HTTPS_PROXY", "http://upper:2"),
        ]);
        assert_eq!(
            proxy(&rule, "https://a.example").as_deref(),
            Some("http://lower:1/")
        );
        let rule = self::rule(&[("https_proxy", " "), ("HTTPS_PROXY", "http://upper:2")]);
        assert_eq!(
            proxy(&rule, "https://a.example").as_deref(),
            Some("http://upper:2/")
        );
    }

    #[test]
    fn all_proxy_is_the_fallback() {
        let rule = rule(&[("ALL_PROXY", "http://every:1")]);
        assert_eq!(
            proxy(&rule, "https://a.example").as_deref(),
            Some("http://every:1/")
        );
        assert_eq!(
            proxy(&rule, "http://a.example").as_deref(),
            Some("http://every:1/")
        );
        assert_eq!(proxy(&ProxyRule::none(), "https://a.example"), None);
    }

    #[test]
    fn no_proxy_entries() {
        let rule = rule(&[
            ("HTTPS_PROXY", "http://p:1"),
            (
                "NO_PROXY",
                "internal.example, .corp.example,*.lab.example,10.0.0.5,[::1],only-port.example:8443",
            ),
        ]);
        for direct in [
            "https://internal.example/",
            "https://api.internal.example/",
            "https://corp.example/",
            "https://x.corp.example/",
            "https://lab.example/",
            "https://10.0.0.5/",
            "https://[::1]:5000/",
            "https://only-port.example:8443/",
            "https://INTERNAL.example./",
        ] {
            assert_eq!(proxy(&rule, direct), None, "{direct}");
        }
        for proxied in [
            "https://notinternal.example/",
            "https://10.0.0.50/",
            "https://only-port.example/",
            "https://example/",
        ] {
            assert!(proxy(&rule, proxied).is_some(), "{proxied}");
        }
        let everything = self::rule(&[("HTTPS_PROXY", "http://p:1"), ("no_proxy", "*")]);
        assert_eq!(proxy(&everything, "https://a.example"), None);
    }

    #[test]
    fn unusable_proxy_is_an_error() {
        let target = Url::parse("https://a.example").unwrap();
        let secure = rule(&[("HTTPS_PROXY", "https://p:1")]);
        assert!(secure.proxy_for(&target).unwrap_err().contains("http://"));
        let socks = rule(&[("HTTPS_PROXY", "socks5://p:1")]);
        assert!(socks.proxy_for(&target).is_err());
        let broken = rule(&[("HTTPS_PROXY", "http://[nope")]);
        assert!(broken.proxy_for(&target).unwrap_err().contains("not valid"));
    }
}
