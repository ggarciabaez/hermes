# hermes-types

The shared foundation: every type that crosses Zenoh, plus the `Node` helper
that wraps session plumbing. Every node depends on this crate; nothing
hand-rolls its own message shapes.

## Features

| Module | Contents |
|---|---|
| `lib.rs` | `NAMESPACE`, `NodeName`, `Manifest`, the three endpoint handles, `Node`, `wait_for_liveliness` |
| `messages.rs` | `SendMessage<T>`, `RecvMessage<T>`, `MsgMetadata` — pub/sub encode/decode |
| `rpc.rs` | `Request<T>`, `QueryError`, `query`, `query_with` — RPC both directions |
| `params.rs` | `ParamValue`, `Bounds<T>`, `Param<T>`, `ParamInfo`, `ParamError`, `Tunable`, the three `serve_params_*` helpers |
| `sm.rs` | `Mode`, `transition()`, `ModeRequest` |
| `gpad.rs` | `GPadDrive`, re-export of `botpad::ControllerState` |

### Identity and naming — `NodeName`

One variant per node in the system (`lib.rs:31`). It is the single source for
four derived things, so a node never writes a key literal:

```rust
NodeName::SM.name()                      // "State_Machine"
NodeName::SM.domain()                    // "robot/state_machine/"
NodeName::SM.make_endpoint("request_mode")  // "robot/state_machine/request_mode"
NodeName::SM.get_priority()              // 0  (intended launch order; unused)
```

The same lowercased name also keys the `[params.<node>]` config section and the
`robot/<node>/liveliness` token.

### The `Node` helper

`Node::new` (`lib.rs:132`) clones the session, computes the domain, and declares
a `robot/<node>/manifest` queryable backed by an `Arc<RwLock<Manifest>>`. The
manifest is maintained automatically: each declaration appends to it
(`update_manifest`, `lib.rs:178`), so introspection can never drift from what
was actually declared.

