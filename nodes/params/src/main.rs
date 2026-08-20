use clap::Parser;
use hermes_types::params::{params_from_toml, ParamsGetRequest, ParamValue};
use hermes_types::rpc::Request;
use hermes_types::{Node, NodeName, RecvMessage, SendMessage, sm::Mode, GlobalArgs};
use toml::Table;

const NAME: NodeName = NodeName::Params;

#[derive(clap::Parser, Debug)]
#[command(rename_all = "snake_case")]
struct Args {
    #[command(flatten)]
    global: GlobalArgs,
    
    #[arg(long)]
    #[clap(default_value = "./nodes/params/Config.toml")]
    cfg: String,
}
#[tokio::main]
async fn main() {
    let args = Args::parse();
    let config_raw = std::fs::read_to_string(args.cfg).unwrap();
    let config: Table = toml::from_str(&config_raw).unwrap();

    let session = zenoh::open(hermes_types::config::default_config()).await.unwrap();
    let mut node = Node::new(&session, &NAME).await.unwrap();
    let config_q = node.declare_queryable("get").await.unwrap();
    let mode_node = node.sub_mode().await.unwrap();
    node.activate_liveliness().await.unwrap();

    loop {
        tokio::select! {
            biased;
            Ok(mode_q) = mode_node.get() => {
                let curmode: RecvMessage<Mode> = mode_q.try_into().unwrap();
                if curmode.payload == Mode::Halt {
                    break
                }
            },
            Ok(config_req) = config_q.get() => {
                let request: Request<ParamsGetRequest> = match (&config_req).try_into() {
                    Ok(r) => r,
                    Err(_) => {
                        config_req.reply_err("Malformed params request").await.unwrap();
                        continue;
                    }
                };
                let response: Vec<(String, ParamValue)> = match request.parameters {
                    ParamsGetRequest::Constants => config
                        .get("constants")
                        .and_then(|v| v.as_table())
                        .map(params_from_toml)
                        .unwrap_or_default(),
                    ParamsGetRequest::Node(node) => {
                        let key = node.name().to_lowercase();
                        config
                            .get("params")
                            .and_then(|v| v.as_table())
                            .and_then(|t| t.get(&key))
                            .and_then(|v| v.as_table())
                            .map(params_from_toml)
                            .unwrap_or_default()
                    }
                };
                config_req
                    .reply(config_req.key_expr(), SendMessage::new(response))
                    .await
                    .unwrap();
            },
            _ctrl_c = tokio::signal::ctrl_c() => {
                println!("Ctrl-C received, exiting.");
                break;
            },
        }
    }
    session.close().await.unwrap();
}
