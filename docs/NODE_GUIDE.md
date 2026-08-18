# Node Design Guide

A companion to the crate types doc. Same idea throughout: this is a shape you
copy and fill in, not a library you depend on, so every node reads the same
way to whoever looks at it next.

## Core rule: one event loop, `tokio::select!`, no ambient concurrency

A control node has exactly one place where it decides what to do next: a
`select!` loop merging every input source it cares about — subscribers,
queryables, a shutdown signal. Nothing runs on its own background task unless
it is genuinely independent of node state and order doesn't matter. If two
things truly need to run concurrently and independently, that's two nodes,
not one node quietly mutating shared state from a background task.

This rule has two sanctioned exceptions:

**Exception 1 — long CPU-bound solves.** A synchronous solve (MPC) inside a
`select!` branch blocks that branch until it returns. Run it via
`tokio::task::spawn_blocking`, and race the resulting `JoinHandle` against
higher-priority branches (mode changes, especially `EStop`) with `biased;`
ordering so the priority branch is checked first when both are ready. The
orphaned solve keeps running to completion on the blocking pool even after
its result is ignored — that's an accepted cost, not a bug, so long as the
node's normal solve cadence is fast enough that an abandoned solve dies
quickly relative to the next one starting.

**Exception 2 — aggregator/tool nodes.** Achilles and anything shaped like it
have a fundamentally different problem: an unbounded, dynamically-changing
set of inputs discovered at runtime, where "latest value wins" is correct
rather than an ordering guarantee. These nodes run one background task per
discovered topic/node, writing into shared state that a UI reads once per
frame. Sanctioned specifically because these nodes never *decide* anything —
a UI frame reading a value that's one tick stale is a non-event. If a node
like this ever starts issuing writes based on its own cached state rather
than direct user action, that write goes through the same validated
queryable any other caller uses — no shortcut through the cached state.

## Late-joiner state: a `get` queryable, not a heartbeat

