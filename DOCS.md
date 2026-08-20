# Hermes — System Documentation

Hermes is a sim-first F1TENTH autonomy stack written in Rust, with **Zenoh as the
only inter-node transport**. Every runnable unit is a separate process ("node")
that talks to the others exclusively over Zenoh key expressions — there is no
shared memory, no in-process bus, and no node reaches into another node's state.

This file documents what actually exists in the tree today. Design intent that
has not been built yet lives in `docs/` (`f1tenth-rust-plan.md`,
`NODE_GUIDE.md`, `hermes-types.md`, `roadmap.md`); where the two disagree, this
file and the per-folder `DOCS.md` files describe the code, and `docs/` describes
the target.

## Documentation map

| File | Covers |
|---|---|
| `DOCS.md` (this file) | System architecture, key-expression map, wire format, boot order, build/run |
| `nodes/DOCS.md` | **The node authoring guide** — what every node must do, in order |
| `nodes/*/DOCS.md` | One per node: features, endpoints, gaps |
| `crates/DOCS.md` | What belongs in a shared crate vs. in a node |
| `crates/*/DOCS.md` | One per library crate: API surface, gaps |
| `docs/f1tenth-rust-plan.md` | Full target architecture (mostly unbuilt) |
| `docs/roadmap.md`, `ROADMAP.md` | Phase checklists (two variants, see *Gaps*) |

`tools/` (Achilles, the egui debug tool) is deliberately out of scope here.

## Workspace layout

```
hermes/
├── crates/
│   ├── hermes-types/     shared wire types + the Node helper (every node depends on this)
│   └── hermes-macros/    setup_endpoints! declaration macro
├── nodes/
│   ├── gamepad-adapter/  physical controller → drive topic + mode RPCs
│   ├── state-machine/    sole authority over Mode
│   └── params/           serves Config.toml (constants + per-node param slices)
├── tools/achilles/       egui debug tool (out of scope for these docs)
└── docs/                 design docs and roadmaps
```

Workspace members are listed in the root `Cargo.toml`; adding a node is
`cargo new nodes/<name>` plus one line in `members` (see
`docs/new_component.txt`).

## Architecture in one page

**Bus.** Every node opens its own `zenoh::open(zenoh::Config::default())`
session (peer mode, multicast scouting — no router process is required for a
local run). Two patterns are used, chosen per use case:

- **pub/sub** for streaming or "latest value wins" data: controller state,
  drive values, mode changes.
- **queryable/get (RPC)** for request/reply: mode changes, param get/set/list,
  manifests.

**Namespace.** `NAMESPACE = "robot/"` (`crates/hermes-types/src/lib.rs:26`).
Every key a node owns lives under its own domain,
`robot/<node_name_lowercased>/`, computed by `NodeName::domain()`
(`crates/hermes-types/src/lib.rs:58`). Nothing publishes outside its own domain.

**Identity.** `NodeName` (`crates/hermes-types/src/lib.rs:31`) has one variant
per node in the system, built or not. It is simultaneously: the RPC-caller
identity carried in payloads, the domain-name source, the liveliness-key
source, and the `[params.<node>]` config-section key. Adding a node starts by
adding a variant here.

**Discovery.** Each node declares a Zenoh liveliness token at
`robot/<node>/liveliness` once it is ready to serve
(`Node::activate_liveliness`, `crates/hermes-types/src/lib.rs:167`). Zenoh
retracts it on clean shutdown *or* process death, which makes it the
crash-detection mechanism for the whole system. `wait_for_liveliness()`
(`crates/hermes-types/src/lib.rs:300`) is the boot-ordering primitive built on
top of it.

**Introspection.** `Node::new` declares a `robot/<node>/manifest` queryable for
free (`crates/hermes-types/src/lib.rs:143`), served from a callback over an
`Arc<RwLock<Manifest>>` that `declare_pub`/`declare_sub`/`declare_queryable`
append to as endpoints are declared. So a node's manifest is always an accurate
list of what it declared, without the node author maintaining it by hand.

**Mode.** `Mode` (`crates/hermes-types/src/sm.rs:7`) is the single global
operating state: `Standby | Manual | Auto | EStop | SWStop | Halt`. State
Machine is the only writer. Every other node reads it — never infers it —
either by subscribing (`Node::sub_mode`) or by a one-shot query
(`Node::get_mode`). `Mode::Halt` is the system-wide shutdown signal: every node
breaks its loop on it.

