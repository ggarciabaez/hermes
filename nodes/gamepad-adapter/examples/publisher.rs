use serde::{Deserialize, Serialize};
use hermes_types::SendMessage;

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
    let mut s = Scan {
        start_angle: 0.0,
        end_angle: 360.0,
        distances: vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0],
        quality: 0.0
    };
    for i in 1..=10{
        s.quality = i as f32;
        session.put("robot/scan", SendMessage::new(&s))
            .timestamp(session.new_timestamp())
            .await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    session.close().await.unwrap();
}