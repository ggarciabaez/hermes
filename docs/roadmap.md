# Hermes — Phase Roadmap

Checkbox tracker, one section per phase. Check items off as they land. Phases
aren't strictly sequential — several run concurrently in practice (Phase 2
and Phase 6 already do) — but each phase's checklist assumes the phases
listed as prerequisites are functionally done.

---

## Phase 0 — Rust Fundamentals
**Status: done.**

- [x] Serialization wrappers (`SendMessage<T>`, `RecvMessage<T>`, `Request<T>`)
- [x] `NodeName` enum seeded with initial variants
- [x] `Mode` enum + `transition()` function

---

## Phase 1 — Async Fundamentals
**Status: done.**

- [x] `tokio::select!` loop pattern proven out in a real node
- [x] Clean shutdown on `ctrl_c()` proven out

---

## Phase 2 — Gamepad
**Status: in progress.**

- [x] Controller input wiring via `botpad` (button press → internal `Command`)
- [x] Mode/EStop requests wired as RPC calls to State Machine
- [x] Two-button EStop chord (accidental-press guard)
- [x] `robot/gamepad/state` publisher (ControllerState, ticked)
- [x] Raw drive-value publisher (velocity/steering), consumed later by Controller in `Manual`
- [x] Adopt `query()`/`query_with()` helpers once Phase 3 lands, replacing the hand-rolled `session.get()` call in `request_mode_change`
- [ ] Add Gamepad's own `NodeManifest` + introspection queryable once Phase 5's pattern exists

---

## Phase 3 — RPC Pattern & Shared Plumbing
**Status: in progress.**

- [x] Prove out the query/reply dance end to end (Gamepad ↔ State Machine)
- [x] Extract `query()` — generic no-payload GET helper
- [x] Extract `query_with()` — generic payload-carrying RPC helper
- [x] Define `QueryError` (`Transport`, `NoReply`, `Denied(String)`, `Decode`)
- [?] **Tier 1:** add a `topics` module of shared `&'static str` key-expr constants; migrate Gamepad + State Machine's hardcoded strings to reference them
- [ ] **Tier 2:** define `NodeManifest` + `serve_manifest()` helper (see Phase 5 for the full shape) — worth landing here since it's generic plumbing, even though it isn't *used* until Phase 5
- [?] Open discussion, revisit once Params (Phase 4) or Achilles (Phase 5) has 3-4 real RPCs to compare: is a `RpcCall` trait (binding request/response/key-expr) worth building, or does the current two-function approach keep being enough?
- [ ] **Tier 3, explicitly deferred:** auto-declaring typed pub/sub/queryable Zenoh handles from a declarative topic list (likely a proc macro). Don't start this until the manual `declare_publisher`/`declare_subscriber`/`declare_queryable` boilerplate is visibly repeated across several real nodes — see Phase 5 design notes.

---

## Phase 4 — Params
**Status: done.** Mechanism proven end to end on Gamepad (`deadzone` plus a
drive-range clamp); other nodes pick up their own tunables the same way as
they land — Controller's Pure Pursuit gains are the natural next one when
Phase 8 exists.

- [x] Design `config.toml` shape: `[constants]` section + one
  `[params.<node_name>]` section per owning node, where `<node_name>` is
  `NodeName::name().to_lowercase()` (`[params.gamepad]`, `[params.state_machine]`, ...)
- [x] `ParamValue` enum (type-erased value for cross-node param traffic)
- [x] `Bounds<T>` min/max validation. Allowed-set validation deliberately
  deferred — no current param needs it; revisit only if one does.
- [x] `Param<T>` (value + optional `Bounds<T>`, `set()` centralizes
  reject-out-of-range)
- [x] `ParamInfo` (non-generic snapshot shape returned by a node's `list`
  queryable)
- [x] `Tunable` trait: `get(key) -> Option<ParamValue>`,
  `set(key, ParamValue) -> Result<(), ParamError>`, `list() -> Vec<ParamInfo>`
