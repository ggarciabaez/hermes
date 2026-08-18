# hermes-types — Crate Design Reference

Single source of truth for every cross-node type in the project. Every node
depends on this crate; nothing hand-rolls its own message shapes. This crate
grows as each roadmap phase actually needs a new type — it is not built out
speculatively ahead of that need.

## Quick reference

| Name | Kind | Status | Purpose |
|---|---|---|---|
| `NodeName` | enum | implemented | RPC-caller identity |
| `Mode` | enum | implemented | Robot operating state |
| `transition()` | fn | implemented | Legality check for a mode change, used only by State Machine |
| `ModeRequest` | struct | implemented | Payload for the mode-set RPC |
| `MsgMetadata` | struct | implemented | key_expr/kind/timestamp carried alongside a decoded pub/sub payload |
| `SendMessage<T>` | struct | implemented | Outgoing payload wrapper, converts into Zenoh's byte type |
| `RecvMessage<T>` | struct | implemented | Incoming pub/sub payload + metadata |
| `Request<T>` | struct | implemented | Incoming RPC query payload + key_expr |
| `QueryError` | enum | implemented | Error type returned by the `query()`/`query_with()` helper functions |
| `Severity` | enum | not built | Fault report priority |
| `FaultReport` | struct | not built | Fault channel payload |
| `ParamValue` | enum | implemented | Type-erased value for cross-node param traffic |
| `Bounds<T>` | struct | implemented | Min/max validation window |
| `Param<T>` | struct | implemented | Typed value + optional `Bounds<T>`, `set()` rejects out-of-range |
| `ParamInfo` | struct | implemented | Snapshot returned by a node's parameter-list queryable |
| `ParamError` | enum | implemented | Domain-level rejection from `Tunable::set` |
| `ParamSetRequest` | struct | implemented | Payload for a node's `params/set` RPC |
| `ParamsGetRequest` | enum | implemented | Payload for the Params node's `get` queryable (constants vs. node slice) |
| `Tunable` | trait | implemented | Live params, implemented by every param-owning node |
| `NodeManifest` / `TopicInfo` / `QueryableInfo` | struct | not built | Self-description for Achilles' graph |
| `Introspectable` | trait | not built | Self-description, implemented by every node that needs it |

There is no shared "Command" type in this crate, and there will not be one.
Button/trigger→intent mapping is internal to whichever node owns the input
device and never crosses Zenoh in that form.

---

## 1. `NodeName`

```rust
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy)]
pub enum Node {
    Gamepad,
    SM,
    Lifeguard,
    Achilles,
    Controller,
    Params,
}
```

One variant per node. Used as the RPC-caller identity (carried in
`ModeRequest.source`) and anywhere else a message needs to say who sent it.
Add a variant as each new node comes online.

A `name()` method returning a plain debug/display string is reasonable to
add now:

```rust
impl Node {
    pub const fn name(&self) -> &'static str {
        match self {
            Node::Gamepad => "gamepad",
            Node::SM => "state_machine",
            Node::Lifeguard => "lifeguard",
            Node::Achilles => "achilles",
            Node::Controller => "controller",
            Node::Params => "params",
        }
    }
}
```

Two key conventions are now settled and live in this crate:
- **Liveliness topic** per node: `robot/<name>/liveliness` (built in
  `Node::new`, using `name().to_lowercase()`).
- **Params-config key** per node: the same `name().to_lowercase()` string
  names the `[params.<node>]` section the Params node serves, and each node's
  tunable endpoints are `robot/<name>/params/{list,set,get}` plus a
  `robot/<name>/params/changed` publisher. Achilles' later fan-out query is
  `robot/*/params/list`.

---

## 2. `Mode` and `transition()`

```rust
#[repr(u8)]
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy)]
pub enum Mode { Standby, Manual, Auto, EStop, SWStop, Halt }

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Mode::{:?}", self)
    }
}

pub fn transition(curstate: Mode, newstate: Mode) -> bool {
    match (curstate, newstate) {
        (Mode::Standby, _) => true,
        (Mode::Manual, _) => true,
        (Mode::Auto, _) => true,
        (Mode::EStop, Mode::Halt) => true,
        (Mode::SWStop, Mode::Standby | Mode::Halt) => true,
        (Mode::Halt, _) => false,
        _ => false,
    }
}
```

