use serde::{Deserialize, Serialize};
use zenoh::query::Query;

use crate::rpc::Request;
use crate::{NodeName, SendMessage};

/// A type-erased parameter value used for all cross-node param traffic.
///
/// Every param-owning node stores its params typed internally (`Param<f64>`,
/// `Param<bool>`, ...) and converts to/from this enum at the boundary with
/// Zenoh — a boot-loaded value and a live `set` RPC both arrive as a
/// `ParamValue`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum ParamValue {
    Float(f64),
    Int(i64),
    Bool(bool),
    Str(String),
}

impl ParamValue {
    /// Extract a scalar value from a TOML value. Arrays/tables/datetimes are
    /// not scalar params and yield `None`.
    pub fn from_toml(value: &toml::Value) -> Option<ParamValue> {
        match value {
            toml::Value::Float(f) => Some(ParamValue::Float(*f)),
            toml::Value::Integer(i) => Some(ParamValue::Int(*i)),
            toml::Value::Boolean(b) => Some(ParamValue::Bool(*b)),
            toml::Value::String(s) => Some(ParamValue::Str(s.clone())),
            _ => None,
        }
    }
}

impl From<f64> for ParamValue {
    fn from(value: f64) -> Self {
        ParamValue::Float(value)
    }
}
impl From<i64> for ParamValue {
    fn from(value: i64) -> Self {
        ParamValue::Int(value)
    }
}
impl From<bool> for ParamValue {
    fn from(value: bool) -> Self {
        ParamValue::Bool(value)
    }
}
impl From<String> for ParamValue {
    fn from(value: String) -> Self {
        ParamValue::Str(value)
    }
}
impl From<&str> for ParamValue {
    fn from(value: &str) -> Self {
        ParamValue::Str(value.to_owned())
    }
}

/// Float is lenient about integer values so a TOML `x = 1` can back a
/// `Param<f64>`.
impl TryFrom<ParamValue> for f64 {
    type Error = ParamError;
    fn try_from(value: ParamValue) -> Result<Self, Self::Error> {
        match value {
            ParamValue::Float(f) => Ok(f),
            ParamValue::Int(i) => Ok(i as f64),
            _ => Err(ParamError::TypeMismatch),
        }
    }
}
impl TryFrom<ParamValue> for i64 {
    type Error = ParamError;
    fn try_from(value: ParamValue) -> Result<Self, Self::Error> {
        match value {
            ParamValue::Int(i) => Ok(i),
            _ => Err(ParamError::TypeMismatch),
        }
    }
}
impl TryFrom<ParamValue> for bool {
    type Error = ParamError;
    fn try_from(value: ParamValue) -> Result<Self, Self::Error> {
        match value {
            ParamValue::Bool(b) => Ok(b),
            _ => Err(ParamError::TypeMismatch),
        }
    }
}
impl TryFrom<ParamValue> for String {
    type Error = ParamError;
    fn try_from(value: ParamValue) -> Result<Self, Self::Error> {
        match value {
            ParamValue::Str(s) => Ok(s),
            _ => Err(ParamError::TypeMismatch),
        }
    }
}

/// Min/max validation window for a parameter.
///
/// Only min/max is supported for now. An allowed-set variant is a known
/// possible future extension, but no current param needs one, so it's not
/// built speculatively.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Bounds<T> {
    pub low: T,
    pub high: T,
}

impl<T: PartialOrd> Bounds<T> {
    pub fn contains(&self, value: &T) -> bool {
        value >= &self.low && value <= &self.high
    }
}

/// A typed parameter owned by a node: a value plus optional validation.
///
/// `set()` is the single choke point for mutating a param — it rejects values
/// outside `Bounds` before they ever land. Both boot loading and a live tweak
/// route through here.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Param<T> {
    pub name: String,
    pub value: T,
    pub bounds: Option<Bounds<T>>,
}

impl<T: PartialOrd> Param<T> {
    pub fn get(&self) -> &T {
        &self.value
    }

    pub fn set(&mut self, value: T) -> Result<(), ParamError> {
        if let Some(bounds) = &self.bounds
            && !bounds.contains(&value)
        {
            return Err(ParamError::OutOfRange);
        }
        self.value = value;
        Ok(())
    }
}

