use serde::{Deserialize, Serialize};
use hermes_types::RecvMessage;

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Scan{
    start_angle: f32,
    end_angle: f32,
    distances: Vec<f32>,
    quality: f32,
}

#[tokio::main]
async fn main() {
    let session = zenoh::open(zenoh::Config::default()).await.unwrap();
    let subscriber = session.declare_subscriber("robot/scan").await.unwrap();

    while let Ok(sample) = subscriber.recv_async().await {
        let scan: RecvMessage<Scan> = sample.try_into().unwrap();
        let curtime = scan.system_time();
        println!("Received: {:?} at time {:?}", scan.payload, curtime);
        if scan.quality == 10.0{
            break;
        }
    };
}