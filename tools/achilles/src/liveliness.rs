use std::time::Duration;
use egui_graphs::GraphChange; // Adjust imports as needed
use petgraph::stable_graph::{IndexType, NodeIndex};
use zenoh::sample::SampleKind;
use hermes_types::{Manifest, RecvMessage};
use hermes_types::rpc::query;
pub use tokio::sync::mpsc::{UnboundedSender, UnboundedReceiver, unbounded_channel};

pub enum LivelinessChange {
    Created{
        name: String,
        manifest: Manifest,
    },
    Dropped(String)
}

/// TODO: when implementing the full one, make it return all 3 separately
pub fn extract_domain(ke: &str) -> (String, String, String) {
    let k: Vec<&str> = ke.split("/").collect();
    (k[0].to_string(), k[1].to_string(), k[2].to_string())
}

pub fn extract_node<Ix: IndexType>(change: &GraphChange<Ix>) -> Option<NodeIndex<Ix>> {
    match change {
        GraphChange::NodeMoved { node, .. }
        | GraphChange::NodeDragStarted { node }
        | GraphChange::NodeDragEnded { node }
        | GraphChange::NodeSelected { node }
        | GraphChange::NodeDeselected { node }
        | GraphChange::NodeClicked { node }
        | GraphChange::NodeDoubleClicked { node }
        | GraphChange::NodeHoverEntered { node }
        | GraphChange::NodeHoverExited { node } => Some(*node),
        _ => None,
    }
}

pub async fn handle_liveliness(tx: UnboundedSender<LivelinessChange>){
    let session = zenoh::open(zenoh::Config::default()).await.unwrap();
    let replies = session
        .liveliness()
        .declare_subscriber("robot/*/liveliness")
        .history(true)
        .await.unwrap();

    while let Ok(sample) = replies.recv_async().await {
        let name = sample.key_expr().to_string();
        match sample.kind() {
            SampleKind::Put => {
                println!(">> New alive token: {}", name);
                let manifest_addr = name.replace("liveliness", "manifest").to_owned();

                let manifest: Manifest = query::<Manifest>(&session, &manifest_addr, Duration::from_millis(100))
                    .await
                    .unwrap_or(RecvMessage::new(Manifest::default()))
                    .payload;
                println!("manifest {:?}", manifest);
                tx.send(LivelinessChange::Created{name, manifest}).expect("Failed to send");
            },
            SampleKind::Delete => {
                println!(">> Token dropped: {}", name);
                tx.send(LivelinessChange::Dropped(name)).expect("Failed to send drop signal.");
            }
        }
    }
}