**Params.** One Params node parses one `Config.toml` at boot and serves it.
Boot loading is a **pull**: a node waits for the Params liveliness token, calls
`fetch_params()`, and applies each `(key, ParamValue)` through its own
`Tunable::set` — the identical code path a live `set` RPC would take, so there
is exactly one way a tunable ever changes. See
`crates/hermes-types/DOCS.md` for the type-level detail.

## Wire format

All payloads are [postcard](https://docs.rs/postcard)-encoded Rust types. There
is no JSON, no schema file, no IDL — the shared Rust type *is* the schema, which
is why every cross-node type must live in `hermes-types` and nowhere else.

Three wrappers do all the encoding/decoding
(`crates/hermes-types/src/messages.rs`, `crates/hermes-types/src/rpc.rs`):

| Wrapper | Direction | Used for |
|---|---|---|
| `SendMessage<T>` | outgoing | anything put or replied; `Into<ZBytes>` via postcard |
| `RecvMessage<T>` | incoming | `TryFrom<Sample>` — pub/sub payload + key_expr/kind/timestamp |
| `Request<T>` | incoming | `TryFrom<&Query>` — RPC payload + key_expr |

`PublisherHandle::put` wraps in `SendMessage` for you
(`crates/hermes-types/src/lib.rs:110`); on the receiving side you name the
concrete type and `try_into()`:

```rust
let curmode: RecvMessage<Mode> = sample.try_into().unwrap();
let request: Request<ModeRequest> = (&query).try_into()?;
```

Because both sides are Rust and postcard is not self-describing, publisher and
subscriber must agree on `T` exactly. A mismatched type is a decode error at
runtime, not a compile error — this is the main hazard of the design and the
reason nothing hand-rolls its own message shapes.

## Key expression map (as implemented)

| Key | Kind | Owner | Payload |
|---|---|---|---|
| `robot/<node>/manifest` | queryable | every `Node` | `Manifest { publishes, subscribes, serves }` |
| `robot/<node>/liveliness` | liveliness token | every `Node` (after `activate_liveliness`) | — |
| `robot/state_machine/current` | pub | State Machine | `Mode` |
| `robot/state_machine/get` | queryable | State Machine | → `Mode` |
| `robot/state_machine/request_mode` | queryable | State Machine | `ModeRequest` → `bool` / `reply_err` |
| `robot/gamepad/state` | pub | Gamepad | `ControllerState` |
| `robot/gamepad/drive` | pub | Gamepad | `GPadDrive { v, y }` |
| `robot/params/get` | queryable | Params | `ParamsGetRequest` → `Vec<(String, ParamValue)>` |
| `robot/<node>/params/list` | queryable | any `Tunable` node | → `Vec<ParamInfo>` |
| `robot/<node>/params/get` | queryable | any `Tunable` node | `String` → `ParamValue` |
| `robot/<node>/params/set` | queryable | any `Tunable` node | `ParamSetRequest` → `bool` / `reply_err` |
| `robot/<node>/params/changed` | pub | any `Tunable` node | `ParamInfo` |

Note `NodeName::SM.name()` is `"State_Machine"`, so its domain is
`robot/state_machine/` — not `robot/sm/` and not `robot/mode/` (the latter
appears in `docs/NODE_GUIDE.md` and in `nodes/gamepad-adapter/examples/`, both
of which predate the current naming).

The `params/*` row group is served by `Node::spawn_params_server`
(`crates/hermes-types/src/lib.rs:246`), which exists and compiles but **no node
calls it yet** — see *Cross-cutting gaps*.

## Boot and runtime order

1. **Params** starts first (`NodeName::get_priority()` ranks it 0, alongside
   State Machine). It reads `Config.toml`, declares `robot/params/get`, and
   activates liveliness.
2. **State Machine** starts, publishes the initial `Mode::Standby`, and
   activates liveliness. Nothing else can resolve a mode until it is up.
3. **Every other node** constructs its `Node`, optionally waits on the Params
   liveliness token and pulls its param slice, declares its endpoints, seeds
   its cached mode via `get_mode()`, activates liveliness *last*, and enters its
   `select!` loop.
4. **Shutdown** is a mode change: something requests `Mode::Halt`, State Machine
   publishes it and exits, and every subscriber sees `Halt` and breaks its own
   loop, then calls `session.close()`. Dropping a `Node` retracts its liveliness
   token and undeclares its manifest queryable, so peers see it go away.

`get_priority()` encodes the intended launch ordering (0 = first) but nothing
reads it — there is no launcher yet.

## Build and run

```bash
cargo build --workspace          # builds cleanly today
cargo run -p params              # MUST be run from the workspace root, see below
cargo run -p state-machine
cargo run -p gamepad-adapter     # needs a connected gamepad (gilrs/botpad)
```

Order matters: Params, then State Machine, then the rest. Gamepad waits up to 5s
for Params and falls back to compiled-in defaults if it never appears, but it
has no such fallback for State Machine — with no State Machine running,
`get_mode()` times out and it starts in `Standby`, where it publishes controller
state but no drive values.

The Params node reads `./nodes/params/Config.toml` as a **hardcoded relative
path** (`nodes/params/src/main.rs:10`), so it only works when launched from the
workspace root.

Examples (`cargo run -p gamepad-adapter --example sub`, `--example smoke` under
`params`) are scratch programs from earlier phases; several use pre-rename key
expressions and are not maintained as tests.

## Status

| Component | State |
|---|---|
| `hermes-types` | Working. Wire types, `Node` helper, params system. |
| `hermes-macros` | Working. One declarative macro. |
| `params` node | Working, single-file config, read-only serving. |
| `state-machine` node | Working. Full mode gate, no fault channel. |
| `gamepad-adapter` node | Working. Boot params, no live param server. |
| Lifeguard, Controller | `NodeName` variants exist; nothing built. |
| Sensor/sim/planning nodes | Not started. |
| Achilles | Started, out of scope for these docs. |

## Cross-cutting gaps

Items that are missing everywhere rather than in one node. Per-node gaps live in
each folder's own `DOCS.md`.

- **No `ctrl_c` handling anywhere.** `docs/NODE_GUIDE.md` requires every node's
  `select!` to have a `tokio::signal::ctrl_c()` branch treated identically to
  `Mode::Halt`. No node has one, so Ctrl-C kills processes without stopping
  actuators or closing the session. This is the single most consistent
  deviation from the documented design.
- **No Zenoh priority or congestion control.** The plan calls for gamepad
  EStop on high priority with `Drop` congestion control, and Achilles on
  background/best-effort. Nothing sets either; `NodeName::get_priority()` is
  dead code.
- **No `Introspectable` trait, no typed manifests.** `Manifest` carries bare
  `Vec<String>`; `TopicInfo<T>` (`crates/hermes-types/src/lib.rs:83`) is an
  unused stub. Achilles cannot learn payload types from a manifest.
- **No fault channel.** `Severity`, `FaultReport`, and the Lifeguard node do not
  exist, so "all faults go through one channel" is currently unenforced — nodes
  `eprintln!` locally.
- **No tests.** Zero `#[test]` functions in `crates/` or `nodes/`. Pure logic
  that is trivially testable today — `sm::transition`, `Param::set` bounds
  rejection, `ParamValue` conversions, `params_from_toml` — has no coverage.
- **No structured logging.** Everything is `println!`/`eprintln!` with ad-hoc
  `[params]`-style prefixes. No `tracing`, no levels, no way to turn output down
  on a node under test.
- **No launcher or orchestration.** Starting the system is manual, ordered by
  hand, in three terminals. A `TODO` at `crates/hermes-types/src/lib.rs:62`
  notes the intent to build one.
- **Panics on the hot path.** `.unwrap()`/`.expect()` on Zenoh and decode
  results is pervasive, including in State Machine, which is the system's sole
  mode authority — a single malformed request payload takes it down and with it
  the whole system's ability to change mode. `Request<T>::try_from` itself
  unwraps a missing payload (`crates/hermes-types/src/rpc.rs:24`).
- **Two divergent roadmaps.** `ROADMAP.md` (root, time-estimate oriented, phase
  numbering through 12) and `docs/roadmap.md` (status-oriented, different phase
  numbering) describe the same project with conflicting phase numbers. One
  should be deleted or made canonical.
- **`Cargo.lock` is gitignored** (`.gitignore:4`) even though this workspace
  produces binaries, so builds are not reproducible from a fresh clone.
- **`docs/` has drifted.** `NODE_GUIDE.md` shows `robot/mode/get`, a
  `Node::Gamepad` enum name, and `query()` returning `T` rather than
  `RecvMessage<T>` — all superseded by the current code.
