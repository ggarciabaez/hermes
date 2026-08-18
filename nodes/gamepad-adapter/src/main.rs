/*
So the gamepad will expose a drive publisher, a state publisher and stream to an estop rpc as well as mode change rpcs
 */
mod utils;

use std::time::Duration;
use botpad::{AsyncInputController, Axis};
use tokio::{time, sync::mpsc};
use hermes_types::params::Tunable;
use hermes_types::{
    RecvMessage, SendMessage, NodeName, Node, wait_for_liveliness, sm::{Mode, ModeRequest},
    gpad::GPadDrive,
};
use hermes_macros::setup_endpoints;
use utils::*;
const NAME: NodeName = NodeName::Gamepad;


async fn request_mode_change(command: Command, session: zenoh::Session) -> Result<(), String> {
    let mode = match command{
        Command::Manual => Mode::Manual,
        Command::Auto => Mode::Auto,
        Command::Halt => Mode::Halt,
        Command::EStop => Mode::EStop,
        Command::Standby => Mode::Standby,
        _ => panic!("Unexpected mode command")
    };
    let q = session.get(NodeName::SM.make_endpoint("request_mode"))
        .payload(SendMessage::new(ModeRequest{mode, source: NodeName::Gamepad}))
        .await.map_err(|_e| "Failure sending mode change.")?;
    if let Ok(reply) = q.recv_async().await {  // TODO: could be slow but safe
        match reply.result() {
            Ok(_sample) => {},
            Err(e) => {println!("Error changing mode: {:?}", e)}
        }
    }
    Ok(())
}

async fn handle_command_response(command: Option<Command>, session: zenoh::Session, ctrl: &AsyncInputController<Command>) -> Result<(), String>{
    if command.is_none() {
        panic!("MPSC Channel was closed! Something is seriously wrong!")
    }
     match command.unwrap() {
        Command::Other => {println!("hi! im other"); ctrl.rumble(0.5, 0.5, 1000).map_err(|_e| "Failed to rumble.")?;},
        default => request_mode_change(default, session).await?
    };
    Ok(())
}

#[tokio::main]
async fn main() {
    let session = zenoh::open(zenoh::Config::default()).await.unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel::<Command>();

    // TODO: these two are quite easy names to confuse
    let mut node = Node::new(&session, &NAME).await.unwrap();
    let mut params = GamepadParams::defaults();

    // Boot load: wait for Params, pull this node's [params.gamepad] slice and
    // apply it through the same Tunable::set a live tweak uses. Defaults on
    // any failure so the node still boots without Params around.
    // Any node that needs to wait for another's liveliness is a secondary node.
    if wait_for_liveliness(&session, &NodeName::Params, Duration::from_secs(5)).await {
        match node.fetch_params().await {
            Ok(params_list) => {
                for (key, value) in params_list {
                    if let Err(e) = params.set(&key, value) {
                        eprintln!("[params] ignoring {}: {}", key, e);
                    }
                }
                println!("[params] loaded from Config.toml");
            }
            Err(e) => eprintln!("[params] failed to fetch params: {:?}", e),
        }
    } else {
        eprintln!("[params] Params node not seen within 5s; using defaults");
    }

    setup_endpoints!(
    node,
    pubs: [
        state_node = "state",
        drive_node = "drive",
    ],
    queries: [ ]);
    let mode_node = node.sub_mode().await.unwrap();

    let controller = setup_controller(tx, *params.deadzone.get()).unwrap();
    let mut ticker = time::interval(Duration::from_millis(20));
    let mut mode = node.get_mode().await.unwrap_or(Mode::Standby);
    node.activate_liveliness().await.unwrap();
    println!("Async Controller loop started in {}. Waiting for inputs...", mode);

    // Main Control Loop
    loop {
        tokio::select! {
            biased;
            Ok(mode_q) = mode_node.get() => {
                let curmode: RecvMessage<Mode> = mode_q.try_into().unwrap();
                mode = curmode.payload;
                println!("Received mode: {:?}", mode);
                if mode == Mode::Halt {
                    println!("Got {:?} mode. Stopping...", mode.to_string());
                    break
                }
            },
            command = rx.recv() => {
                handle_command_response(command, session.clone(), &controller)
                .await
                .map_err(|e| eprintln!("{:?}", e)).expect("");
            },
            _ = ticker.tick() => {
                state_node.put(&controller.get_state())
                    .await
                    .map_err(|e| eprintln!("{:?}", e))
                    .ok();
                if mode == Mode::Manual {
                    let deadzone = *params.deadzone.get();
                    let v_min = *params.v_min.get();
                    let v_max = *params.v_max.get();
                    let v = clamp_drive(apply_deadzone(controller.axis(Axis::RightZ), deadzone), v_min, v_max);
                    let y = clamp_drive(apply_deadzone(controller.axis(Axis::LeftStickX), deadzone), v_min, v_max);
                    drive_node.put(&GPadDrive{v, y})
                        .await
                        .map_err(|e| eprintln!("{:?}", e))
                        .ok();
                }
            }
        }
    }
    session.close().await.unwrap();
}
