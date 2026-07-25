# Node Design Guide (v2)

A companion to `node_template.rs`. Same spirit as the gamepad crate's templates: not a
library you depend on, but a shape you copy and fill in, so every node reads the same
way six months from now.

This version adds two sanctioned exceptions to the core rule below, plus standard
handling for mode and fault reporting, now that the node set includes a State Machine
and a debug/aggregator tool that don't fit the original model as-is.

## Core rule: one event loop, `tokio::select!`, no ambient concurrency

A **control node** has exactly one place where it decides what to do next: a `select!`
loop merging every input source it cares about — command subscriber, RPC queryable(s),
param queryable, `state/mode`, shutdown signal. Nothing runs on its own background task
unless it is *genuinely* independent of node state and order doesn't matter. This is a
deliberate choice: in a control-adjacent system you want a single, readable place where
"what can happen next" is enumerable. If two things truly need to run concurrently and
independently, that's two nodes, not one node with two background tasks quietly
mutating shared state.

This rule applies to every node in the system **except** the two categories below,
which are structurally different problems and are documented as exceptions rather than
worked around silently.

## Exception 1: long CPU-bound solves (`spawn_blocking` + race)

A synchronous solve (MPC via `clarabel`) inside a `select!` branch blocks that branch
until it returns — `select!` can't interrupt work already running, only choose among
futures that haven't started polling. Two rules for this case:

- The solve runs via `tokio::task::spawn_blocking`, never inline on a normal worker
  thread — a blocking synchronous call on a shared runtime thread stalls everything
  else scheduled there, including higher-priority branches in *other* nodes if they
  happen to share a runtime.
- The `select!` loop races the resulting `JoinHandle` against higher-priority branches
  (`state/mode` changes, especially `EStop`), with `biased;` ordering so the priority
  branch is checked first when both are ready:

  ```rust
  select! {
      biased;
      _ = mode_rx.changed() => {
          if *mode_rx.borrow() == Mode::EStop {
              publish_stop().await;
              // solve_handle is dropped/ignored — it keeps running to
              // completion on the blocking pool, its result is just unused
          }
      }
      result = &mut solve_handle => {
          publish_twist(result?).await;
      }
  }
  ```

  This is offload, not true preemption — the OS thread keeps computing until the solver
  returns — but it's functionally equivalent for safety purposes: the node stops
  waiting on it and acts immediately.

## Exception 2: aggregator/tool nodes

Some nodes (Achilles; anything similar later) have a fundamentally
different shape: an unbounded, dynamically-changing set of inputs discovered at
runtime, where "latest value wins" is the correct semantics rather than an ordering
guarantee. Forcing these into one `select!` loop doesn't work — you can't add branches
at runtime.

These nodes instead run one background task per discovered topic/node, each writing
into shared state (a `tokio::watch` per topic, or an `Arc<Mutex<GraphState>>` for
aggregate views), which the UI reads once per frame and triggers a repaint from. This is
sanctioned specifically because these nodes make no control decisions — a UI frame
reading a value that's one tick stale is a non-event, unlike a control node acting on
stale mode state. If a node in this category ever starts *deciding* things (not just
displaying them) — e.g. issuing `SetParam` calls based on live values rather than
direct user action — that decision path should go through the same validated queryable
any other caller uses, not a shortcut through its own cached state.

## Every control node looks like this, in order

1. **State struct** — everything the node owns: cached subscriptions data, tunable
   params (with bounds loaded from config), handles to things it calls out to. No
   global mutable statics.
2. **`Tunable` impl** (if it has any live-adjustable params) — `get`/`set`/`list`,
   validation lives here, not in the caller. Bounds come from `config.toml` where
   declarative bounds are sufficient; dynamic bounds are checked in code and a
   violation is reported via the fault channel (see below), not silently clamped.
3. **`Introspectable` impl** — every node exposes its manifest (what it
   publishes/subscribes/serves, with types) via a queryable. This is what the debug
   tool uses to build its graph; keep it accurate as topics are added.
4. **Message handlers** — plain functions/methods taking `&mut State` and a decoded
   message, returning what (if anything) to publish. Keep these free of Zenoh types
   where possible.
5. **`main`** — wires up the session, declares a liveliness token, declares
   subscriber(s)/queryable(s), runs the `select!` loop, handles shutdown.

## Mode handling is a standard, high-priority branch

Every control node subscribes to `state/mode` and treats it as one of the first
branches checked in its `select!` loop (`biased;`, mode/estop checked before ordinary
work). A node never infers or guesses mode from other signals — `state/mode`, published
solely by the State Machine node, is the only source of truth. On `Mode::Halt`,
treat it exactly like the `ctrl_c` shutdown branch below — this is not a special case,
it's the same cleanup path triggered by a different signal.

## Fault reporting is available to any control node, not just Safety Monitor

Any node that detects its own fault condition (a validation failure outside declared
bounds, a stalled sensor, an internal error worth surfacing) publishes to
`f1tenth/safety/report` with a severity and message, using the same fault-report type
from the types crate that Safety Monitor uses. This is a fire-and-forget pub, not a
queryable — the node doesn't need or wait for a response; State Machine decides the
consequence and logs centrally. Don't build a bespoke local-logging path for faults —
route them here so there's one place to look.

## Serving vs. calling

- **Serving** (this node is the authority on something): declare a queryable, reply to
  each query on the same loop iteration you received it — no spawning a task to handle
  a query unless the work inside is genuinely long-running (see Exception 1), and even
  then, spawn + reply-when-done rather than fire-and-forget.
- **Calling** (this node needs something from another node): a small async fn wrapping
  `session.get()`, always with an explicit timeout, always returning a `Result` the
  caller has to handle. Never `.unwrap()` a remote call in a control-loop node.

## Param mutation is just another queryable, not a special case

`SetParam`/`GetParam`/`ListParams` get handled in the exact same match arm structure as
any other queryable request. There's no separate "config subsystem." This applies
regardless of caller — a gamepad trigger, the Params Registry's boot-time load, and the
Achilles's live-tweak UI all go through the identical `set` queryable on the owning
node. No caller gets a privileged bypass.

## Shutdown is explicit

Every control node's `select!` includes a branch on `tokio::signal::ctrl_c()` *and*
treats `Mode::Halt` from the State Machine identically — both break the loop cleanly
after stopping actuators, closing resources, and flushing state. No relying on the
process getting SIGKILL'd.

## What NOT to do

- Don't `tokio::spawn` a handler just because you can — spawning means you've given up
  the ordering guarantee the `select!` loop exists to provide, unless you're in one of
  the two documented exceptions above.
- Don't let a node read another node's state directly (shared memory, global, whatever)
  — if you need it, that's a subscribe or a query, full stop.
- Don't put validation logic in the caller of `set_param` — the owning node validates,
  always.
- Don't infer mode from anything other than `state/mode` — no guessing based on which
  topics are currently publishing.
- Don't build a node-local logging path for faults that bypasses `safety/report` — one
  channel, one place to look.