/// Extract the scalar params of a TOML section into key/value pairs.
///
/// Non-scalar values (arrays/tables) are skipped — params crossing Zenoh are
/// scalars, and postcard cannot round-trip `toml::Value` anyway, so the wire
/// shape is always the type-erased [`ParamValue`] pairs this returns.
pub fn params_from_toml(table: &toml::Table) -> Vec<(String, ParamValue)> {
    table
        .iter()
        .filter_map(|(key, value)| ParamValue::from_toml(value).map(|pv| (key.clone(), pv)))
        .collect()
}

/// Type-erased snapshot of a parameter, returned by a node's `list` queryable
/// and published on its `changed` topic. Deliberately not generic — this is
/// the shape that actually crosses the wire.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ParamInfo {
    pub name: String,
    pub value: ParamValue,
    pub bounds: Option<Bounds<ParamValue>>,
}

impl<T: Clone + Into<ParamValue>> From<&Param<T>> for ParamInfo {
    fn from(param: &Param<T>) -> Self {
        ParamInfo {
            name: param.name.clone(),
            value: param.value.clone().into(),
            bounds: param.bounds.as_ref().map(|b| Bounds {
                low: b.low.clone().into(),
                high: b.high.clone().into(),
            }),
        }
    }
}

/// Domain-level rejection from a `Tunable::set`. The serving side turns this
/// into a `reply_err` string; the caller-side `QueryError` stays transport-level.
#[derive(Debug)]
pub enum ParamError {
    Unknown(String),
    TypeMismatch,
    OutOfRange,
}

impl std::fmt::Display for ParamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParamError::Unknown(key) => write!(f, "Unknown parameter: {}", key),
            ParamError::TypeMismatch => write!(f, "Value type does not match parameter"),
            ParamError::OutOfRange => write!(f, "Value outside parameter bounds"),
        }
    }
}

/// Live-adjustable parameters, implemented by every param-owning node.
///
/// `set` is the single code path by which a tunable ever changes — whether
/// the value came from a boot-time `[params.<node>]` slice or a live `set`
/// RPC.
pub trait Tunable {
    fn get(&self, key: &str) -> Option<ParamValue>;
    fn set(&mut self, key: &str, value: ParamValue) -> Result<(), ParamError>;
    fn list(&self) -> Vec<ParamInfo>;
}

/// Payload for a node's `params/set` RPC.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ParamSetRequest {
    pub key: String,
    pub value: ParamValue,
}

/// Payload for the Params node's `get` queryable — which slice the caller
/// wants back.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ParamsGetRequest {
    Constants,
    Node(NodeName),
}

/// Serve the node's `params/list` queryable: no-payload get, replies with the
/// full snapshot.
pub async fn serve_params_list<T: Tunable>(
    query: Query,
    tunable: &T,
) -> Result<(), zenoh::Error> {
    query
        .reply(query.key_expr().clone(), SendMessage::new(tunable.list()))
        .await
}

/// Serve the node's `params/set` RPC. Replies with a `bool` ack on success or
/// a `reply_err` on rejection. On a successful mutation, returns the updated
/// `ParamInfo` so the caller can publish it on the `params/changed` topic.
pub async fn serve_params_set<T: Tunable>(
    query: Query,
    tunable: &mut T,
) -> Result<Option<ParamInfo>, zenoh::Error> {
    let request: Request<ParamSetRequest> = match (&query).try_into() {
        Ok(r) => r,
        Err(_) => {
            query.reply_err("Malformed set request").await?;
            return Ok(None);
        }
    };
    match tunable.set(&request.key, request.value.clone()) {
        Ok(()) => {
            query
                .reply(query.key_expr().clone(), SendMessage::new(true))
                .await?;
            Ok(tunable.list().into_iter().find(|info| info.name == request.key))
        }
        Err(e) => {
            query.reply_err(e.to_string()).await?;
            Ok(None)
        }
    }
}

/// Serve the node's `params/get` RPC: payload is the `String` key, reply is
/// the current `ParamValue`.
pub async fn serve_params_get<T: Tunable>(
    query: Query,
    tunable: &T,
) -> Result<(), zenoh::Error> {
    let request: Request<String> = match (&query).try_into() {
        Ok(r) => r,
        Err(_) => {
            query.reply_err("Malformed get request").await?;
            return Ok(());
        }
    };
    match tunable.get(&request.parameters) {
        Some(value) => query
            .reply(query.key_expr().clone(), SendMessage::new(value))
            .await,
        None => query
            .reply_err(ParamError::Unknown(request.parameters).to_string())
            .await,
    }
}
