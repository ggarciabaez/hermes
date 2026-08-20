# Node Authoring Guide

How a Hermes node is built, derived from the three `main.rs` files that exist
(`gamepad-adapter`, `state-machine`, `params`). This is a shape you copy, not a
framework you inherit from: `hermes_types::Node` gives you the plumbing, but the
ordering and the loop are yours to write correctly each time.

Read `../docs/DOCS.md` first for the architecture and key-expression map. Design
rationale that is not yet implemented lives in `../docs/NODE_GUIDE.md`.

## What a node is

One binary crate under `nodes/`, one process, one Zenoh session, one
`tokio::select!` loop. A node owns some state, exposes it over Zenoh, and reacts
to inputs. If two things inside one node need to run truly concurrently and
independently, that is two nodes — not one node mutating shared state from a
background task.

## The mandatory sequence

Every node does these in this order. Steps marked **required** apply to all
nodes, including future ones; steps marked *conditional* apply when the node has
that concern.

### 1. Declare identity — required

```rust
use hermes_types::{Node, NodeName};
const NAME: NodeName = NodeName::Gamepad;
```

A new node starts by adding a variant to `NodeName`
(`crates/hermes-types/src/lib.rs:31`) plus its `name()` and `get_priority()`
arms. That one variant gives you the domain (`robot/<name>/`), the liveliness
key, the `[params.<name>]` config section, and the caller identity other nodes
allowlist against. Do not hardcode a domain string anywhere.

### 2. Open exactly one session — required

```rust
#[tokio::main]
async fn main() {
    let session = zenoh::open(zenoh::Config::default()).await.unwrap();
```

One session per process. Pass `&session` where a helper needs it and clone it
only when a spawned task genuinely needs its own handle (`Session` clones are
cheap handles to the same session, not new sessions).

### 3. Construct the `Node` — required

```rust
let mut node = Node::new(&session, &NAME).await.unwrap();
```

This declares the `robot/<node>/manifest` queryable for you and starts an empty
manifest that later declarations append to. Keep `node` alive for the entire
run: dropping it undeclares the manifest queryable **and retracts the liveliness
token**, which peers read as "this node died".

### 4. Load params — *conditional* (node has tunables or needs constants)

```rust
let mut params = GamepadParams::defaults();
if wait_for_liveliness(&session, &NodeName::Params, Duration::from_secs(5)).await {
    match node.fetch_params().await {
        Ok(list) => for (key, value) in list {
            if let Err(e) = params.set(&key, value) {
                eprintln!("[params] ignoring {}: {}", key, e);
            }
        },
        Err(e) => eprintln!("[params] failed to fetch params: {:?}", e),
    }
} else {
    eprintln!("[params] Params node not seen within 5s; using defaults");
}
```

Three rules here, all load-bearing:

- **Always start from compiled-in defaults**, so the node boots with Params
  absent. A node that cannot run without Params is a node that cannot be
  debugged alone.
- **Apply every value through `Tunable::set`**, never by assigning fields. `set`
  is the single choke point where bounds and type checking happen, and boot
  loading must not be a second path that skips them.
- **A rejected key is a warning, not a crash.** A typo in `Config.toml` should
  not take the node down.

Use `fetch_constants()` for the shared `[constants]` section (wheelbase, map
paths) and `fetch_params()` for this node's own `[params.<name>]` slice.

### 5. Declare endpoints — required

```rust
setup_endpoints!(
    node,
    pubs: [ state_node = "state", drive_node = "drive" ],
    queries: [ ]
);
```

`pubs:` and `queries:` names are **domain-relative** — `"state"` becomes
`robot/gamepad/state`. Subscribers are the exception: `declare_sub` takes a
**full key expression**, because you subscribe to other nodes' domains:

```rust
let mode_node = node.sub_mode().await.unwrap();          // robot/state_machine/current
let other = node.declare_sub(&NodeName::Controller.make_endpoint("twist")).await?;
```

Always build a foreign key with `NodeName::make_endpoint`, never a string
literal. Both macro sections must be present even when empty (see
`crates/hermes-macros/DOCS.md`), and the macro `unwrap()`s, so a declaration
failure panics here rather than returning.

### 6. Serve tunables — *conditional* (node implements `Tunable`)

```rust
let params = Arc::new(tokio::sync::RwLock::new(params));
node.spawn_params_server(params.clone()).await.unwrap();
```

This declares `params/{list,set,get}` plus `params/changed` in your domain and
runs them on a background task. It is the one sanctioned background task in a
node — parameter serving is independent of control state, and it publishes
`changed` on the same iteration as the mutation. **No node currently calls
this**, so it is untested in practice; the first node to adopt it should verify
that holding the `RwLock` across the control loop's reads does not stall it.

### 7. Seed the cached mode — required (every node except State Machine)

```rust
let mut mode = node.get_mode().await.unwrap_or(Mode::Standby);
```

A node that starts after a mode change must not act on a stale assumption, and
the mode publisher only fires *on change* — so the one-shot query is how a late
joiner catches up. `Standby` is the correct fallback: it is the "ready but do
nothing" mode.

### 8. Activate liveliness — required, and last

```rust
node.activate_liveliness().await.unwrap();
```

This is the node's "I am ready to serve" announcement, and other nodes block on
it in step 4. Call it **after** every queryable is declared and after any boot
load, never before — a peer that sees the token and immediately queries an
endpoint you have not declared yet gets a timeout.

### 9. One `select!` loop — required

