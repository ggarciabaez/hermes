//! Comms stats for Achilles: harvests zenoh's built-in per-node transport
//! metrics from the admin space (`@/**/metrics`, Prometheus text) and shows
//! live tx/rx volumes, message-type mix, and drop counters per peer.
//!
//! This relies on each node opening its session with
//! [`hermes_types::config::default_config`], which enables the admin space and
//! the `stats` feature already active in the workspace zenoh dependency.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedSender;

/// Aggregated counters for a single peer (one admin-space `metrics` reply).
#[derive(Clone, Debug, Default)]
pub struct PeerStats {
    /// Zenoh id as it appears in the admin key (`@/<zid>/<whatami>/metrics`).
    pub zid: String,
    pub whatami: String,
    /// Cumulative bytes moved over the transport (summed across links).
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    /// Network messages by type (put/query/reply/...) on the user space.
    pub tx_msgs: HashMap<String, u64>,
    pub rx_msgs: HashMap<String, u64>,
    /// Dropped payload bytes by drop reason (congestion, no-link, ...).
    pub tx_dropped: HashMap<String, u64>,
    pub rx_dropped: HashMap<String, u64>,
    /// Live resource gauges.
    pub transports: u64,
    pub links: u64,
    pub resources: u64,
}

/// One poll of the mesh: counters for every peer that answered.
#[derive(Clone, Debug, Default)]
pub struct CommsSnapshot {
    pub peers: Vec<PeerStats>,
}

impl CommsSnapshot {
    pub fn total_tx(&self) -> u64 {
        self.peers.iter().map(|p| p.tx_bytes).sum()
    }
    pub fn total_rx(&self) -> u64 {
        self.peers.iter().map(|p| p.rx_bytes).sum()
    }
}

pub enum CommsEvent {
    Loaded(CommsSnapshot),
    Failed(String),
}

/// View state for the Comms tab. `prev` is kept to compute per-second rates.
pub struct CommsPanel {
    pub snapshot: Option<CommsSnapshot>,
    prev: Option<(Instant, CommsSnapshot)>,
    pub loading: bool,
    pub status: Option<String>,
    last_poll: Instant,
}

impl CommsPanel {
    pub fn new() -> Self {
        Self {
            snapshot: None,
            prev: None,
            loading: false,
            status: None,
            last_poll: Instant::now() - POLL_INTERVAL,
        }
    }

    pub fn apply(&mut self, snap: CommsSnapshot) {
        self.prev = self.snapshot.take().map(|s| (Instant::now(), s));
        self.snapshot = Some(snap);
        self.loading = false;
        self.status = None;
    }

    pub fn due_for_poll(&self) -> bool {
        !self.loading && self.last_poll.elapsed() >= POLL_INTERVAL
    }

    pub fn poll_started(&mut self) {
        self.last_poll = Instant::now();
        self.loading = true;
    }

    /// Bytes/second deltas against the previous snapshot, if any.
    pub fn rates(&self) -> Option<(f64, f64)> {
        let (at, prev) = self.prev.as_ref()?;
        let cur = self.snapshot.as_ref()?;
        let dt = at.elapsed().as_secs_f64().max(0.001);
        let dtx = cur.total_tx().saturating_sub(prev.total_tx());
        let drx = cur.total_rx().saturating_sub(prev.total_rx());
        Some((dtx as f64 / dt, drx as f64 / dt))
    }
}

impl Default for CommsPanel {
    fn default() -> Self {
        Self::new()
    }
}

/// How often the Comms tab re-polls the mesh while visible.
const POLL_INTERVAL: Duration = Duration::from_secs(3);
const QUERY_TIMEOUT: Duration = Duration::from_millis(800);
/// The admin-space metrics queryable, gated behind the `stats` feature.
/// Parameters are `;`-separated (zenoh's parameter syntax). Only the base
/// families are requested; the per-transport/per-link/per-key variants would
/// otherwise repeat the same bytes/messages and inflate totals.
const METRICS_SELECTOR: &str =
    "@/**/metrics?compression=false;per_transport=false;per_link=false;per_key=false";

