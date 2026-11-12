//! The two transports behind one type: the binary opens a `Hub`, and the CLI's rules see
//! only `HubConnection` (generic, not `dyn`).

use serde_json::value::RawValue;

use crate::dev::DevTransport;
use crate::signalr::Transport as SignalrTransport;
use crate::{CallError, Closed, HubEvent};
use gecko_hub_protocol::HubConnection;

/// One open connection: to the development hub or to the hosted hub (SignalR).
#[derive(Debug)]
pub enum Hub {
    Dev(DevTransport),
    Signalr(SignalrTransport),
}

impl HubConnection for Hub {
    async fn status(&mut self) -> Result<Box<RawValue>, CallError> {
        match self {
            Hub::Dev(hub) => hub.status().await,
            Hub::Signalr(hub) => hub.status().await,
        }
    }

    async fn send(&mut self, envelope: &RawValue) -> Result<Box<RawValue>, CallError> {
        match self {
            Hub::Dev(hub) => hub.send(envelope).await,
            Hub::Signalr(hub) => hub.send(envelope).await,
        }
    }

    async fn detach(&mut self) -> Result<(), CallError> {
        match self {
            Hub::Dev(hub) => hub.detach().await,
            Hub::Signalr(hub) => hub.detach().await,
        }
    }

    async fn next_event(&mut self) -> Result<HubEvent, Closed> {
        match self {
            Hub::Dev(hub) => hub.next_event().await,
            Hub::Signalr(hub) => hub.next_event().await,
        }
    }

    async fn close(self) -> Closed {
        match self {
            Hub::Dev(hub) => hub.close().await,
            Hub::Signalr(hub) => hub.close().await,
        }
    }
}
