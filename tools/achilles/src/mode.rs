use std::time::Duration;

use hermes_types::rpc::{query, query_with};
use hermes_types::sm::{Mode, ModeRequest};
use hermes_types::{NodeName, RecvMessage};
use tokio::sync::mpsc::UnboundedSender;

pub enum ModeEvent {
    Current(Mode),
    SetResult {
        mode: Mode,
        result: Result<(), String>,
    },
}

/// Mode display + control panel state. Achilles is an allowlisted SetMode
/// source, so mode changes go through State Machine's `request_mode` RPC.
pub struct ModePanel {
    pub mode: Mode,
    pub status: Option<String>,
}

impl ModePanel {
    pub fn new() -> Self {
        Self {
            mode: Mode::Standby,
            status: None,
        }
    }
}

impl Default for ModePanel {
    fn default() -> Self {
        Self::new()
    }
}

/// Track the current mode: one-shot `get` at startup (late-joiner support)
/// then subscribe to the mode publish.
pub async fn handle_mode(session: zenoh::Session, tx: UnboundedSender<ModeEvent>) {
    if let Ok(m) = query::<Mode>(
        &session,
        &NodeName::SM.make_endpoint("get"),
        Duration::from_millis(200),
    )
    .await
    {
        tx.send(ModeEvent::Current(m.payload)).ok();
    }
    if let Ok(sub) = session
        .declare_subscriber(NodeName::SM.make_endpoint("current"))
        .await
    {
        while let Ok(sample) = sub.recv_async().await {
            if let Ok(m) = RecvMessage::<Mode>::try_from(sample) {
                tx.send(ModeEvent::Current(m.payload)).ok();
            }
        }
    }
}

/// Request a mode change through the real `request_mode` RPC.
pub fn spawn_set_mode(
    session: zenoh::Session,
    runtime: &tokio::runtime::Runtime,
    tx: UnboundedSender<ModeEvent>,
    mode: Mode,
) {
    runtime.spawn(async move {
        let req = ModeRequest {
            mode,
            source: NodeName::Achilles,
        };
        let result = match query_with::<ModeRequest, bool>(
            &session,
            &NodeName::SM.make_endpoint("request_mode"),
            req,
            Duration::from_millis(300),
        )
        .await
        {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("{:?}", e)),
        };
        tx.send(ModeEvent::SetResult { mode, result }).ok();
    });
}