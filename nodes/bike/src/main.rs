mod sim;

use clap::Parser; // cause we will def be needing args.
// Also, for that matter, with the orchestrator we def don't need the params node, right?
// We just pass that stuff in as arguments to the command. Anyway...
use hermes_types::{
    SendMessage, RecvMessage, NodeName, Node, sm::Mode,
    config::default_config,
    bike::{BikeConfig, Twist}
};
use hermes_macros::setup_endpoints;
use sim::*;
use std::time;
use nalgebra::{Vector3};
use tokio::time::Interval;
const NAME: NodeName = NodeName::Backend;

#[derive(clap::Parser, Debug)]
#[command(rename_all = "snake_case")]
struct Args {
    #[command(flatten)]
    global: hermes_types::GlobalArgs,

    #[arg(long)]
    twist_pub: String,

    #[arg(long)]
    #[clap(default_value_t = 10.0)]
    v: f32,
    
    #[arg(long)]
    #[clap(default_value_t = 5.0)]
    a: f32,
    
    #[arg(long)]
    #[clap(default_value_t = std::f32::consts::FRAC_PI_8)]  // 22.5 deg
    steer: f32,
    
    #[arg(long)]
    #[clap(default_value_t = 0.324)]
    l: f32,
    
    #[arg(long)]
    #[clap(default_value_t = 0.01)]  // be quite conservative with this, especially relative to l
    understeer: f32
}

#[tokio::main]
async fn main() {
    let args: Args = Args::parse();
    let session = zenoh::open(default_config()).await.unwrap();
    let mut node: Node = Node::new(&session, &NAME).await.unwrap();
    let cfg = BikeConfig {
        dt: time::Duration::from_millis(args.global.dt),  // TODO: this needs to be cast from a f32 to time::Duration
        max_v: args.v,
        max_a: args.a,
        max_steer: args.steer,
        l: args.l,
        understeer: args.understeer
    };
    let mut bike: Bicycle = Bicycle::from_cfg(Vector3::zeros(), cfg);

    setup_endpoints!(
        node,
        pubs: [
            true_pos = "true_pos",
            odom = "odom",
        ],
        queries: [
            get_cfg = "get_cfg",
        ]
    );
    let twist_loc = NodeName::Controller.domain() + &args.twist_pub;
    let twist_node = node.declare_sub(&twist_loc).await.unwrap();
    let mode_node = node.sub_mode().await.unwrap();
    
    let mut ticker: Interval = tokio::time::interval(time::Duration::from_millis(args.global.dt));
    node.activate_liveliness().await.unwrap();
    let mut latest_input = Twist {v: 0.0, w: 0.0};
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
                let (ddist, dangle) = bike.step(latest_input);
                odom.put(&Twist::new(ddist, dangle)).await.unwrap();
            },
            Ok(twist_msg) = twist_node.get() => {
                let drive: RecvMessage<Twist> = twist_msg.try_into().unwrap();
                if matches!(mode, Mode::Manual | Mode::Auto){
                    latest_input = drive.payload;
                } else {
                    latest_input = Twist::zero();
                }

            },
            Ok(cfg_req) = get_cfg.get() => {
                cfg_req.reply(cfg_req.key_expr(), SendMessage::new(&bike.cfg)).await.unwrap();
            }
            
            _ctrl_c = tokio::signal::ctrl_c() => {
                println!("Ctrl-C received, exiting...");
                break;
            }
        }

    }
    session.close().await.unwrap();
}
