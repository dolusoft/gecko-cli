//! A binding's start and end, as a user makes them.

use crate::{Cli, Hub, Output, Tab};

/// The tab makes an attach code at `level` and the CLI binds with it:
/// `gecko attach - --trust-hub` with the code on stdin. Returns once the tab has its
/// binding and has published its command list and place, so the next line finds them.
pub async fn connect(hub: &Hub, cli: &Cli, tab: &mut Tab, level: &str) -> Output {
    let code = tab.attach_code(level).await;
    let output = cli
        .call(["attach", "-", "--trust-hub"])
        .stdin(&format!("{code}\n"))
        .output()
        .await;
    output.expect_code(0);
    let binding = tab.bound().await;
    let token = cli
        .token()
        .unwrap_or_else(|| panic!("no token.json after attach\n{output}"));
    assert_eq!(token["hub"], hub.url.as_str(), "{token}");
    assert_eq!(token["bindingId"], binding["bindingId"], "{token}");
    assert_eq!(token["level"], level, "{token}");
    output
}

/// `gecko detach`: afterwards there is no token file, the old refresh token is
/// `invalid_grant`, and the tab hears its binding end.
pub async fn disconnect(hub: &Hub, cli: &Cli, tab: &mut Tab) -> Output {
    let token = cli.token().expect("a binding to end: token.json");
    let output = cli.run(["detach"]).await;
    output.expect_code(0);
    assert!(cli.token().is_none(), "token.json after detach\n{output}");
    let refresh = token["refreshToken"].as_str().expect("a refresh token");
    let (status, answer) = hub.refresh(refresh).await;
    assert_eq!(
        (status, answer["error"].as_str()),
        (400, Some("invalid_grant")),
        "{answer}"
    );
    let ended = tab.saw_binding("gone").await;
    assert_eq!(ended["bindingId"], token["bindingId"], "{ended}");
    output
}
