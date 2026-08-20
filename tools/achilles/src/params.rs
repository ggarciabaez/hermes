use std::collections::HashMap;
use std::time::Duration;

use hermes_types::params::{ParamInfo, ParamSetRequest, ParamValue};
use hermes_types::rpc::{query_all, query_with};
use tokio::sync::mpsc::UnboundedSender;

use crate::liveliness::extract_domain;

#[derive(Clone)]
pub struct ParamRow {
    pub domain: String,
    pub info: ParamInfo,
}

pub enum ParamEvent {
    Loaded(Vec<ParamRow>),
    SetResult {
        domain: String,
        key: String,
        result: Result<(), String>,
    },
    Changed {
        domain: String,
        info: ParamInfo,
    },
}

/// Live parameter table state: the aggregated rows plus in-progress edit
/// buffers. All mutations go through the owning node's `params/set` RPC; the
/// UI never writes a cached value as if it were authoritative.
pub struct ParamsPanel {
    pub rows: Vec<ParamRow>,
    pub loading: bool,
    pub status: Option<String>,
    pub edits: HashMap<String, String>,
}

impl ParamsPanel {
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            loading: false,
            status: None,
            edits: HashMap::new(),
        }
    }

    pub fn row(&self, domain: &str, name: &str) -> Option<&ParamRow> {
        self.rows
            .iter()
            .find(|r| r.domain == domain && r.info.name == name)
    }

    pub fn apply_loaded(&mut self, rows: Vec<ParamRow>) {
        self.loading = false;
        self.rows = rows;
        self.edits.clear();
    }

    pub fn apply_changed(&mut self, domain: &str, info: ParamInfo) {
        if let Some(row) = self
            .rows
            .iter_mut()
            .find(|r| r.domain == domain && r.info.name == info.name)
        {
            row.info = info;
        }
    }
}

impl Default for ParamsPanel {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse the user's edit string into a `ParamValue` matching the param's
/// current type.
pub fn parse_param_value(info: &ParamInfo, text: &str) -> Result<ParamValue, String> {
    match &info.value {
        ParamValue::Float(_) => text
            .trim()
            .parse::<f64>()
            .map(ParamValue::Float)
            .map_err(|_| format!("'{}' is not a valid float", text)),
        ParamValue::Int(_) => text
            .trim()
            .parse::<i64>()
            .map(ParamValue::Int)
            .map_err(|_| format!("'{}' is not a valid integer", text)),
        ParamValue::Bool(_) => match text.trim().to_lowercase().as_str() {
            "true" | "1" => Ok(ParamValue::Bool(true)),
            "false" | "0" => Ok(ParamValue::Bool(false)),
            _ => Err(format!("'{}' is not a valid boolean (true/false)", text)),
        },
        ParamValue::Str(_) => Ok(ParamValue::Str(text.to_string())),
    }
}

pub fn format_param_value(v: &ParamValue) -> String {
    match v {
        ParamValue::Float(f) => format!("{}", f),
        ParamValue::Int(i) => format!("{}", i),
        ParamValue::Bool(b) => format!("{}", b),
        ParamValue::Str(s) => s.clone(),
    }
}

pub fn format_bounds(b: &Option<hermes_types::params::Bounds<ParamValue>>) -> String {
    match b {
        Some(bounds) => format!(
            "[{}, {}]",
            format_param_value(&bounds.low),
            format_param_value(&bounds.high)
        ),
        None => "—".to_string(),
    }
}

/// Fan-out `get` across every discovered node's `params/list` queryable
/// (`robot/*/params/list`), aggregating the replies per node.
pub fn spawn_refresh(
    session: zenoh::Session,
    runtime: &tokio::runtime::Runtime,
    tx: UnboundedSender<ParamEvent>,
) {
    runtime.spawn(async move {
        let mut rows = Vec::new();
        match query_all::<Vec<ParamInfo>>(
            &session,
            "robot/*/params/list",
            Duration::from_millis(600),
        )
        .await
        {
            Ok(replies) => {
                for reply in replies {
                    let key = reply.key_expr().to_string();
                    let (_, domain, _) = extract_domain(&key);
                    for info in reply.payload {
                        rows.push(ParamRow {
                            domain: domain.clone(),
                            info,
                        });
                    }
                }
            }
            Err(e) => {
                tx.send(ParamEvent::Loaded(rows)).ok();
                eprintln!("[params] fan-out list failed: {:?}", e);
                return;
            }
        }
        tx.send(ParamEvent::Loaded(rows)).ok();
    });
}

/// Fire a real `params/set` RPC at the owning node.
pub fn spawn_set(
    session: zenoh::Session,
    runtime: &tokio::runtime::Runtime,
    tx: UnboundedSender<ParamEvent>,
    domain: String,
    key: String,
    value: ParamValue,
) {
    runtime.spawn(async move {
        let req = ParamSetRequest {
            key: key.clone(),
            value,
        };
        let result = match query_with::<ParamSetRequest, bool>(
            &session,
            &format!("robot/{}/params/set", domain),
            req,
            Duration::from_millis(300),
        )
        .await
        {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("{:?}", e)),
        };
        tx.send(ParamEvent::SetResult { domain, key, result })
            .ok();
    });
}

/// Live-update subscription to every node's `params/changed` publish.
pub async fn handle_params_changed(session: zenoh::Session, tx: UnboundedSender<ParamEvent>) {
    if let Ok(sub) = session.declare_subscriber("robot/*/params/changed").await {
        while let Ok(sample) = sub.recv_async().await {
            let key = sample.key_expr().to_string();
            let (_, domain, _) = extract_domain(&key);
            if let Ok(msg) = hermes_types::RecvMessage::<ParamInfo>::try_from(sample) {
                tx.send(ParamEvent::Changed {
                    domain,
                    info: msg.payload,
                })
                .ok();
            }
        }
    }
}