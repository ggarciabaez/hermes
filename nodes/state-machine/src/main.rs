use hermes_types::{SendMessage, rpc::Request, Node};
use hermes_types::sm::*;

#[tokio::main]
async fn main() {
    let session = zenoh::open(zenoh::Config::default()).await.unwrap();

    let chmode = session.declare_queryable("robot/mode/request_mode").await.unwrap();
    let curmode = session.declare_publisher("robot/mode/current").await.unwrap();
    let qmode = session.declare_queryable("robot/mode/get").await.unwrap();

    let mut mode = Mode::Standby;

    loop {
        tokio::select! {
            Ok(mode_query) = chmode.recv_async() => {
                let request: Request<ModeRequest> = (&mode_query).try_into().unwrap();
                
                if !matches!(request.source, Node::Gamepad | Node::Achilles | Node::Lifeguard) {
                    mode_query.reply_err("Access Denied").await.unwrap();
                    continue;
                }
                if !transition(mode, request.mode){
                    mode_query.reply_err("Invalid transition").await.unwrap();
                    continue;
                }
                if mode == Mode::SWStop && (request.source == Node::Lifeguard){
                    mode_query.reply_err("Access Denied").await.unwrap();
                    continue;
                }
                mode = request.mode;
                curmode.put(SendMessage::new(&mode)).await.unwrap();
                mode_query.reply("robot/mode/request_mode", SendMessage::new(true)).await.unwrap();
                if mode == Mode::Halt {
                    break;
                }
            },
            Ok(mode_query) = qmode.recv_async() => {
                mode_query.reply("robot/mode/get", SendMessage::new(&mode)).await.unwrap();
            }
        }
    }
    session.close().await.unwrap();
}