- [x] Params node: parse `config.toml` once at boot
- [x] Params node: one-shot `get` queryable at `robot/params/get` serving
  `[constants]` and per-node slices, no live subscription, no runtime write
  path. Wire shape is `Vec<(String, ParamValue)>` — postcard can't round-trip
  raw `toml::Table`/`toml::Value`.
- [x] Boot loading is a **pull**: each node waits for Params' liveliness
  (`wait_for_liveliness`), fetches its slice via `fetch_params()`, and applies
  it through its own `Tunable::set` — the same code path a live `set` RPC
  uses. (Chosen over the originally-worded Params-push dispatch.)
- [x] Each param-owning node: implement `Tunable`, serve
  `robot/<node>/params/{list,set,get}`, publish a `changed` topic on
  successful mutation
- [x] Confirm: no separate registry/mirror node gets built (Achilles queries
  directly — see Phase 5)
- [x] Wire one real node's tunables end to end as a proof — Gamepad's
  `deadzone` / `v_min` / `v_max` (section renamed `[params.gpad]` →
  `[params.gamepad]`; the old `[params.sm]` `initial_mode` dropped — initial
  mode is always `Standby`)

---

## Phase 5 — Introspection + Achilles
**Status: not started.** This is the deepest phase in this doc — Achilles
touches almost every convention decided so far (liveliness, manifests, params,
mode), so getting its shape right matters more than usual.

### 5A — Introspection primitives (shared, in `hermes-types` / RPC helper crate)

- [ ] `TopicInfo` — key-expr + role (published / subscribed / served) + optional short description
- [ ] `QueryableInfo` — key-expr + short description of what it does
- [ ] `NodeManifest` — `NodeName`, `Vec<TopicInfo>` (pub + sub combined, tagged by role), `Vec<QueryableInfo>`
- [ ] `Introspectable` trait: `fn manifest(&self) -> &NodeManifest`
- [ ] `serve_manifest(query, &manifest)` one-liner helper (Tier 2 from Phase 3)
- [ ] Decide the introspection queryable's key-expr convention — proposal: `robot/introspect/<node_name>`, using `Node::name()` (already exists) to build it. This is the same kind of convention question that was previously held off pending stabilization — flagging that Achilles is the actual forcing function to settle it now, not a silent resurrection of the earlier deferred idea.
- [ ] Retrofit State Machine and Gamepad with a manifest + the introspection queryable branch, since they're the two nodes that already exist — cheap validation that the pattern actually works before other nodes copy it

### 5B — Achilles bootstrap

- [ ] Separate binary, own Cargo target, own Zenoh session (never shares a session with a control node)
- [ ] egui + Zenoh async bootstrap plumbing (this is the piece Sim Frontend will later reuse)
- [ ] Background-task-per-discovered-topic/node pattern (Exception 2 from `NODE_GUIDE.md`) — confirm this is where that exception is actually exercised, not simulated

### 5C — Node discovery (liveliness-driven)

- [ ] Subscribe to `robot/liveliness/**` (wildcard) via Zenoh's liveliness API
- [ ] On a node's liveliness token appearing: add it to Achilles' internal node-registry state, mark "discovering"
- [ ] On appearance, immediately issue a `get` against that node's `robot/introspect/<name>` queryable to fetch its `NodeManifest`
- [ ] On liveliness token retracting (clean shutdown or session death): mark the node "offline" in the UI rather than deleting it outright, so a crash is visually distinguishable from "never existed"
- [ ] Handle the race where Achilles itself starts *after* other nodes are already up — liveliness subscriptions in Zenoh replay existing tokens on subscribe, confirm this actually happens and isn't something Achilles needs to special-case with an extra query-all-nodes step at boot

### 5D — Graph construction (manifest-driven, never raw-payload-driven)

