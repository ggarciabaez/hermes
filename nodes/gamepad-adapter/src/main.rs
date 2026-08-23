/*
So the gamepad will expose a drive publisher, a state publisher and stream to an estop rpc as well as mode change rpcs
 */
mod utils;

use std::time::Duration;
use std::sync::Arc;
use botpad::{AsyncInputController, Axis};
use clap::Parser;
use tokio::sync::mpsc;
use hermes_types::params::Tunable;
use hermes_types::{
    RecvMessage, SendMessage, NodeName, Node, wait_for_liveliness, sm::{Mode, ModeRequest},
    gpad::GPadDrive, config::default_config
};
use hermes_macros::setup_endpoints;
use utils::*;
const NAME: NodeName = NodeName::Gamepad;

// gamepad-adapter
#[derive(clap::Parser, Debug)]
#[command(rename_all = "snake_case")]
struct Args {
    #[command(flatten)]
    global: hermes_types::GlobalArgs,
    #[arg(long)]
    wait_for_params: bool,
    #[arg(long)]
    #[clap(default_value = "so_forth")]
    so_on: String,
}

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
    let args: Args = Args::parse();
    let session = zenoh::open(default_config()).await.unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel::<Command>();

    let mut node = Node::new(&session, &NAME).await.unwrap();
    let mut params = GamepadParams::defaults();

    if args.wait_for_params && wait_for_liveliness(&session, &NodeName::Params, Duration::from_secs(5)).await {
        match node.fetch_params().await {
            Ok(params_list) => {
                for (key, value) in params_list {
                    if let Err(e) = params.set(&key, value) {
                        eprintln!("[gamepad] ignoring param {}: {}", key, e);
                    }
                }
                println!("[gamepad] loaded from Config.toml");
            }
            Err(e) => eprintln!("[gamepad] failed to fetch params: {:?}", e),
        }
    } else {
        eprintln!("[gamepad] Params node not seen within 5s; using defaults");
    }

    setup_endpoints!(
    node,
    pubs: [
        state_node = "state",
        drive_node = "drive",
    ],
    queries: [ ]);
    let mode_node = node.sub_mode().await.unwrap();

    let deadzone = *params.deadzone.get();
    let controller = setup_controller(tx, deadzone).unwrap();
    let mut mode = node.get_mode().await.unwrap_or(Mode::Standby);

    // Serve params + declare liveliness together, so the node can't do one
    // without the other. The params server keeps this node's live-tunable
    // values queryable/editable (Achilles' param view reads/writes them).
    let mut ticker = tokio::time::interval(Duration::from_millis(args.global.dt));
    let params = Arc::new(tokio::sync::RwLock::new(params));
    node.setup_with_params(params.clone()).await.unwrap();
    println!("Async Controller loop started in {}. Waiting for inputs...", mode);

    // Main Control Loop
    loop {
        tokio::select! {
            biased;
            Ok(mode_q) = mode_node.get() => {
                let curmode: RecvMessage<Mode> = mode_q.try_into().unwrap();
                mode = curmode.payload;
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
                    let p = params.read().await;


                    let v = apply_deadzone((
                        controller.axis(Axis::RightZ)-controller.axis(Axis::LeftZ))/2.0,
                        *p.deadzone.get()
                    ).clamp(*p.v_min.get() as f32, *p.v_max.get() as f32);
                    drop(p);
                    let y = apply_deadzone(controller.axis(Axis::LeftStickX), deadzone).clamp(-1.0, 1.0);
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
