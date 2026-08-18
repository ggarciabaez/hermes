// TODO: we need a way to
mod messages;
pub mod rpc;
pub mod sm;
pub mod gpad;
pub mod params;

use std::fmt;
use std::string::ToString;
use std::time::Duration;
use serde::{Deserialize, Serialize};
use zenoh::handlers::fifo::RecvFut;
use zenoh::handlers::FifoChannelHandler;
use zenoh::pubsub::{Publisher, Subscriber};
use zenoh::query::Queryable;
use zenoh::{Wait, Session};
use std::sync::{Arc, RwLock as sRwLock};
use tokio::sync::RwLock as tRwLock;
use params::{serve_params_get, serve_params_list, serve_params_set, Tunable};
use hermes_macros::setup_endpoints;
pub use messages::{MsgMetadata, SendMessage, RecvMessage};
use crate::rpc::{query_with, QueryError, query};
use crate::sm::Mode;
use crate::params::{ParamsGetRequest, ParamValue};

pub const NAMESPACE: &str = "robot/";

// TODO: add nodes as we go. These are the ones that come to mind
// Remember to add priority to them!
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy)]
pub enum NodeName {
    Gamepad,
    SM,
    Lifeguard,
    Achilles,
    Controller,
    Params,
}
impl fmt::Display for NodeName {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}
impl NodeName {
    pub fn name(&self) -> String {
        match self {
            NodeName::Gamepad => "Gamepad".to_owned(),
            NodeName::SM => "State_Machine".to_owned(),
            NodeName::Lifeguard => "Lifeguard".to_owned(),
            NodeName::Achilles => "Achilles".to_owned(),
            NodeName::Controller => "Controller".to_owned(),
            NodeName::Params => "Params".to_owned(),
        }
    }
    pub fn make_endpoint(&self, endpoint: &str) -> String {
        self.domain() + endpoint
    }
    pub fn domain(&self) -> String {
        NAMESPACE.to_owned() + &*self.name().to_lowercase() + "/"
    }

    // TODO: sketch out an orchestrated launch system with python to launch everything step by step.
    pub fn get_priority(&self) -> u8 {
        match self {
            NodeName::Gamepad => 1,
            NodeName::SM => 0,
            NodeName::Lifeguard => 1,
            NodeName::Achilles => 9,
            NodeName::Controller => 2,
            NodeName::Params => 0,
        }
    }

}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Default)]
pub struct Manifest{
    pub publishes: Vec<String>,
    pub subscribes: Vec<String>,
    pub serves: Vec<String>,
}

pub struct TopicInfo<T> {  // TODO: switch out manifest strings for this. Start simple, build up.
    pub topic: String,
    pub item_type: Option<T>
}

pub struct SubscriberHandle{
    pub sub: Subscriber<FifoChannelHandler<zenoh::sample::Sample>>
}
impl SubscriberHandle {
    pub fn get(&self) -> RecvFut<'_, zenoh::sample::Sample> {
        self.sub.recv_async()
    }
}

pub struct QueryableHandle{
    pub q: Queryable<FifoChannelHandler<zenoh::query::Query>>
}
impl QueryableHandle {
    pub fn get(&self) -> RecvFut<'_, zenoh::query::Query> {
        self.q.recv_async()
    }
}

pub struct PublisherHandle{
    pub p: Publisher<'static>
}
impl PublisherHandle {
    pub async fn put<T: Serialize>(&self, payload: &T) -> Result<(), zenoh::Error> {
        self.p.put(SendMessage::new(payload)).await
    }
}


pub struct Node {
    session: Session,
    // IT IS USED FOR FUCK'S SAKE (not pub)
    pub liveliness: Option<zenoh::liveliness::LivelinessToken>,
    pub manifest: Arc<sRwLock<Manifest>>,
    pub name: NodeName,
    domain: String,
    // TODO: verify this
    _manifest_queryable: Queryable<()>,
}

// TODO: change declaration params from string to NodeName + Endpoint
// Then call the make_endpoint inside the declaration
// TODO: add either a variant or a way to let the node handle params.
// We can pass in an Arc<RwLock<impl Tunable>> and let the node handle it. right?
impl Node {
    pub async fn new(session_handle: &Session, node_name: &NodeName) -> Result<Self, zenoh::Error> {
        let session = session_handle.clone();
        let domain = node_name.domain();
        let manifest = Arc::new(sRwLock::new(Manifest {
            publishes: vec![], subscribes: vec![], serves: vec![]
        }));

        // 1. Clone the Arc so we can move a reference into the closure
        let manifest_clone = manifest.clone();

        // 2. Declare the queryable with a background callback
        let _manifest_queryable = session
            .declare_queryable(domain.clone() + "manifest")
            .callback(move |query| {
                // Zenoh callbacks are synchronous, so we use .wait() instead of .await
                let m = manifest_clone.read().unwrap();
                let mc = m.clone();
                let reply_data = SendMessage::new(&mc);

                query.reply(query.key_expr().clone(), reply_data)
                    .wait()
                    .unwrap_or_else(|e| eprintln!("Failed to reply to manifest query: {}", e));
            })
            .await?;

        Ok(Self{
            session,
            liveliness: None,
            manifest,
            name: *node_name,
            domain,
            _manifest_queryable
        })
    }

