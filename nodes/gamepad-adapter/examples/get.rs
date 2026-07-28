use serde::{Deserialize, Serialize};
use hermes_types::{SendMessage, RecvMessage, rpc::query_with};

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Scan{
    start_angle: f32,
    end_angle: f32,
    distances: Vec<f32>,
    quality: f32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Parameters{
    pub quality: u8,
    pub log: String
}

#[tokio::main]
async fn main() {
    let session = zenoh::open(zenoh::Config::default()).await.unwrap();
    let scan = query_with::<Parameters, Scan>(&session, "robot/log", 
                                                 Parameters{quality: 16, log: "Hello, Query!".to_string()}, 
                                                 std::time::Duration::from_millis(50)).await.unwrap();

    println!("Received: {:?} at time {:?}", scan, scan.system_time());
    // NOTE: don't attach a payload IF YOU DON'T NEED A PAYLOAD!!!! FOR GOD'S SAKE

    let replies = session.get("robot/log")
        .payload(SendMessage::new(Parameters{quality: 16, log: "Hello, Query!".to_string()}))
        .await.unwrap();
    
    while let Ok(reply) = replies.recv_async().await {
        let payload = match reply.into_result() {
            Ok(payload) => payload,
            Err(e) => {println!("Err: {}", e); continue;},
        };

        let scan: RecvMessage<Scan> = payload.try_into().unwrap();
        let curtime = scan.system_time();
        println!("Received: {:?} at time {:?}", scan.payload, curtime);
    }

}