- [ ] Build an internal graph: nodes as vertices, edges inferred by matching `TopicInfo.key_expr` where one node's `publishes` matches another's `subscribes` (or a queryable/RPC pairing)
- [ ] Handle partial manifests gracefully — a node whose introspection `get` timed out should show up as "liveliness seen, manifest unknown" rather than crashing graph construction
- [ ] Re-query a node's manifest if its liveliness token flaps (goes down and comes back) — a restarted node's manifest could in principle differ across builds
- [ ] Render: a graph/network view (nodes + edges) as the default Achilles screen

### 5E — Per-node detail view

- [ ] Clicking a node shows its full manifest: every topic it publishes/subscribes/serves, with role tags
- [ ] For topics whose wire type is known to Achilles at compile time (since Achilles links `hermes-types` directly, it can decode any type already defined there — `Mode`, `ModeRequest`, `ControllerState`, etc.), show a live-updating decoded value when the user opens that topic
- [ ] For topics with a type Achilles doesn't know how to decode (shouldn't normally happen since all wire types live in the shared crate, but guard for it anyway), fall back to showing raw byte length / a "undecodable" indicator rather than panicking
- [ ] Subscriptions for live values are best-effort priority, per the existing priority convention — confirm this is set explicitly, not left at Zenoh's default

### 5F — Live parameter view (replaces the dropped Registry node)

- [ ] Wildcard `get` across every discovered node's own `list` queryable (e.g. `robot/params/*/list`), using Zenoh's native multi-reply `get` to fan out and collect every node's response in one call
- [ ] Aggregate results into a per-node, per-param table in the UI
- [ ] Editable fields call that owning node's `set` RPC directly — Achilles never mutates a cached value and calls it done; every edit is a real round-trip
- [ ] **Client-side-only lockout:** disable/grey out edit controls unless Achilles' own subscribed copy of `robot/mode/current` currently reads `Standby`. This is enforced only in Achilles' UI, not by the owning node's `set` RPC itself — matches the earlier decision that node-level enforcement isn't worth the complexity here, since the person is the one choosing to tweak
- [ ] Re-fetch the full param table on demand (button) rather than continuously polling every node's `list` queryable

### 5G — Mode control panel

- [ ] Buttons for `SetMode` requests (`Standby`, `Manual`, `Auto`, `Halt`) — Achilles is an allowlisted `SetMode` source
- [ ] Display the current mode prominently (subscribed from `robot/mode/current`, or via `get_mode()` on load) since it gates the param-edit lockout above
- [ ] Surface `SetMode` rejections (from the three-step gate) as a visible error, not just a silent no-op

### 5H — Fault / log visibility