    pub async fn activate_liveliness(&mut self) -> Result<(), zenoh::Error> {
        let liveliness = self.session
            .liveliness()
            .declare_token(
                format!("robot/{}/liveliness", self.name.name().to_lowercase())
            ).await?;
        self.liveliness = Some(liveliness);
        Ok(())
    }


    fn update_manifest(&mut self, name: &str, endpoint_type: u8) {
        let mut m = self.manifest.write().unwrap();
        match endpoint_type {
            0 => {m.publishes.push(name.to_string());},
            1 => {m.subscribes.push(name.to_string());},
            2 => {m.serves.push(name.to_string());},
            _ => {}
        }
    }

    pub async fn declare_pub(&mut self, endpoint_name: &str) -> Result<PublisherHandle, zenoh::Error> {
        let name = self.domain.clone() + endpoint_name;
        println!("[P] Opening at {}", name);
        let p = self.session.declare_publisher(name.clone()).await?;
        self.update_manifest(&name, 0);
        Ok(PublisherHandle { p })
    }

    /// Takes the full endpoint name instead of just the topic.
    /// Other methods just take the topic given that the domain is stored in the class.
    pub async fn declare_sub(&mut self, key_expr: &str) -> Result<SubscriberHandle, zenoh::Error> {
        println!("[S] Listening at {}", key_expr);
        let p = self.session.declare_subscriber(key_expr.to_owned()).await?;
        self.update_manifest(key_expr, 1);
        Ok(SubscriberHandle { sub: p })
    }

    pub async fn declare_queryable(&mut self, endpoint_name: &str) -> Result<QueryableHandle, zenoh::Error> {
        let name = self.domain.clone() + endpoint_name;
        println!("[Q] Opening at {}", name);
        let p = self.session.declare_queryable(name.clone()).await?;
        self.update_manifest(&name, 2);
        Ok(QueryableHandle { q: p })
    }

    pub async fn fetch_constants(&self) -> Result<Vec<(String, ParamValue)>, QueryError> {
        let k = query_with::<ParamsGetRequest, Vec<(String, ParamValue)>>(
            &self.session,
            &NodeName::Params.make_endpoint("get"),
            ParamsGetRequest::Constants,
            Duration::from_millis(100),
        )
        .await?;
        Ok(k.payload)
    }
    pub async fn fetch_params(&self) -> Result<Vec<(String, ParamValue)>, QueryError> {
        let k = query_with::<ParamsGetRequest, Vec<(String, ParamValue)>>(
            &self.session,
            &NodeName::Params.make_endpoint("get"),
            ParamsGetRequest::Node(self.name),
            Duration::from_millis(100),
        )
        .await?;
        Ok(k.payload)
    }

    pub async fn get_mode(&self) -> Result<Mode, QueryError> {
        Ok(
            query::<Mode>(&self.session,
                         &NodeName::SM.make_endpoint("get"),
                         Duration::from_millis(100))
            .await?.payload
        )
    }
    pub async fn sub_mode(&mut self) -> Result<SubscriberHandle, zenoh::Error> {
        self.declare_sub(&NodeName::SM.make_endpoint("current")).await
    }

    pub async fn spawn_params_server<T>(&mut self, params: Arc<tRwLock<T>>) -> Result<(), zenoh::Error>
    where
        T: Tunable + Send + Sync + 'static,
    {
        setup_endpoints!(self,
            pubs: [
                params_changed = "params/changed"
            ],
            queries: [
                params_list = "params/list",
                params_set = "params/set",
                params_get = "params/get"
            ]
        );
        // 2. Spawn the background loop
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;

                    Ok(query) = params_set.get() => {
                        // Write lock needed to mutate the params
                        let mut p = params.write().await;
                        if let Ok(Some(info)) = serve_params_set(query, &mut *p).await {
                            params_changed.put(&info).await.ok();
                        }
                    },
                    Ok(query) = params_list.get() => {
                        // Only a read lock needed
                        let p = params.read().await;
                        serve_params_list(query, &*p).await.ok();
                    },
                    Ok(query) = params_get.get() => {
                        // Only a read lock needed
                        let p = params.read().await;
                        serve_params_get(query, &*p).await.ok();
                    },
                    else => {
                        // This triggers if the queryables are dropped or the session closes
                        break;
                    }
                }
            }
        });

        Ok(())
    }
}

/// Wait until the given node's liveliness token is seen, polling its
/// liveliness key until `timeout` expires. Returns whether the node came up.
///
/// This is the boot-ordering primitive from the Phase 11 plan: a node that
/// needs Params (or any other node) present before it acts calls this first.
pub async fn wait_for_liveliness(
    session: &zenoh::Session,
    node: &NodeName,
    timeout: Duration,
) -> bool {
    let key = format!("robot/{}/liveliness", node.name().to_lowercase());
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.checked_duration_since(std::time::Instant::now());
        let Some(remaining) = remaining else {
            return false;
        };
        let replies = match session.liveliness().get(key.clone()).timeout(remaining).await {
            Ok(replies) => replies,
            Err(_) => return false,
        };
        if let Ok(reply) = replies.recv_async().await
            && reply.result().is_ok()
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}