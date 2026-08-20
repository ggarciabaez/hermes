# Achilles — Implementation Plan

Plan for turning Achilles into the full debug/visualization tool for the Hermes
Zenoh nodes. Saved so it can be revisited later; deviations during implementation
should be noted here.

## Current state

- `liveliness.rs` — background tokio task: opens a Zenoh session, subscribes to
  `robot/*/liveliness` (history replay), and on `Put` queries
  `robot/<domain>/manifest` (100 ms timeout, empty `Manifest` fallback) then
  sends `LivelinessChange::{Created,Dropped}` over an unbounded channel.
  `extract_domain()` splits on `/` into exactly 3 parts — breaks on 4-segment
  endpoint keys like `robot/gamepad/params/changed`.
- `main.rs` — egui app (`eframe 0.35`, `egui_graphs` git dep pinned at
  `e48d176`). On `Created` it adds one domain node per manifest and connects it
  to every other domain node (fully-connected mesh, empty labels). On `Dropped`
  it removes the domain node. Left panel has dead controls (Name, Timeout
  slider, Force Liveliness toggle — none wired), a "Use Force on Graph"
  checkbox, and a raw `GraphChange` dump. Node payload is `()`, all nodes are
  identical fixed-radius circles.
- `examples/animated_nodes.rs` — scratch demo, not part of the app.

## Deviations from the plan (kept as-is)

- Liveliness key: `robot/<domain>/liveliness` (not `robot/liveliness/**`).
- Manifest key: `robot/<domain>/manifest` (plan proposed `robot/introspect/<name>`).
- Manifest shape: untyped `Manifest { publishes, subscribes, serves:
  Vec<String> }` (no `TopicInfo`/`NodeManifest`/`Introspectable`).
- No registry node — Achilles queries nodes directly.
- Graph is manifest-driven, never raw-payload-driven.

## Missing (vs Phase 5 of the roadmap)

- Real graph topology: endpoint nodes, ownership edges, data edges, wildcards.
- Visual separation between domains and endpoints.
- Node detail/inspector view.
- Message viewer (live decoded values on pub topics).
- Live param table + `set` RPC editing + Standby lockout.
- Mode control panel (`SetMode` buttons + current mode display).
- Fault/log view — blocked: no fault channel (`Severity`/`FaultReport`) exists.

## Plan

### Milestone 1 — Graph visualization (primary)

**1a. Fix `extract_domain`** (`liveliness.rs:18`) to use `splitn(3, '/')` so
`domain` is always segment 2 and the endpoint is the remaining path.

**1b. Data model** (`model.rs`):
- `enum NodeKind { Domain, Endpoint(EndpointKind) }`,
  `enum EndpointKind { Publisher, Queryable }`.
  Subscribers are NOT endpoint nodes; a domain's `subscribes` entries become
  data-edges from the matching publisher endpoint node to the subscribing
  domain node.
- `NodeData { kind: NodeKind, key: String }` node payload;
  `EdgeData { kind: Owner | Data }` edge payload.
- Records: `domain_records` (`node`, `manifest`, `manifest_loaded`, owned
  endpoint keys, subscription patterns); `endpoint_records` (`node`, owner,
  kind); a set of existing data-edge keys for idempotent reconcile.

**1c. Graph construction on `Created { name, manifest }`**:
1. Parse domain, add domain node (custom large shape, domain color).
2. For each `publishes`/`serves` entry → endpoint node (small shape, pub=green /
   queryable=orange) + owner edge domain→endpoint.
3. Store `subscribes` patterns; run `reconcile_edges()`: for every
   (sub-pattern, known publisher endpoint) where
   `pub_key_expr.intersects(&pattern)` add a data edge
   publisher-endpoint → subscribing-domain. Re-run after every endpoint
   addition so late-arriving publishers wire up existing subscriptions.

**1d. Removal on `Dropped`**: remove all owned endpoint nodes first, then the
domain node; clean all maps (keep delete, not offline-mark).

**1e. Visual separation** (`shape.rs`, custom `DisplayNode`):
- Domains: large square (blue); endpoints: small circle (green=pub,
  orange=queryable).
- `set_color()` per node so selection/stroke hooks work as today.
- Uniform thin gray edges with labels; data edges carry the pub key as label.
  (Edge-kind coloring via custom `DisplayEdge` is optional polish.)