```rust
loop {
    tokio::select! {
        biased;
        Ok(mode_q) = mode_node.get() => {
            let curmode: RecvMessage<Mode> = mode_q.try_into().unwrap();
            mode = curmode.payload;
            if mode == Mode::Halt { break; }
        },
        // ... this node's own work, in descending priority ...
    }
}
session.close().await.unwrap();
```

Rules for the loop:

- **`biased;` first**, with the mode branch first inside it. `select!` is random
  by default; `biased;` makes it check branches top-down, so a pending mode
  change (especially `EStop`) is always seen before ordinary work.
- **Never infer mode** from anything but this branch or `get_mode()`. Not from
  which topics are active, not from your own last command.
- **`Mode::Halt` breaks the loop.** It is the system-wide shutdown signal.
- **Keep branch bodies short.** Awaiting a slow RPC inside a branch stalls every
  other branch — Gamepad does exactly this and it is listed as a known issue
  there.
- **Borrow `&mut` state in the branch *body*, never in the branch condition** —
  the condition futures are all created before any body runs, so a `&mut` in a
  condition conflicts with every other branch.
- **Close the session** after the loop.

### 10. What the loop must not do

- Don't `tokio::spawn` a handler to get concurrency; that discards the ordering
  guarantee the single loop exists to provide. The sanctioned exceptions are
  `spawn_params_server` (step 6), a `spawn_blocking` CPU-bound solve raced
  against the mode branch, and aggregator tools like Achilles.
- Don't reach into another node's state; subscribe or query.
- Don't validate another node's input on the caller side — the owning node
  validates.
- Don't call `query`/`query_with`/`session.get()` without an explicit timeout.

## Required additions not yet implemented

These are required by `../docs/NODE_GUIDE.md` and absent from all three existing
nodes. New nodes should include them; existing ones should be retrofitted.

- **A `ctrl_c` branch** in the `select!`, handled identically to `Mode::Halt`:

  ```rust
  _ = tokio::signal::ctrl_c() => {
      println!("ctrl_c — shutting down");
      break;
  }
  ```

- **A stop action before breaking.** Any node that commands actuation must
  publish a zero/neutral command on the way out, not just stop publishing.
- **Priority and congestion control** on safety-critical publishers (EStop path:
  high priority, `Drop` congestion control).
- **Fault reporting** to Lifeguard's channel instead of local `eprintln!`, once
  that channel exists.

## Reference skeleton

```rust
use std::time::Duration;
use hermes_types::{Node, NodeName, RecvMessage, sm::Mode, wait_for_liveliness};
use hermes_macros::setup_endpoints;

const NAME: NodeName = NodeName::Controller;

#[tokio::main]
async fn main() {
    let session = zenoh::open(zenoh::Config::default()).await.unwrap();
    let mut node = Node::new(&session, &NAME).await.unwrap();

    // 4. params (if any)
    let mut params = MyParams::defaults();
    if wait_for_liveliness(&session, &NodeName::Params, Duration::from_secs(5)).await {
        if let Ok(list) = node.fetch_params().await {
            for (k, v) in list {
                if let Err(e) = params.set(&k, v) { eprintln!("[params] {}: {}", k, e); }
            }
        }
    }

    // 5. endpoints
    setup_endpoints!(node, pubs: [ out = "twist" ], queries: [ ]);
    let mode_node = node.sub_mode().await.unwrap();

    // 7. + 8.
    let mut mode = node.get_mode().await.unwrap_or(Mode::Standby);
    node.activate_liveliness().await.unwrap();

    // 9.
    let mut ticker = tokio::time::interval(Duration::from_millis(20));
    loop {
        tokio::select! {
            biased;
            Ok(sample) = mode_node.get() => {
                let m: RecvMessage<Mode> = sample.try_into().unwrap();
                mode = m.payload;
                if mode == Mode::Halt { break; }
            },
            _ = tokio::signal::ctrl_c() => break,   // not yet in existing nodes
            _ = ticker.tick() => {
                if mode == Mode::Auto { out.put(&compute()).await.ok(); }
            }
        }
    }
    session.close().await.unwrap();
}
```

## Compliance of the existing nodes

| Step | gamepad-adapter | state-machine | params |
|---|---|---|---|
| 1 `NodeName` const | yes | yes | yes |
| 2 single session | yes | yes | yes |
| 3 `Node::new` | yes | yes | yes |
| 4 param load | yes | n/a | n/a (is the server) |
| 5 endpoints via macro | yes | yes | manual `declare_queryable` |
| 6 `spawn_params_server` | **no** (has `Tunable`, never serves it) | n/a | n/a |
| 7 seed mode | yes | n/a (owner) | **no** (subscribes only) |
| 8 liveliness last | yes | yes | yes |
| 9 `biased;` mode-first loop | yes | **no `biased;`** | yes |
| 9 `Halt` breaks | yes | yes | yes |
| `ctrl_c` branch | **no** | **no** | **no** |
| stop action on exit | **no** | n/a | n/a |
| session closed | yes | yes | yes |

## Adding a new node

1. `cargo new nodes/<name>`, add `"nodes/<name>"` to the root `Cargo.toml`
   `members`.
2. Depend on `hermes-types` (and `hermes-macros` if you use the macro) by path:
   `hermes-types = { path = "../../crates/hermes-types" }`.
3. Add the `NodeName` variant, its `name()` and `get_priority()` arms.
4. Add any new cross-node type to `hermes-types` **before** using it — never
   define a wire type in the node crate.
5. Write `main.rs` following the sequence above.
6. Add a `DOCS.md` in the node folder: features, endpoint table, gaps.
7. Update the key-expression map in `../docs/DOCS.md`.
