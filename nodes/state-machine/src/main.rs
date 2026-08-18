use hermes_types::{SendMessage, rpc::Request, NodeName, Node};
use hermes_types::sm::*;
use hermes_macros::setup_endpoints;

const NAME: NodeName = NodeName::SM;

#[tokio::main]
async fn main() {
    let mut mode = Mode::Standby;
    let session = zenoh::open(zenoh::Config::default()).await.unwrap();
    let mut node = Node::new(&session, &NAME).await.unwrap();
    setup_endpoints!(node,
        pubs: [curmode = "current"],
        queries: [chmode = "request_mode", qmode = "get"]
    );

    curmode.put(&mode).await.unwrap();
    node.activate_liveliness().await.unwrap();

    loop {
        tokio::select! {
            Ok(mode_query) = chmode.get() => {
                let request: Request<ModeRequest> = (&mode_query).try_into().unwrap();
                
                if !matches!(request.source, NodeName::Gamepad | NodeName::Achilles | NodeName::Lifeguard) {
                    mode_query.reply_err("Access Denied").await.unwrap();
                    continue;
                }
                if !transition(mode, request.mode){
                    mode_query.reply_err("Invalid transition").await.unwrap();
                    continue;
                }
                if mode == Mode::SWStop && (request.source == NodeName::Lifeguard){
                    mode_query.reply_err("Access Denied").await.unwrap();
                    continue;
                }
                mode = request.mode;
                curmode.put(&mode).await.unwrap();
                mode_query.reply(mode_query.key_expr(), SendMessage::new(true)).await.unwrap();
                if mode == Mode::Halt {
                    break;
                }
            },
            Ok(mode_query) = qmode.get() => {
                mode_query.reply(mode_query.key_expr(), SendMessage::new(&mode)).await.unwrap();
            }
        }
    }
    session.close().await.unwrap();
}