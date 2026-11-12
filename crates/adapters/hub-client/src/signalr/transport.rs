//! The hosted hub's CLI methods over one SignalR connection (#1974): `Status`, `Send`
//! and `Detach`, each answering `{ ok, value | error }`; the events `Reply` and
//! `Binding`.

use serde_json::value::RawValue;

use super::connection::{Connection, Event, InvokeError, Options};
use super::protocol::hub_result;
use crate::{CallError, Closed, HubError, HubEvent};
use gecko_hub_protocol::HubConnection;

/// A SignalR connection to the hub's CLI endpoint, as the CLI's port sees it.
#[derive(Debug)]
pub struct Transport {
    connection: Connection,
}

impl Transport {
    pub async fn connect(options: &Options) -> Result<Transport, Closed> {
        Ok(Transport {
            connection: Connection::connect(options).await?,
        })
    }

    async fn call(
        &mut self,
        target: &str,
        arguments: &[&RawValue],
    ) -> Result<Box<RawValue>, CallError> {
        match self.connection.invoke(target, arguments).await {
            Ok(result) => hub_result(&result).map_err(CallError::Hub),
            Err(InvokeError::Closed(closed)) => Err(CallError::Closed(closed)),
            Err(InvokeError::TimedOut) => Err(CallError::TimedOut),
            // A hub method that threw: the hub answered, so the connection stays; the
            // text is all SignalR carries.
            Err(InvokeError::Failed(text)) => Err(CallError::Hub(HubError {
                code: "failed".to_owned(),
                message: Some(format!("the hub failed: {text}")),
                reason: None,
                retry_after_ms: None,
            })),
        }
    }
}

/// `Reply` and `Binding` become the CLI's `reply` and `binding`; the payload is the
/// first argument.
fn hub_event(event: Event) -> HubEvent {
    let payload = event
        .arguments
        .into_iter()
        .next()
        .unwrap_or_else(|| RawValue::from_string("null".to_owned()).expect("null"));
    HubEvent::from_parts(&event.target.to_ascii_lowercase(), payload)
}

impl HubConnection for Transport {
    async fn status(&mut self) -> Result<Box<RawValue>, CallError> {
        self.call("Status", &[]).await
    }

    async fn send(&mut self, envelope: &RawValue) -> Result<Box<RawValue>, CallError> {
        self.call("Send", &[envelope]).await
    }

    async fn detach(&mut self) -> Result<(), CallError> {
        self.call("Detach", &[]).await.map(|_| ())
    }

    async fn next_event(&mut self) -> Result<HubEvent, Closed> {
        self.connection.next_event().await.map(hub_event)
    }

    async fn close(self) -> Closed {
        self.connection.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(json: &str) -> Box<RawValue> {
        RawValue::from_string(json.to_owned()).unwrap()
    }

    #[test]
    fn events_take_the_cli_names_and_the_first_argument() {
        let reply = hub_event(Event {
            target: "Reply".to_owned(),
            arguments: vec![raw("{\"v\":1}"), raw("2")],
        });
        assert!(matches!(reply, HubEvent::Reply(payload) if payload.get() == "{\"v\":1}"));
        let binding = hub_event(Event {
            target: "Binding".to_owned(),
            arguments: vec![],
        });
        assert!(matches!(binding, HubEvent::Binding(payload) if payload.get() == "null"));
        let other = hub_event(Event {
            target: "CliBusy".to_owned(),
            arguments: vec![raw("{}")],
        });
        assert!(matches!(other, HubEvent::Other { name, .. } if name == "clibusy"));
    }
}