- [ ] Subscribe to the fault-report topic (same channel Lifeguard subscribes to) for a raw feed of reported faults
- [ ] Correlate fault-report timestamps against `robot/mode/current` changes to show "this report likely caused this mode change" — best-effort, not authoritative (Achilles isn't the one deciding, just displaying)
- [ ] Flag as an open question for whichever future conversation designs Lifeguard's logging in detail: does Lifeguard expose its own log as a queryable/stream Achilles can read directly, or is Achilles expected to reconstruct causality itself from the raw fault-report feed? Not decided here.

### 5I — Validation

- [ ] Point Achilles at the currently-running State Machine + Gamepad and confirm the graph, manifest view, and mode-control panel all work against real (not mocked) nodes before building out 5F/5H further

---

## Phase 6 — State Machine + Lifeguard
**Status: in progress (State Machine); Lifeguard not started.**

- [x] State Machine: hold current `Mode`, no-payload `get` queryable
- [x] State Machine: `set` queryable, three-step gate (allowlist → transition → SWStop lockout)
- [x] State Machine: clean `Halt` shutdown
- [ ] State Machine: add `NodeManifest` + introspection queryable (Phase 5 retrofit)
- [ ] Resolve or explicitly re-confirm the two open mode-table questions before Lifeguard locks in behavior against them: `Auto → Halt` legality, and whether the SWStop lockout should ever admit a Lifeguard escalation exception
- [ ] Lifeguard: subscribe to scan/pose for direct hazard detection
- [ ] Lifeguard: subscribe to the fault-report topic for externally-reported faults
- [ ] Lifeguard: shared evaluate/log function used by both self-detected and externally-reported faults (self-detected skips the publish step)
- [ ] Lifeguard: `SetMode` RPC call when evaluation warrants a mode change — same call any other allowlisted source uses
- [ ] Lifeguard: add `NodeManifest` + introspection queryable

---

## Phase 7 — Sim Physics Engine + Driver Nodes
**Status: not started.**

- [ ] Sim Physics Engine: own physics tick, independent of query rate
- [ ] Sim Physics Engine: internal `sim/*` topics for synthetic motor-state, scan, imu
- [ ] VESC Driver: real path (serial+JSON) and sim path (subscribe to Sim Physics Engine), same canonical output topic either way
- [ ] RPLidar Driver: same real/sim split
- [ ] IMU Driver: same real/sim split
- [ ] Confirm all three drivers' canonical topics are genuinely identical in shape between sim and real before anything downstream depends on them

---

## Phase 8 — Controller
**Status: not started.**

- [ ] Subscribe to LiDAR, `robot/mode/current`, and Gamepad's raw drive topic
- [ ] Pure Pursuit implementation for `Auto`
- [ ] Manual pass-through: convert Gamepad's raw drive values into the same `Twist` output type, so bounds/limits apply uniformly regardless of source
- [ ] Publish `Twist`
- [ ] `Tunable` impl for Controller's own gains (feeds Phase 4's end-to-end proof if not already done elsewhere)
- [ ] Later: MPC via `spawn_blocking` + `biased;` race against mode-change branches (Exception 1 from `NODE_GUIDE.md`) — explicitly not required for the first working version (Pure Pursuit is enough to validate the node's shape)

---

## Phase 9 — SLAM, Localization, Global Planner
**Status: not started.**

- [ ] SLAM: subscribe to scan + odom, run only under `Manual`, no dedicated mode of its own
- [ ] SLAM: save finished map to disk; confirm nothing subscribes to it live during the run
- [ ] Confirm the run-then-save-then-later-load model end to end: a *separate*, later run's Params config points at the saved map — no runtime write path into a live Constants/Params instance
- [ ] Localization: subscribe to scan + odom, read map path from Params, active in both `Manual` and `Auto`
- [ ] Global Planner: serve/publish waypoints, reading precomputed lines from Params rather than computing live

---

## Phase 10 — Sim Frontend
**Status: not started.**

- [ ] Separate binary, reusing Achilles' egui/Zenoh bootstrap plumbing from Phase 5B
- [ ] Visual sim front-end: render the car/environment state from sim topics
- [ ] Confirm it doesn't duplicate Achilles' introspection/graph logic — Sim Frontend is a visualization tool for the simulated world, not a second copy of Achilles

---

## Phase 11 — Integration & Hardening
**Status: not started.**

- [ ] Full system boot: Params → every node waiting on Params' liveliness → mode requests flowing → Controller producing Twist
- [ ] Settle the `Auto → Halt` open question with real end-to-end testing, now that the full system exists to test against
- [ ] Settle the SWStop-lockout escalation question the same way
- [ ] Revisit the Tier 3 auto-endpoint-declaration idea (Phase 3) now that every node's `declare_publisher`/`declare_subscriber`/`declare_queryable` boilerplate is visible across the whole system — decide, with real evidence, whether it's worth building
- [ ] Revisit the `RpcCall` trait question (Phase 3) the same way, now that Params + Achilles have added several more real RPCs to compare against
- [ ] Decide whether Params and Constants (already merged into one node) should further absorb the Zenoh-router storage/memory plugin option, or whether that stays parked indefinitely