`transition()` is used exclusively by State Machine's mode-set gate — it is
not a generic helper other nodes are expected to call. `Mode` itself is
published (on change) and served (via a no-payload `get` queryable) solely by
State Machine; every other node treats it as read-only, sourced only from
State Machine.

Two things about this table are intentionally still open, not settled:
whether `Auto → Halt` (and `Manual → Halt`) should be legal as written here,
and whether the SWStop lockout (see `ModeRequest` below) should ever admit an
escalation exception for Lifeguard. Both are tracked in the project plan and
should not be resolved silently in code without flagging it back.

---

## 3. `ModeRequest`

```rust
#[derive(Serialize, Deserialize, Debug)]
pub struct ModeRequest {
    pub mode: Mode,
    pub source: Node,
}
```

Payload for the mode-set RPC. State Machine checks, in order: is `source` in
`{Gamepad, Achilles, Lifeguard}`; does `transition(current, mode)` hold; and,
if current mode is `SWStop`, is `source` anything other than `Lifeguard`
(Lifeguard is locked out of changing mode at all while in `SWStop` —
recovery from `SWStop` is human/Achilles-only). This three-part gate belongs
to State Machine specifically and isn't a shape other RPCs need to copy.

---

## 4. Serialization wrappers

These four types are the backbone every node uses to get data on and off the
wire, and are stable as implemented:

```rust
pub struct MsgMetadata {
    pub key_expr: KeyExpr<'static>,
    pub kind: SampleKind,
    pub timestamp: Option<Timestamp>,
}

pub struct SendMessage<T> { pub payload: T }
// SendMessage<T>: Into<ZBytes> via postcard. Panics if T fails to serialize —
// keep every wire type simple and derive-friendly.

pub struct RecvMessage<T> { pub payload: T, pub metadata: MsgMetadata }
// TryFrom<Sample> — used for anything arriving over a subscriber.

pub struct Request<T> { pub parameters: T, pub key_expr: KeyExpr<'static> }
// TryFrom<&Query> — used for anything arriving over a queryable.
```

`SendMessage`/`RecvMessage` are the pub/sub pair; `Request` is the
query-decoding counterpart on the serving side. All three `Deref` to their
inner payload for convenience.

---

## 5. RPC error type

```rust
#[derive(Debug)]
pub enum QueryError {
    Transport,      // couldn't issue the get at all
    NoReply,        // no reply arrived within the timeout
    Denied(String), // the far side replied with reply_err(..)
    Decode,         // reply arrived but didn't deserialize as expected
}
```

Returned by the `query()`/`query_with()` helper functions described in
`NODE_GUIDE.md`. This is transport/call-level only — it doesn't attempt to
carry a structured domain-specific rejection reason, since the serving side
currently replies with a plain string via `reply_err(&str)` rather than a
typed error payload. If a specific RPC later needs a richer, typed domain
error, that's a decision made for that RPC's own response type, not a reason
to expand `QueryError` itself.

The params system pairs this with a *domain-level* error on the serving side:
`ParamError` (see next section) is what a node's `Tunable::set` returns, and
the `params/set` handler turns it into the `reply_err` string a caller sees
as `QueryError::Denied`.

---

## 6. Params system

The Params node and every param-owning node share these types, all in the
`params` module:

```rust
/// Type-erased value for all cross-node param traffic.
pub enum ParamValue { Float(f64), Int(i64), Bool(bool), Str(String) }

/// Min/max validation window. Allowed-set validation is a possible future
/// extension, deliberately not built yet — no current param needs it.
pub struct Bounds<T> { pub low: T, pub high: T }

/// A typed param owned by a node. `set()` is the single choke point for
/// mutation and rejects values outside `Bounds`.
pub struct Param<T> {
    pub name: String,
    pub value: T,
    pub bounds: Option<Bounds<T>>,
}

/// Type-erased snapshot that actually crosses the wire (list reply / changed
/// publish). Not generic.
pub struct ParamInfo {
    pub name: String,
    pub value: ParamValue,
    pub bounds: Option<Bounds<ParamValue>>,
}

/// Domain-level rejection from `Tunable::set`.
pub enum ParamError { Unknown(String), TypeMismatch, OutOfRange }

/// Implemented by every param-owning node. `set` is the single code path by
/// which a tunable ever changes — boot loading and a live tweak both land
/// here.
pub trait Tunable {
    fn get(&self, key: &str) -> Option<ParamValue>;
    fn set(&mut self, key: &str, value: ParamValue) -> Result<(), ParamError>;
    fn list(&self) -> Vec<ParamInfo>;
}

/// Payload for a node's `params/set` RPC.
pub struct ParamSetRequest { pub key: String, pub value: ParamValue }

/// Payload for the Params node's `get` queryable.
pub enum ParamsGetRequest { Constants, Node(NodeName) }
```

`serve_params_list` / `serve_params_set` / `serve_params_get` are the
one-line serving-side glue: they decode the query, call the `Tunable` impl,
reply with data (or `reply_err`), and `serve_params_set` returns the updated
`ParamInfo` so the node can publish it on `params/changed`.

Conventions (settled, and now baked into code):
- Params node serves constants and per-node slices at `robot/params/get`,
  keyed by `ParamsGetRequest`. Slices are named `[params.<node>]` where
  `<node>` is `name().to_lowercase()`.
- Wire shape for params is `Vec<(String, ParamValue)>` — **not** raw TOML.
  postcard cannot round-trip `toml::Table`/`toml::Value`, so the Params node
  converts its config sections through `params_from_toml()` before replying.
- Each param-owning node serves `robot/<name>/params/{list,set,get}` and
  publishes `robot/<name>/params/changed`. Achilles' later fan-out query is
  `robot/*/params/list`.
- Boot loading is a **pull**: a node waits for the Params liveliness token
  (`wait_for_liveliness`), calls `fetch_params()` (or `fetch_constants()`),
  and applies each pair through its own `Tunable::set` — the same method a
  live `set` RPC invokes.

---

## 7. Not yet built

- `Severity`, `FaultReport` — needed once Lifeguard's fault-report channel is
  built.
- `NodeManifest`, `TopicInfo`, `QueryableInfo`, `Introspectable` — needed once
  Achilles' graph-building starts.
- `Pose`, `Twist`, `LaserScan`, `ImuData`, `VescTelemetry`, `Waypoint`/`Path`,
  `OccupancyGrid` — needed by Odometry, SLAM, Localization, Global Planner,
  Controller, and the driver nodes respectively, each added as that node is
  actually built.

---

## 8. Notes for future conversations (Context for Claude)

- No shared `Command` type exists or should be added — per-node internal
  action enums stay local.
- `transition()` and the three-step gate in `ModeRequest`'s handling are
  State-Machine-specific, not a general RPC pattern.
- `Mode`'s late-joiner visibility is a `get` queryable, not a heartbeat
  publish and not something liveliness tokens solve.
- Two `Mode`-table questions remain open (Auto→Halt legality, and whether
  Lifeguard should ever get an SWStop-escalation exception) — don't resolve
  either silently.
- Whether RPCs eventually get a `RpcCall`-style trait binding
  request/response/key-expr together is an open, parked question — not
  adopted, not rejected, revisit once more RPCs exist.
- Params and Constants are one node in this project, not two, and there is no
  separate registry/mirror node.
- The one-slot `get` queryable a param-owning node serves (`params/get`) is
  the trait's `get()` exposed over the wire. Achilles doesn't strictly need it
  (it lists then edits), but it's cheap and mirrors the trait.
- `Bounds<T>` allowed-set validation is a deliberate deferral — revisit only
  when a real param needs an allowed-set constraint, not before.