/// Trigger one metrics poll on the runtime; results come back on `tx`.
pub fn spawn_refresh(
    session: zenoh::Session,
    runtime: &tokio::runtime::Runtime,
    tx: UnboundedSender<CommsEvent>,
) {
    runtime.spawn(async move {
        match fetch_metrics(&session).await {
            Ok(snap) => {
                tx.send(CommsEvent::Loaded(snap)).ok();
            }
            Err(e) => {
                tx.send(CommsEvent::Failed(e)).ok();
            }
        }
    });
}

async fn fetch_metrics(session: &zenoh::Session) -> Result<CommsSnapshot, String> {
    let replies = session
        .get(METRICS_SELECTOR)
        .timeout(QUERY_TIMEOUT)
        .await
        .map_err(|e| format!("comms get failed: {:?}", e))?;

    let mut peers = Vec::new();
    while let Ok(reply) = replies.recv_async().await {
        let sample = match reply.result() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[comms] reply error: {:?}", e);
                continue;
            }
        };
        let mut ps = PeerStats::default();
        if let Ok(text) = std::str::from_utf8(&sample.payload().to_bytes()) {
            parse_metrics(text, &mut ps);
        }
        // Reply key is `@/<zid>/<whatami>/metrics`.
        let key = sample.key_expr().to_string();
        let parts: Vec<&str> = key.split('/').collect();
        if parts.len() >= 3 && parts[0] == "@" {
            ps.zid = parts[1].to_string();
            ps.whatami = parts[2].to_string();
        }
        peers.push(ps);
    }

    peers.sort_by(|a, b| a.zid.cmp(&b.zid));
    Ok(CommsSnapshot { peers })
}

/// Parse a Prometheus text dump into a [`PeerStats`], summing every series of
/// a given family (per-link/per-message breakdowns collapse into totals).
fn parse_metrics(text: &str, ps: &mut PeerStats) {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name_and_labels, value)) = line.split_once(' ') else {
            continue;
        };
        let Ok(value) = value.trim().parse::<f64>() else {
            continue;
        };
        // Skip histogram buckets/counts; the dropped-payload totals live on `_sum`.
        if name_and_labels.ends_with("_bucket") || name_and_labels.ends_with("_count") {
            continue;
        }

        let (name, labels) = split_name_labels(name_and_labels);
        // prometheus-client appends `_total` to counter metric names; gauges
        // keep their bare name.
        match name.as_str() {
            "zenoh_tx_bytes_total" => ps.tx_bytes = ps.tx_bytes.saturating_add(value as u64),
            "zenoh_rx_bytes_total" => ps.rx_bytes = ps.rx_bytes.saturating_add(value as u64),
            "zenoh_transports_opened" => ps.transports = value as u64,
            "zenoh_links_opened" => ps.links = value as u64,
            "zenoh_resources_declared" => ps.resources = value as u64,
            "zenoh_tx_network_message_total" => {
                bump(&mut ps.tx_msgs, labels.get("message"), value as u64)
            }
            "zenoh_rx_network_message_total" => {
                bump(&mut ps.rx_msgs, labels.get("message"), value as u64)
            }
            _ => {}
        }

        if name.ends_with("_dropped_payload_bytes_sum")
            && let Some(reason) = labels.get("reason")
        {
            if name.starts_with("zenoh_tx_") {
                bump(&mut ps.tx_dropped, Some(reason), value as u64);
            } else if name.starts_with("zenoh_rx_") {
                bump(&mut ps.rx_dropped, Some(reason), value as u64);
            }
        }
    }
}

fn bump(map: &mut HashMap<String, u64>, key: Option<&String>, value: u64) {
    if let Some(k) = key {
        *map.entry(k.clone()).or_insert(0) += value;
    }
}

