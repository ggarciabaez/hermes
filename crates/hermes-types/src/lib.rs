mod messages;
pub mod rpc;
pub mod sm;
pub mod ctrl;

use serde::{Deserialize, Serialize};
pub use messages::{MsgMetadata, SendMessage, RecvMessage};

// TODO: add nodes as we go. These are the ones that come to mind
// we could add strings to them to work as node names
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy)]
pub enum Node {
    Gamepad,
    SM,
    Lifeguard,
    Achilles,
    Controller,

}
