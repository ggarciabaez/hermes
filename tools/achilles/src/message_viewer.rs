use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use eframe::egui;
use tokio::sync::RwLock;

use crate::decoders::decode;

#[derive(Clone)]
pub struct ViewerEntry {
    pub time: String,
    pub key: String,
    pub value: String,
}

/// Live message viewer for one pub topic. A single background task subscribes
/// (Exception 2 pattern), decodes each sample, and keeps only the latest entry.
/// The task calls `Context::request_repaint()` (throttled to ~30 Hz) so the
/// panel redraws as data arrives instead of waiting for the next user
/// interaction — and so a fast topic can't peg the UI at every sample rate.
pub struct MessageViewer {
    pub topic: Option<String>,
    join: Option<tokio::task::JoinHandle<()>>,
    latest: Arc<RwLock<Option<ViewerEntry>>>,
    count: Arc<AtomicU64>,
}

const REPAINT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(33);

impl MessageViewer {
    pub fn new() -> Self {
        Self {
            topic: None,
            join: None,
            latest: Arc::new(RwLock::new(None)),
            count: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn is_open(&self) -> bool {
        self.topic.is_some()
    }

    pub fn is_open_for(&self, topic: &str) -> bool {
        self.topic.as_deref() == Some(topic)
    }

    pub fn open(
        &mut self,
        session: zenoh::Session,
        runtime: &tokio::runtime::Runtime,
        ctx: egui::Context,
        topic: String,
    ) {
        self.close();
        let latest = self.latest.clone();
        let count = self.count.clone();
        let t = topic.clone();
        let join = runtime.spawn(async move {
            let mut last_repaint = std::time::Instant::now();
            match session.declare_subscriber(t.clone()).await {
                Ok(sub) => {
                    while let Ok(sample) = sub.recv_async().await {
                        let key = sample.key_expr().to_string();
                        let value = decode(&key, sample.payload());
                        let time = sample
                            .timestamp()
                            .map(|ts| {
                                let sys: SystemTime = ts.get_time().to_system_time();
                                format!("{:?}", sys)
                            })
                            .unwrap_or_default();
                        *latest.write().await = Some(ViewerEntry { time, key, value });
                        count.fetch_add(1, Ordering::Relaxed);
                        if last_repaint.elapsed() >= REPAINT_INTERVAL {
                            last_repaint = std::time::Instant::now();
                            ctx.request_repaint();
                        }
                    }
                }
                Err(e) => eprintln!("[viewer] failed to subscribe to {}: {:?}", t, e),
            }
        });
        self.topic = Some(topic);
        self.join = Some(join);
    }

    pub fn close(&mut self) {
        if let Some(j) = &mut self.join {
            j.abort();
        }
        self.join = None;
        self.topic = None;
        self.latest = Arc::new(RwLock::new(None));
        self.count = Arc::new(AtomicU64::new(0));
    }

    pub fn take_latest(&self) -> Option<ViewerEntry> {
        self.latest
            .try_read()
            .ok()
            .and_then(|g| g.clone())
    }

    pub fn received(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }
}

impl Default for MessageViewer {
    fn default() -> Self {
        Self::new()
    }
}