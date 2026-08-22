use clap::Parser;
use hermes_types::{Node, NodeName, RecvMessage};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use hermes_types::gpad::GPadDrive;

const NAME: NodeName = NodeName::Unknown;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value_t = 1)]
    n: u32,

    #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = false, hide = true)]
    extra_args: Vec<String>
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let session = zenoh::open(hermes_types::config::default_config()).await.unwrap();

    // 1. Create a central channel to funnel all subscriber events
    let (tx, mut rx) = mpsc::channel(1024);
    let token = CancellationToken::new();
    let mut node_list: Vec<Node> = Vec::new();
    for i in 0..args.n {
        let mut node = Node::new(&session, &NAME).await.unwrap();
        let name = format!("load{}", i);
        node.custom_liveliness(&name).await.unwrap();

        // Assuming declare_sub returns the handle you need to listen to
        let sub_handle = node.sub_mode().await.unwrap();
        let ctrl_addr = NodeName::Gamepad.domain() + "drive";
        let ctrl_sub = node.declare_sub(&ctrl_addr).await.unwrap();
        let tx_clone = tx.clone();
        let token_clone = token.clone();
        node_list.push(node);

        // 2. Spawn an independent task for each subscriber
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Ok(msg) = sub_handle.get() => {
                        let mode: RecvMessage<hermes_types::sm::Mode> = msg.try_into().unwrap();
                        if tx_clone.send(mode.payload).await.is_err() {
                            break;
                        }
                    },
                    Ok(drv) = ctrl_sub.get() => {
                        let drivein: RecvMessage<GPadDrive> = drv.try_into().unwrap();
                        println!("[LOAD] {:?} got drive: {:?}", i, drivein.payload);
                    },
                    _ = token_clone.cancelled() => {
                        break;
                    }
                }
            }
        });
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }

    // Drop the original sender so the receiver can close when workers finish
    drop(tx);

    // 3. Central loop processes the funneled messages
    loop {
        tokio::select! {
            biased;
            Some(msg) = rx.recv() => {
                println!("[LOAD] {:?} got mode.", msg);
                if msg == hermes_types::sm::Mode::Halt {
                    break;
                }
            }
            // You can easily add static branches here (like Ctrl+C or timers)
            _ctrl_c = tokio::signal::ctrl_c() => {
                println!("[LOAD] Ctrl-C received, exiting.");
                break;
            },
        }
    }
    println!("{:?}", node_list.iter().map(|n| n.name).collect::<Vec<_>>());
    drop(node_list);
    session.close().await.unwrap();
}
