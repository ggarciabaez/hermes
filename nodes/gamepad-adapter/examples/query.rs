use serde::{Deserialize, Serialize};
use hermes_types::SendMessage;
use hermes_types::rpc::Request;

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
async fn main(){
    let session = zenoh::open(zenoh::Config::default()).await.unwrap();
    let queryable = session.declare_queryable("robot/log").await.unwrap();

    //let s = Arc::new(RwLock::new(Scan {
    let mut s = Scan {
        start_angle: 0.0,
        end_angle: 360.0,
        distances: vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0],
        quality: 100.0,
    };

   while let Ok(query) = queryable.recv_async().await {
       // You only need the Request<Parameters> if you're expecting detailed parameters with a custom type.
       // The standard API works just fine too. This is for custom types
       let payload: Request<Parameters> = (&query).try_into().unwrap();
       println!("{:?}", payload.log);
       s.quality = payload.quality as f32;
       query.reply("robot/log", SendMessage::new(&s)).await.expect("Failed to send response.");
   }
    /*
    // Works just as well
    while let Ok(query) = queryable.recv_async().await {
        if *query.parameters().get("mode").get_or_insert("") == "half"{
            s.end_angle = 180.0;
            query.reply("robot/log", SendMessage::new(&s)).await.expect("Failed to send response.");
        }
    }
     */
}