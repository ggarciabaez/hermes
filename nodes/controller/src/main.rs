// TODO: make it modular so that we can add more algorithms. Should be simple enough, right?
// A controller should have access to the network to fetch nodes or parameters. Take note.

mod utils;

use clap::Parser;
use hermes_types::{RecvMessage, NodeName, Node, sm::Mode, config::default_config, bike::Twist, wait_for_liveliness};
use hermes_macros::setup_endpoints;
use std::time::Duration;
use tokio::time::Interval;
use hermes_types::bike::BikeConfig;
use hermes_types::gpad::GPadDrive;
use hermes_types::rpc::query;

const NAME: NodeName = NodeName::Controller;

#[derive(clap::Parser, Debug)]
#[command(rename_all = "snake_case")]
struct Args {
    #[command(flatten)]
    global: hermes_types::GlobalArgs,

    #[arg(long)]
    #[clap(default_value = "gpad")]
    algo: String,
}

fn parse_into_twist(drive: GPadDrive, cfg: &BikeConfig) -> Twist {
    // The GPadDrive message is misformatted. We need to correct it.
    // V goes from -1 to 1, so does the steering angle.
    // V is at least properly handled with direction, so no conversion shenanigans.
    // just need to convert it to real units.
    let v = drive.v * cfg.max_v;
    let w = v * drive.y.tan() / cfg.l;
    Twist::new(v, w)
}

#[tokio::main]
async fn main() {
    let args: Args = Args::parse();
    let session = zenoh::open(default_config()).await.unwrap();
    let mut node: Node = Node::new(&session, &NAME).await.unwrap();
    let config: BikeConfig;
    let ctrl_addr = NodeName::Gamepad.domain() + "drive";

    if wait_for_liveliness(&session, &NodeName::Backend, Duration::from_secs(5)).await {
        let cfgmsg: RecvMessage<BikeConfig> = query(&session, &(NodeName::Backend.domain() + "get_cfg"), Duration::from_millis(100)).await.unwrap();
        config = cfgmsg.payload;
    } else {
        panic!("[Controller] Did not find Bike backend within 5s.");
    }

    setup_endpoints!(
        node,
        pubs: [
            gpad = "gpad",
        ],
        queries: []
    );
    let mode_node = node.sub_mode().await.unwrap();
    let drive_node = node.declare_sub(&ctrl_addr).await.unwrap();
    node.activate_liveliness().await.unwrap();

    let mut ticker: Interval = tokio::time::interval(Duration::from_millis(args.global.dt));
    let mut mode = Mode::Standby;
    loop {
        tokio::select! {
            biased;
            Ok(mode_msg) = mode_node.get() => {
                let curmode: RecvMessage<Mode> = mode_msg.try_into().unwrap();
                if curmode.payload == Mode::Halt {
                    println!("Got mode {:?}, exiting...", curmode.payload);
                    break;
                }
                mode = curmode.payload;
            },
            _ = ticker.tick() => {

            },
            Ok(drive_msg) = drive_node.get() => {
                let gpdrive: RecvMessage<GPadDrive> = drive_msg.try_into().unwrap();
                if matches!(mode, Mode::Manual | Mode::Auto) {
                    gpad.put(&parse_into_twist(gpdrive.payload, &config)).await.unwrap();
                } else {
                    gpad.put(&Twist::zero()).await.unwrap();
                }

            },
            _ctrl_c = tokio::signal::ctrl_c() => {
                println!("Ctrl-C received, exiting...");
                break;
            }
        }

    }
    session.close().await.unwrap();
}