/// Split `name{label="v",label2="v2"}` into the metric name and label map.
fn split_name_labels(s: &str) -> (String, HashMap<String, String>) {
    let Some(open) = s.find('{') else {
        return (s.to_string(), HashMap::new());
    };
    let mut labels = HashMap::new();
    if let Some(inner) = s.get(open + 1..s.len().saturating_sub(1)) {
        for part in inner.split(',') {
            if let Some((k, v)) = part.split_once('=') {
                labels.insert(
                    k.trim().to_string(),
                    v.trim().trim_matches('"').to_string(),
                );
            }
        }
    }
    (s[..open].to_string(), labels)
}

/// Short human-readable id (first 8 chars of the hex zid).
pub fn short_zid(zid: &str) -> String {
    zid.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_metrics() {
        let text = r#"# HELP zenoh_build Zenoh build version.
# TYPE zenoh_build info
zenoh_build_info{local_id="aabbccddeeff00112233445566778899",local_whatami="peer",version="v1.10.0"} 1
# HELP zenoh_transports_opened Count of transports currently opened.
# TYPE zenoh_transports_opened gauge
zenoh_transports_opened{local_id="aabbccddeeff00112233445566778899",local_whatami="peer"} 1
zenoh_links_opened{local_id="aabbccddeeff00112233445566778899",local_whatami="peer",protocol="tcp"} 1
zenoh_tx_bytes_total{local_id="aabbccddeeff00112233445566778899",local_whatami="peer",protocol="tcp"} 5581
zenoh_rx_bytes_total{local_id="aabbccddeeff00112233445566778899",local_whatami="peer",protocol="tcp"} 4321
zenoh_tx_network_message_total{local_id="aabbccddeeff00112233445566778899",local_whatami="peer",priority="control",message="declare",shm="false",protocol="tcp"} 5
zenoh_tx_network_message_total{local_id="aabbccddeeff00112233445566778899",local_whatami="peer",priority="data",message="put",shm="false",protocol="tcp"} 300
zenoh_tx_network_message_payload_bytes_sum{local_id="aabbccddeeff00112233445566778899",local_whatami="peer",space="user",priority="data",message="put",shm="false"} 2590.0
zenoh_tx_network_message_payload_bytes_count{local_id="aabbccddeeff00112233445566778899",local_whatami="peer",space="user",priority="data",message="put",shm="false"} 300
zenoh_tx_network_message_payload_bytes_bucket{local_id="aabbccddeeff00112233445566778899",local_whatami="peer",space="user",priority="data",message="put",shm="false",le="+Inf"} 300
zenoh_tx_network_message_dropped_payload_bytes_sum{local_id="aabbccddeeff00112233445566778899",local_whatami="peer",space="user",priority="data",message="put",shm="false",reason="congestion"} 42.0
"#;
        let mut ps = PeerStats::default();
        parse_metrics(text, &mut ps);

        assert_eq!(ps.transports, 1);
        assert_eq!(ps.links, 1);
        assert_eq!(ps.tx_bytes, 5581);
        assert_eq!(ps.rx_bytes, 4321);
        assert_eq!(ps.tx_msgs.get("put"), Some(&300));
        assert_eq!(ps.tx_msgs.get("declare"), Some(&5));
        assert_eq!(ps.tx_msgs.get("query"), None);
        assert_eq!(ps.tx_dropped.get("congestion"), Some(&42));
    }

    #[test]
    fn split_labels_handles_no_labels() {
        assert_eq!(split_name_labels("zenoh_resources_declared{local_id=\"x\"}").0, "zenoh_resources_declared");
        let (name, labels) = split_name_labels("m{a=\"b\",c=\"d\"}");
        assert_eq!(name, "m");
        assert_eq!(labels.get("a"), Some(&"b".to_string()));
        assert_eq!(labels.get("c"), Some(&"d".to_string()));
    }
}