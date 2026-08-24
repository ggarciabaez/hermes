use rerun::{Color, LineStrips2D, Points2D, RecordingStreamBuilder, Boxes2D, Arrows2D};
use std::time::Duration;


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>>{
    let rec = RecordingStreamBuilder::new("Bike Viz").spawn()?;

    let mut step = 0;
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                rec.set_time_sequence("step", step);
                step += 1;
            },

            _ = tokio::signal::ctrl_c() => {
                break;
            }
        }
    }
    Ok(())
}
