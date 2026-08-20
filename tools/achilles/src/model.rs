use std::collections::{HashMap, HashSet};

use eframe::egui::{Color32, Pos2};
use egui_graphs::Graph;
use hermes_types::Manifest;
use petgraph::stable_graph::{DefaultIx, NodeIndex};
use petgraph::Directed;
use zenoh::key_expr::KeyExpr;

use crate::shape::{AchillesEdgeShape, AchillesNodeShape};

pub const DOMAIN_COLOR: Color32 = Color32::from_rgb(70, 130, 220);
pub const PUBLISHER_COLOR: Color32 = Color32::from_rgb(90, 190, 110);
pub const QUERYABLE_COLOR: Color32 = Color32::from_rgb(230, 160, 70);

/// The concrete graph type Achilles renders with: custom node and edge shapes.
pub type AchGraph = Graph<
    NodeData,
    EdgeData,
    Directed,
    DefaultIx,
    AchillesNodeShape,
    AchillesEdgeShape,
>;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum EndpointKind {
    Publisher,
    Queryable,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum NodeKind {
    Domain,
    Endpoint(EndpointKind),
}

/// Payload carried by every graph node.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct NodeData {
    pub kind: NodeKind,
    /// Domain name for domains, full key expr for endpoints.
    pub key: String,
    /// Only meaningful for domains: whether the manifest query has succeeded
    /// yet. False dims the node.
    pub manifest_loaded: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum EdgeKind {
    /// domain -> its own endpoint (ownership)
    Owner,
    /// publisher endpoint -> subscribing domain (data flow)
    Data,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EdgeData {
    pub kind: EdgeKind,
}

pub struct DomainRecord {
    pub node: NodeIndex,
    pub manifest: Manifest,
    /// full key exprs of this domain's endpoints (publishes + serves)
    pub endpoints: Vec<String>,
    /// subscription key exprs (may contain wildcards)
    pub subs: Vec<String>,
}

pub struct EndpointRecord {
    pub node: NodeIndex,
    pub owner: String,
    pub kind: EndpointKind,
}

/// Owns the egui_graphs graph plus the bookkeeping needed to build it from
/// liveliness + manifests: domain/endpoint maps and the current data edges.
pub struct GraphModel {
    g: AchGraph,
    domains: HashMap<String, DomainRecord>,
    endpoints: HashMap<String, EndpointRecord>,
    data_edges: Vec<petgraph::stable_graph::EdgeIndex>,
}

impl GraphModel {
    pub fn new() -> Self {
        Self {
            g: AchGraph::new(),
            domains: HashMap::new(),
            endpoints: HashMap::new(),
            data_edges: Vec::new(),
        }
    }

    pub fn g_mut(&mut self) -> &mut AchGraph {
        &mut self.g
    }

    pub fn domain_by_node(&self, idx: NodeIndex) -> Option<(&String, &DomainRecord)> {
        self.domains.iter().find(|(_, r)| r.node == idx)
    }

    pub fn endpoint_by_node(&self, idx: NodeIndex) -> Option<(&String, &EndpointRecord)> {
        self.endpoints.iter().find(|(_, r)| r.node == idx)
    }

    pub fn live_pub_topics(&self) -> Vec<String> {
        self.endpoints
            .iter()
            .filter(|(_, r)| matches!(&r.kind, EndpointKind::Publisher))
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Insert a domain (or replace it on liveliness re-put / manifest update).
    /// Any subscription edges that match newly added publishers are wired up by
    /// `reconcile_edges`.
    pub fn upsert_domain(&mut self, domain: &str, manifest: Manifest) {
        if self.domains.contains_key(domain) {
            self.remove_domain(domain);
        }

        let node = self.g.add_node_custom(
            NodeData {
                kind: NodeKind::Domain,
                key: domain.to_string(),
                manifest_loaded: !is_empty_manifest(&manifest),
            },
            |n| {
                n.set_label(domain.to_string());
                n.set_color(DOMAIN_COLOR);
                n.set_location(Pos2::new(
                    rand::random::<f32>() * 200.0 - 100.0,
                    rand::random::<f32>() * 200.0 - 100.0,
                ));
            },
        );

        let mut endpoints = Vec::new();
        for key in manifest.publishes.iter() {
            self.add_endpoint(key, domain, EndpointKind::Publisher, node);
            endpoints.push(key.clone());
        }
        for key in manifest.serves.iter() {
            self.add_endpoint(key, domain, EndpointKind::Queryable, node);
            endpoints.push(key.clone());
        }

        let subs = manifest.subscribes.clone();
        self.domains.insert(
            domain.to_string(),
            DomainRecord {
                node,
                manifest,
                endpoints,
                subs,
            },
        );

        self.reconcile_edges();
    }

    pub fn remove_domain(&mut self, domain: &str) {
        let Some(rec) = self.domains.remove(domain) else {
            return;
        };
        for key in &rec.endpoints {
            if let Some(erec) = self.endpoints.remove(key) {
                self.g.remove_node(erec.node);
            }
        }
        self.g.remove_node(rec.node);
        self.reconcile_edges();
    }

    fn add_endpoint(
        &mut self,
        key: &str,
        owner: &str,
        kind: EndpointKind,
        owner_node: NodeIndex,
    ) {
        let label = endpoint_label(key, owner);
        let node = self.g.add_node_custom(
            NodeData {
                kind: NodeKind::Endpoint(kind.clone()),
                key: key.to_string(),
                manifest_loaded: true,
            },
            |n| {
                n.set_label(label.clone());
                n.set_color(match kind {
                    EndpointKind::Publisher => PUBLISHER_COLOR,
                    EndpointKind::Queryable => QUERYABLE_COLOR,
                });
            },
        );
        self.g.add_edge_with_label(
            owner_node,
            node,
            EdgeData { kind: EdgeKind::Owner },
            String::new(),
        );
        self.endpoints.insert(
            key.to_string(),
            EndpointRecord {
                node,
                owner: owner.to_string(),
                kind,
            },
        );
    }

    /// Rebuild all data edges (publisher endpoint -> subscribing domain) from
    /// scratch. Idempotent; safe to call whenever the graph changes.
    pub fn reconcile_edges(&mut self) {
        for idx in self.data_edges.drain(..) {
            if self.g.edge(idx).is_some() {
                self.g.remove_edge(idx);
            }
        }

        let pub_endpoints: Vec<(String, NodeIndex)> = self
            .endpoints
            .iter()
            .filter(|(_, r)| matches!(&r.kind, EndpointKind::Publisher))
            .map(|(k, r)| (k.clone(), r.node))
            .collect();
        let domains: Vec<(String, NodeIndex, Vec<String>)> = self
            .domains
            .iter()
            .map(|(d, r)| (d.clone(), r.node, r.subs.clone()))
            .collect();

        let mut added: HashSet<String> = HashSet::new();
        for (domain, domain_node, subs) in &domains {
            for sub in subs {
                for (pub_key, pub_node) in &pub_endpoints {
                    if let (Ok(a), Ok(b)) = (KeyExpr::new(pub_key), KeyExpr::new(sub))
                        && a.intersects(&b)
                    {
                        let dedup = format!("{}->{}", pub_key, domain);
                        if added.insert(dedup) {
                            let idx = self.g.add_edge(
                                *pub_node,
                                *domain_node,
                                EdgeData { kind: EdgeKind::Data },
                            );
                            self.data_edges.push(idx);
                        }
                    }
                }
            }
        }
    }
}

impl Default for GraphModel {
    fn default() -> Self {
        Self::new()
    }
}

fn is_empty_manifest(m: &Manifest) -> bool {
    m.publishes.is_empty() && m.serves.is_empty() && m.subscribes.is_empty()
}

/// `robot/<domain>/params/changed` -> `params/changed`; keeps the domain when
/// the endpoint is deeper than the second segment so labels stay readable.
pub fn endpoint_label(key: &str, owner: &str) -> String {
    let prefix = format!("robot/{}/", owner);
    match key.strip_prefix(&prefix) {
        Some(tail) if !tail.is_empty() => tail.to_string(),
        _ => key.to_string(),
    }
}