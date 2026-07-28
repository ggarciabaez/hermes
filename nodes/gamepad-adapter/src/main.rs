/*
So the gamepad will expose a drive publisher, a state publisher and stream to an estop rpc as well as mode change rpcs
 */
use std::time::Duration;
use botpad::{InputController, AsyncInputController, Button, Gilrs};
use tokio::{time, sync::mpsc};
use hermes_types::{RecvMessage, SendMessage, Node, sm::{Mode, ModeRequest}, rpc::query};

enum Command{
    Standby,
    Manual,
    Auto,
    Halt,
    EStop,
    Other
}

fn setup_controller(tx: mpsc::UnboundedSender<Command>) -> Result<AsyncInputController<Command>, String> {
    let gilrs = Gilrs::new().expect("Failed to initialize gilrs");
    let gpid = InputController::first_connected(&gilrs);

    let mut sync_controller = InputController::new(tx, gpid, Some(gilrs))?;

    sync_controller.deadzone = 0.1;


    // One will be pressed first. Pressing the second one makes it actually trigger.
    // Thus guaranteeing it will be called once. Rarely it will call twice, but it's safely handled.
    sync_controller.on_press(Button::LeftTrigger, |gamepad| {
        if gamepad.is_pressed(Button::DPadLeft){
            return Some(Command::EStop)
        }
        None
    })?;
    sync_controller.on_press(Button::DPadLeft, |gamepad| {
        if gamepad.is_pressed(Button::LeftTrigger){
            return Some(Command::EStop)
        }
        None
    })?;

    // Other modes
    sync_controller.on_press(Button::Start, |_gamepad| {
        Some(Command::Manual)
    })?;
    sync_controller.on_press(Button::Select, |_gamepad| {
        Some(Command::Auto)
    })?;
    sync_controller.on_press(Button::Mode, |_gamepad| {
        Some(Command::Halt)
    })?;
    sync_controller.on_press(Button::RightThumb, |_gamepad| {
        Some(Command::Standby)
    })?;

    // to please clippy and also cause rumble
    sync_controller.on_press(Button::South, |_gamepad| {
        Some(Command::Other)
    })?;

    // 3. Wrap it in the Async controller (spawns a 16ms background polling task)
    Ok(AsyncInputController::new(sync_controller, 16))
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
    let q = session.get("robot/mode/request_mode")
        .payload(SendMessage::new(ModeRequest{mode, source: Node::Gamepad}))
        .await.map_err(|_e| "Failure sending mode change.")?;
    if let Ok(reply) = q.recv_async().await {  // TODO: could be slow but safe
        match reply.result() {
            Ok(_sample) => {println!("Current mode: {}", mode)},
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

    let controller = setup_controller(tx).unwrap();
    let mut ticker = time::interval(Duration::from_millis(20));
    let mut mode = query::<Mode>(&session, "robot/mode/get", Duration::from_millis(50))
        .await.unwrap_or(RecvMessage::new(Mode::Standby)).payload;



    let mode_node = session.declare_subscriber("robot/mode/current").await.unwrap();
    let state_node = session.declare_publisher("robot/gamepad/state").await.unwrap();

    println!("Async Controller loop started in {}. Waiting for inputs...", mode);

    // Main Control Loop
    loop {
        tokio::select! {
            command = rx.recv() => {
                handle_command_response(command, session.clone(), &controller)
                .await
                .map_err(|e| eprintln!("{:?}", e)).expect("");
            },
            mode_msg = mode_node.recv_async() => {
                let Ok(sample) = mode_msg else {
                    eprintln!("Zenoh mode channel is closed!");
                    break;
                };
                let curmode: RecvMessage<Mode> = sample.try_into().unwrap();
                mode = curmode.payload;
                println!("Received mode: {:?}", mode);
                if mode == Mode::Halt {
                    println!("Got HALT mode. Stopping...");
                    break;
                };
            },
            _ = ticker.tick() => {
                let latest_state = controller.get_state();
                state_node.put(SendMessage::new(latest_state))
                    .timestamp(session.new_timestamp())
                    .await
                    .map_err(|e| eprintln!("{:?}", e))
                    .ok();
            }
        }
    }
    session.close().await.unwrap();
}