| Method | Effect |
|---|---|
| `activate_liveliness()` | declares `robot/<node>/liveliness`; call last, after all declarations |
| `declare_pub(name)` | `PublisherHandle` at `<domain>/name`; `put()` wraps in `SendMessage` |
| `declare_sub(key_expr)` | `SubscriberHandle` at a **full** key expression (you subscribe to others' domains) |
| `declare_queryable(name)` | `QueryableHandle` at `<domain>/name` |
| `get_mode()` | one-shot query to `robot/state_machine/get`, 100 ms timeout |
| `sub_mode()` | subscribe `robot/state_machine/current` |
| `fetch_params()` | Params query for this node's `[params.<name>]` slice, 100 ms |
| `fetch_constants()` | Params query for `[constants]`, 100 ms |
| `spawn_params_server(Arc<RwLock<T: Tunable>>)` | declares and serves `params/{list,set,get}` + `params/changed` on a background task |

The three handles (`PublisherHandle`, `SubscriberHandle`, `QueryableHandle`)
exist so a `select!` branch reads uniformly: `handle.get()` returns the
receive future for both subscribers and queryables.

`wait_for_liveliness(&session, &NodeName, timeout)` (`lib.rs:300`) polls a
node's liveliness key until it appears or the deadline passes, returning
`bool`. This is the boot-ordering primitive — it is how a node waits for Params
without a fixed sleep.

### Serialization

Postcard over three wrappers. `SendMessage<T>` on the way out, `RecvMessage<T>`
(`TryFrom<Sample>`, carries key_expr/kind/timestamp) for pub/sub in,
`Request<T>` (`TryFrom<&Query>`) for RPC in. All three `Deref` to the payload,
so `msg.field` works directly.

### RPC

```rust
query::<Mode>(&session, key, timeout).await?             // no payload
query_with::<Req, Resp>(&session, key, req, timeout).await?  // with payload
```

Both return `RecvMessage<T>` and `QueryError { Transport, NoReply, Denied(String), Decode }`.
`Denied` carries the serving side's `reply_err` string — that is where a
`ParamError` or State Machine's "Access Denied" surfaces on the caller side.
**Every call takes an explicit timeout; there is no default.**

### Params system

Typed inside a node, type-erased on the wire:

- `Param<T> { name, value, bounds: Option<Bounds<T>> }` — `set()` (`params.rs:135`)
  is the single mutation choke point and rejects out-of-range with
  `ParamError::OutOfRange`.
- `ParamValue { Float, Int, Bool, Str }` — the wire form, with `From`/`TryFrom`
  conversions in both directions. `f64` accepts an `Int` so TOML `x = 1` can back
  a `Param<f64>`.
- `Tunable { get, set, list }` — implemented by the node's params struct; `set`
  is the only path by which a tunable changes, whether from boot load or a live
  RPC.
- `serve_params_list/get/set` — decode the query, call the trait, reply or
  `reply_err`; `serve_params_set` returns the updated `ParamInfo` so the caller
  publishes it on `params/changed`.
- `params_from_toml` — extracts scalars from a TOML table into
  `Vec<(String, ParamValue)>`. Raw TOML never crosses the wire because postcard
  cannot round-trip `toml::Value`.

### Mode

`Mode { Standby, Manual, Auto, EStop, SWStop, Halt }` and `transition(cur, new)
-> bool` (`sm.rs:24`), which encodes the legality table. `transition` is used
**only** by State Machine's gate; other nodes read mode, never evaluate
transitions. `ModeRequest { mode, source }` is the RPC payload, with `source`
being the caller's `NodeName`.

## Guide: extending the crate

- **Adding a wire type**: put it in the module matching its owner (`sm.rs` for
  mode-related, a new module for a new subsystem), derive
  `Serialize, Deserialize, Debug, Clone`, keep it postcard-friendly (no
  `toml::Value`, no trait objects, no borrowed lifetimes). Add it to the table
  in `docs/hermes-types.md`.
- **Adding a node-level capability** (something every node should be able to do
  in one line): add a method to `Node` rather than a free function, following
  `get_mode`/`sub_mode` — they exist so no node writes
  `robot/state_machine/...` by hand.
- **Adding a param type**: implement `From<T> for ParamValue` and
  `TryFrom<ParamValue> for T` (`params.rs:35`–`99`); everything else is generic
  over those two.

## Missing / known issues

- **Introspection is a stub.** `Manifest` is three `Vec<String>`s with no type
  information, and `TopicInfo<T>` (`lib.rs:83`) is an unused placeholder with a
  `TODO` to replace the strings. There is no `Introspectable` trait. Achilles
  cannot tell what payload type a topic carries.
- **`Request<T>::try_from` unwraps a missing payload** (`rpc.rs:24`): a query
  sent with no payload panics the *serving* node rather than returning a decode
  error. This is reachable from any peer on the network.
- **`SendMessage<T> -> ZBytes` panics on serialization failure**
  (`messages.rs:43`), so an unserializable payload aborts the publisher instead
  of surfacing an error.
- **`RecvMessage::new` fabricates metadata** with `key_expr` `"a/b"` and no
  timestamp (`messages.rs:67`) — a placeholder that will read as a real
  key expression to anything inspecting metadata.
- **Timeouts are hardcoded** to 100 ms in `get_mode`, `fetch_params`,
  `fetch_constants` — not tunable, and tight for a loaded network or a
  cold-started peer.
- **The liveliness key format is duplicated** as a literal
  `format!("robot/{}/liveliness", ...)` in both `activate_liveliness`
  (`lib.rs:171`) and `wait_for_liveliness` (`lib.rs:306`), instead of deriving
  from `domain()`. Two places to change, one convention.
- **`declare_sub` is asymmetric** with `declare_pub`/`declare_queryable`: it
  takes a full key expression while the others take a domain-relative name. It
  is documented but easy to get wrong. A `TODO` at `lib.rs:127` proposes taking
  `NodeName + endpoint` instead.
- **`get_priority()` is dead code** — no launcher reads it.
- **No `Severity`/`FaultReport`** — the fault channel type set does not exist.
- **No sensor or geometry types** — `LaserScan`, `ImuData`, `VescTelemetry`,
  `Pose`, `Twist`, `OccupancyGrid`, `Path` are all pending their consuming node.
- **No `ctrl_c`/shutdown helper**, so every node would re-implement the shutdown
  branch by hand (and currently none do).
- **`config.rs` was removed** in the working tree; configuration now lives
  entirely in the Params node plus `params.rs`. Nothing references it.
- **No tests.**