**1f. Partial manifests / flaps**: on `Created` with empty manifest (query
timeout), render the domain dimmed with `manifest_loaded=false` and retry the
manifest query (bounded, ~1 s). On liveliness re-Put, rebuild that domain's
endpoints from the fresh manifest.

### Milestone 2 — Node interaction tools

**2a. Inspector panel**: selecting a node shows its manifest (domain) or full
path/kind/owner (endpoint). Buttons per pub endpoint to open the message viewer.

**2b. Message viewer for pub nodes** (`decoders.rs` + per-topic background sub):
- Decoder registry: known concrete topics → typed decoders (all types live in
  hermes-types, which Achilles links): `robot/state_machine/current → Mode`,
  `robot/gamepad/state → ControllerState`, `robot/gamepad/drive → GPadDrive`,
  `*/params/changed → ParamInfo` (suffix match). Fallback = byte length + hex +
  "unknown type".
- Opening a viewer spawns one background subscriber (Exception 2 pattern),
  writing decoded entries into a shared ring buffer the panel renders
  (timestamp + key + value). Best-effort/background priority set explicitly.
  Closing drops the subscriber + task.

### Milestone 3 — Params modifier view + mode panel

**3a. Mode panel** (`mode.rs`): subscribe `robot/state_machine/current` via
`sub_mode()` + initial `get_mode()`; prominent current-mode display; buttons
`Standby/Manual/Auto/Halt` → `query_with::<ModeRequest, bool>` with
`source: NodeName::Achilles` (allowlisted); surface `QueryError::Denied`.

**3b. Params view** (`params.rs`):
- "Refresh" → wildcard fan-out on `robot/*/params/list`, collect all replies →
  per-domain/per-param table (name, value, bounds).
- Edit commit → real `query_with::<ParamSetRequest, bool>` to
  `robot/<domain>/params/set`; show `Denied` string on rejection.
- Live updates via per-domain `params/changed` subscriptions + manual refresh
  (no continuous polling).
- Lockout: edit controls disabled unless current mode == `Standby` (client-side).

### Milestone 4 — hermes-types additions (small, additive)

- Combined setup fn: add `Node::setup_with_params<T: Tunable + Send + Sync +
  'static>(&mut self, params: Arc<tRwLock<T>>) -> Result<(), zenoh::Error>` that
  calls `spawn_params_server(params)` then `activate_liveliness()` (liveliness
  stays last). Retrofit gamepad-adapter to use it so deadzone/v_min/v_max are
  live-served (gives the param view real data).
- `rpc::query_all<T>`: multi-reply fan-out helper (collect all replies with a
  timeout) for the `params/list` wildcard get.

### Deferred / blocked

- Fault/log view — blocked on `Severity`/`FaultReport` existing.
- Typed manifests (`TopicInfo`, `Introspectable`) — kept untyped per
  plan-deviation policy.

## Module layout

`main.rs` (shell) · `model.rs` (graph model + reconcile) · `shape.rs` (custom
`DisplayNode`) · `liveliness.rs` (discovery, fixed) · `inspector.rs` ·
`message_viewer.rs` · `params.rs` · `mode.rs` · `decoders.rs`

## Validation

`cargo build --workspace`, then run `python orchestrator.py LAUNCH.toml` +
achilles and confirm: domains cluster with their endpoints; data edge from
`state_machine/current` pub → gamepad domain; node drop removes node +
endpoints; message viewer decodes `Mode`/`ControllerState`/`GPadDrive`; param
table lists gamepad's 3 params and refuses edits outside `Standby`; mode
buttons round-trip.

## Decisions agreed with user

- Subscriber edges: pub endpoint → subscribing domain (no subscriber endpoint
  nodes).
- Visual separation: custom node shape (size + color).
- Liveliness drop: delete node + endpoints (no offline marking).
- Param server: a combined `setup`-style function that activates liveliness and
  starts the params server, to encourage nodes to do both.

## Progress / deviations noted during implementation

- Milestone 1 (graph), 2 (inspector + message viewer), and 3 (params + mode)
  are implemented; build + clippy clean.
- Message viewer: limited to ONE viewer at a time; renders only the LATEST
  decoded message in a single monospace element (no scrollable history, no
  ring buffer). The background task holds the egui `Context` and calls
  `request_repaint()` on each sample so the panel updates as data arrives.
- Per-subscriber QoS priority (plan 2b) is not possible: zenoh 1.10 only
  exposes priority on publishers, not subscribers (session-level config only).
  Left at default.