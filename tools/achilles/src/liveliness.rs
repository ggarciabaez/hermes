use std::time::Duration;
use egui_graphs::GraphChange;
use petgraph::stable_graph::{IndexType, NodeIndex};
use zenoh::sample::SampleKind;
use hermes_types::{Manifest, RecvMessage};
use hermes_types::rpc::query;
use tokio::sync::mpsc::UnboundedSender;

pub enum LivelinessChange {
    Created{
        name: String,
        manifest: Manifest,
    },
    /// A re-fetch of a previously-empty manifest succeeded.
    ManifestUpdated{
        name: String,
        manifest: Manifest,
    },
    Dropped(String)
}

/// Split a `robot/<domain>/<endpoint...>` key into (namespace, domain,
/// endpoint). `splitn(3)` keeps domain as the second segment even when the
/// endpoint has more slashes (e.g. `robot/gamepad/params/changed`).
pub fn extract_domain(ke: &str) -> (String, String, String) {
    let mut parts = ke.splitn(3, '/');
    (
        parts.next().unwrap_or("").to_string(),
        parts.next().unwrap_or("").to_string(),
        parts.next().unwrap_or("").to_string(),
    )
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

fn manifest_empty(m: &Manifest) -> bool {
    m.publishes.is_empty() && m.serves.is_empty() && m.subscribes.is_empty()
}

pub async fn handle_liveliness(session: zenoh::Session, tx: UnboundedSender<LivelinessChange>){
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
                tx.send(LivelinessChange::Created{name: name.clone(), manifest: manifest.clone()}).expect("Failed to send");

                // The node is alive but its manifest query timed out (it may
                // still be declaring endpoints). Retry in the background and
                // push an update when a real manifest shows up.
                if manifest_empty(&manifest) {
                    let session = session.clone();
                    let tx = tx.clone();
                    let manifest_addr = manifest_addr.clone();
                    tokio::spawn(async move {
                        for _ in 0..6 {
                            tokio::time::sleep(Duration::from_millis(500)).await;
                            if let Ok(m) = query::<Manifest>(&session, &manifest_addr, Duration::from_millis(100)).await {
                                let m = m.payload;
                                if !manifest_empty(&m) {
                                    println!("manifest (retry) {:?}", m);
                                    tx.send(LivelinessChange::ManifestUpdated{name, manifest: m}).ok();
                                    return;
                                }
                            }
                        }
                    });
                }
            },
            SampleKind::Delete => {
                println!(">> Token dropped: {}", name);
                tx.send(LivelinessChange::Dropped(name)).expect("Failed to send drop signal.");
            }
        }
    }
}