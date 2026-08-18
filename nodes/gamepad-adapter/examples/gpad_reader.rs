use hermes_types::gpad::ControllerState;
use hermes_types::{RecvMessage, SendMessage, rpc::Request, NodeName};
use hermes_types::sm::*;
use tokio::time;

#[tokio::main]
async fn main() {
    let session = zenoh::open(zenoh::Config::default()).await.unwrap();

    let chmode = session.declare_queryable("robot/mode/request_mode").await.unwrap();
    let curmode = session.declare_publisher("robot/mode/current").await.unwrap();
    let state_node = session.declare_subscriber("robot/gamepad/state").await.unwrap();

    let mut ticker = time::interval(time::Duration::from_millis(50));

    let mut mode = Mode::Standby;

    loop {
        tokio::select! {
            Ok(mode_query) = chmode.recv_async() => {
                let request: Request<ModeRequest> = (&mode_query).try_into().unwrap();
                let mut approved = false;
                if request.source == NodeName::Gamepad {
                    mode = request.mode.clone();
                    approved = true;
                }
                mode_query.reply("robot/mode/request_mode", SendMessage::new(approved)).await.unwrap();
                if mode == Mode::Halt {
                    curmode.put(SendMessage::new(&mode)).await.unwrap();
                    break;
                }
            },
            state_msg = state_node.recv_async() => {
                let Ok(state_msg) = state_msg else {
                    eprintln!("State node disconnected");
                    break;
                };
                let ctrl_state: RecvMessage<ControllerState> = state_msg.try_into().unwrap();
                println!("Sticks: [{}, {}, {}, {}]", ctrl_state.left_x, ctrl_state.left_y, ctrl_state.right_x, ctrl_state.right_y);
            },
            _ = ticker.tick() => {
                curmode.put(SendMessage::new(&mode)).await.unwrap();
            }
        }
    }
    session.close().await.unwrap();
}