If a node owns a piece of single-writer state that other nodes need to see
even after joining late (State Machine's `Mode` is the reference case), serve
a no-payload `get` queryable that reads the same in-memory value the
`select!` loop already owns:

```rust
let mode_get_qbl = session.declare_queryable("robot/mode/get").await.unwrap();
let mut mode = Mode::Standby;

loop {
    select! {
        biased;
        Ok(query) = mode_get_qbl.recv_async() => {
            query.reply(query.key_expr().clone(), SendMessage::new(&mode)).await.unwrap();
        }
        Ok(query) = mode_set_qbl.recv_async() => {
            // ...mutate `mode`, publish on change...
        }
    }
}
```

This deliberately avoids a periodic heartbeat publish — a heartbeat adds
constant network traffic for a value that changes rarely, and it's not
needed anyway: liveliness tokens already answer "is this node alive," and a
`get` answers "what's its value right now" on demand, only when someone
actually needs it.

## The RPC calling convention: `query()` / `query_with()`

Rather than hand-rolling the get/receive/decode sequence at every call site,
two small generic functions do it once:

```rust
#[derive(Debug)]
pub enum QueryError { Transport, NoReply, Denied(String), Decode }

/// No-payload GET — e.g. reading current mode.
pub async fn query<T: DeserializeOwned>(
    session: &zenoh::Session,
    key_expr: &str,
    timeout: Duration,
) -> Result<T, QueryError> {
    let replies = session.get(key_expr).timeout(timeout).await
        .map_err(|_| QueryError::Transport)?;
    let reply = replies.recv_async().await.map_err(|_| QueryError::NoReply)?;
    match reply.result() {
        Ok(sample) => {
            let recv: RecvMessage<T> = sample.clone().try_into().map_err(|_| QueryError::Decode)?;
            Ok(recv.payload)
        }
        Err(e) => Err(QueryError::Denied(format!("{:?}", e))),
    }
}

/// Payload-carrying RPC — e.g. requesting a mode change.
pub async fn query_with<P: Serialize, T: DeserializeOwned>(
    session: &zenoh::Session,
    key_expr: &str,
    payload: P,
    timeout: Duration,
) -> Result<T, QueryError> {
    let replies = session.get(key_expr)
        .payload(SendMessage::new(payload))
        .timeout(timeout)
        .await
        .map_err(|_| QueryError::Transport)?;
    let reply = replies.recv_async().await.map_err(|_| QueryError::NoReply)?;
    match reply.result() {
        Ok(sample) => {
            let recv: RecvMessage<T> = sample.clone().try_into().map_err(|_| QueryError::Decode)?;
            Ok(recv.payload)
        }
        Err(e) => Err(QueryError::Denied(format!("{:?}", e))),
    }
}
```

Reading current mode from any node becomes one line:

```rust
pub async fn get_mode(session: &zenoh::Session) -> Mode {
    query::<Mode>(session, "robot/mode/get", Duration::from_millis(50))
        .await
        .unwrap_or(Mode::Standby)
}
```

Requesting a mode change from Gamepad becomes:

```rust
async fn request_mode_change(command: Command, session: &zenoh::Session) -> Result<(), String> {
    let mode = /* map Command -> Mode as before */;
    match query_with::<ModeRequest, bool>(
        session,
        "robot/mode/request_mode",
        ModeRequest { mode, source: Node::Gamepad },
        Duration::from_millis(200),
    ).await {
        Ok(_ack) => println!("Current mode: {}", mode),
        Err(e) => println!("Error changing mode: {:?}", e),
    }
    Ok(())
}
```

This is deliberately just two functions, not a trait binding request type,
response type, and key-expr together. That trait-based approach was
considered and set aside for now — with only a couple of RPCs in the system,
it's not yet clear whether real RPCs share enough shape to make the
abstraction worth its own complexity. Revisit this once Params or Achilles is
underway and there's more than one or two RPCs to compare.

Server-side decoding of an incoming query's payload uses the existing
`Request<T>: TryFrom<&Query>` wrapper — no change needed there.

## Access control on RPCs is per-node, not a shared pattern

State Machine's `set` queryable checks source, then transition legality, then
a situational lockout, in that order — but this three-step shape is specific
to what State Machine needs, not a template every RPC must follow. Most other
RPCs won't need to know or care who's calling (they won't inspect a `source`
field at all), and that's fine. The one rule that does generalize: **any
outgoing `query`/`query_with` call must have an explicit timeout** — never
call one of these helpers, or `session.get()` directly, without one.

## Every control node looks like this, in order

1. **State struct** — everything the node owns: cached subscription data,
   tunable params, handles to things it calls out to. No global mutable
   statics.
2. **`Tunable` impl** (if it has live-adjustable params) — `get`/`set`/`list`,
   validation lives here, not in the caller.
3. **`Introspectable` impl** — every node exposes a manifest describing what
   it publishes/subscribes/serves. This is what Achilles uses to build its
   graph.
4. **Message handlers** — plain functions taking `&mut State` and a decoded
   message, returning what (if anything) to publish. Keep these free of
   Zenoh types where possible.
5. **`main`** — wires up the session, declares a liveliness token, declares
   subscriber(s)/queryable(s), runs the `select!` loop, handles shutdown.

## Mode handling is a standard, high-priority branch

Every control node subscribes to mode changes (or reads it via `get_mode()`
at points where it needs a fresh value) and treats it as one of the first
branches checked in its `select!` loop, `biased;` ordering, mode/estop
checked before ordinary work. A node never infers mode from anything other
than the value State Machine publishes/serves.

## Fault reporting is Lifeguard's channel, not State Machine's

Any node that detects its own fault condition publishes to the fault-report
topic. Lifeguard is the sole subscriber, evaluates severity, and decides
whether to call State Machine's `set` RPC as a consequence. This is a
fire-and-forget pub — the reporting node doesn't wait for or need a reply.
Don't build a bespoke local-logging path for faults; route them through this
one channel so there's one place to look. Lifeguard's own self-detected
faults skip the publish step (it would otherwise be both publisher and
subscriber of its own channel) but go through the identical evaluate/log
logic as an externally-reported one.

## Serving vs. calling

- **Serving** (this node is the authority on something): declare a
  queryable, reply to each query on the same loop iteration you received it
  — no spawning a task to handle a query unless the work inside is genuinely
  long-running (Exception 1 above), and even then, spawn + reply-when-done
  rather than fire-and-forget.
- **Calling** (this node needs something from another node): use `query()`
  or `query_with()`, always with an explicit timeout, always handling the
  `Result` — never `.unwrap()` a remote call in a control-loop node.

## Serving tunables

A param-owning node implements `hermes_types::params::Tunable` on the struct
holding its params, then serves three queryables and one publisher, all
declared in its own domain (`robot/<node>/params/...`):

- **`params/list`** — no-payload get; `serve_params_list(query, &params)`
  replies with `Vec<ParamInfo>`.
- **`params/get`** — payload is the `String` key; `serve_params_get(query,
  &params)` replies with the current `ParamValue`.
- **`params/set`** — payload is `ParamSetRequest { key, value }`;
  `serve_params_set(query, &mut params)` decodes, calls `Tunable::set` (all
  rejection logic lives in the node's own `set` impl — bounds, type, unknown
  key), replies `true` on success or a `reply_err` otherwise, and returns the
  updated `ParamInfo` so the caller publishes it on `params/changed`:

  ```rust
  Ok(query) = params_set.get() => {
      if let Ok(Some(info)) = serve_params_set(query, &mut params).await {
          params_changed.put(&info).await.ok();
      }
  }
  ```

  The `changed` publish happens on the same loop iteration as the successful
  mutation — no background task, no deferred emit. Borrowing `&mut params` in
  the branch *body* (never in a `select!` condition) keeps the one-loop rule
  and the borrow checker happy.

Boot loading is the same `Tunable::set` path: the node waits for the Params
liveliness token (`wait_for_liveliness`), calls `fetch_params()`, and applies
each `(key, ParamValue)` pair through `set`. A param-owning node typically
waits for Params before starting its control loop, falling back to its
defaults if Params never appears.

## Shutdown is explicit

Every control node's `select!` includes a branch on `tokio::signal::ctrl_c()`
and treats `Mode::Halt` identically — both break the loop cleanly after
stopping actuators, closing resources, and flushing state. No relying on the
process getting SIGKILL'd.

## What NOT to do

- Don't `tokio::spawn` a handler just because you can — that gives up the
  ordering guarantee the `select!` loop exists to provide, unless you're in
  one of the two documented exceptions.
- Don't let a node read another node's state directly (shared memory,
  global, whatever) — if you need it, that's a subscribe or a query, full
  stop.
- Don't put validation logic in the caller of a `set` — the owning node
  validates, always.
- Don't infer mode from anything other than what State Machine
  publishes/serves — no guessing based on which topics are currently active.
- Don't build a node-local logging path for faults that bypasses the
  fault-report channel.
- Don't call `query()`/`query_with()` (or `session.get()` directly) without
  an explicit timeout.
